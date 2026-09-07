//! Wiremock-based tests for `CodexStreamFn` (issue #1265).

use std::sync::{Arc, Mutex};

use base64::Engine as _;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use swink_agent::{
    AssistantMessageEvent, CredentialError, CredentialFuture, CredentialResolver, ModelSpec,
    ResolvedCredential, StreamErrorKind, StreamFn, StreamOptions,
};
use swink_agent_adapters::{CodexError, CodexStreamFn};

use crate::common::{event_name, find_error_kind, find_error_message, sse_response, test_context};

// ── Helpers ──────────────────────────────────────────────────────────────────

fn jwt_with_account(account_id: &str) -> String {
    let seg = |v: serde_json::Value| {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&v).unwrap())
    };
    format!(
        "{}.{}.sig",
        seg(serde_json::json!({"alg": "none"})),
        seg(serde_json::json!({"https://api.openai.com/auth": {"chatgpt_account_id": account_id}}))
    )
}

/// Returns a fixed credential, counting calls.
struct StaticResolver {
    credential: Result<String, ()>,
    calls: Mutex<usize>,
}

impl StaticResolver {
    fn token(token: &str) -> Arc<Self> {
        Arc::new(Self {
            credential: Ok(token.to_owned()),
            calls: Mutex::new(0),
        })
    }
    fn failing() -> Arc<Self> {
        Arc::new(Self {
            credential: Err(()),
            calls: Mutex::new(0),
        })
    }
}

impl CredentialResolver for StaticResolver {
    fn resolve(&self, key: &str) -> CredentialFuture<'_, ResolvedCredential> {
        *self.calls.lock().unwrap() += 1;
        let key = key.to_owned();
        Box::pin(async move {
            match &self.credential {
                Ok(token) => Ok(ResolvedCredential::OAuth2AccessToken(token.clone())),
                Err(()) => Err(CredentialError::NotFound { key }),
            }
        })
    }
}

fn completed_body() -> String {
    "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\nevent: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":3,\"output_tokens\":1}}}\n\n".to_owned()
}

async fn collect(stream_fn: &CodexStreamFn, options: StreamOptions) -> Vec<AssistantMessageEvent> {
    let model = ModelSpec::new("codex", "gpt-5.6-luna");
    stream_fn
        .stream(&model, &test_context(), &options, CancellationToken::new())
        .collect::<Vec<_>>()
        .await
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn sends_bearer_account_id_beta_originator_and_a_fresh_session_id() {
    let token = jwt_with_account("acct-42");
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .and(header("authorization", format!("Bearer {token}").as_str()))
        .and(header("chatgpt-account-id", "acct-42"))
        .and(header("openai-beta", "responses=experimental"))
        .and(header("originator", "superswink"))
        .respond_with(sse_response(&completed_body()))
        .expect(2)
        .mount(&server)
        .await;

    let resolver = StaticResolver::token(&token);
    let stream_fn = CodexStreamFn::new(resolver.clone(), "superswink")
        .unwrap()
        .with_base_url(server.uri());

    let first = collect(&stream_fn, StreamOptions::default()).await;
    let second = collect(&stream_fn, StreamOptions::default()).await;
    for events in [&first, &second] {
        assert_eq!(
            events.iter().map(event_name).collect::<Vec<_>>(),
            ["Start", "TextStart", "TextDelta", "TextEnd", "Done"],
            "{events:?}"
        );
    }
    assert_eq!(
        *resolver.calls.lock().unwrap(),
        2,
        "credential resolved once per request"
    );

    let requests = server.received_requests().await.unwrap();
    let session_ids: Vec<String> = requests
        .iter()
        .map(|r| {
            r.headers
                .get("session_id")
                .expect("session_id header")
                .to_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(session_ids.len(), 2);
    assert_ne!(
        session_ids[0], session_ids[1],
        "session_id must be fresh per request"
    );
    assert!(
        uuid::Uuid::parse_str(&session_ids[0]).is_ok(),
        "{}",
        session_ids[0]
    );

    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["store"], false);
    assert!(!body["instructions"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn model_not_entitled_is_a_typed_non_retryable_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(400).set_body_string(
            r#"{"error":{"message":"The model `gpt-5.1-codex` is not supported when using Codex with a ChatGPT account.","type":"invalid_request_error"}}"#,
        ))
        .expect(1)
        .mount(&server)
        .await;
    let stream_fn = CodexStreamFn::new(StaticResolver::token(&jwt_with_account("a")), "superswink")
        .unwrap()
        .with_base_url(server.uri());
    let events = collect(&stream_fn, StreamOptions::default()).await;
    assert_eq!(
        find_error_kind(&events),
        Some(Some(StreamErrorKind::ModelRetired)),
        "{events:?}"
    );
    assert!(
        find_error_message(&events)
            .unwrap()
            .contains("not available on your ChatGPT plan")
    );
}

#[tokio::test]
async fn x_codex_headers_populate_the_rate_limit_snapshot() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            sse_response(&completed_body())
                .insert_header("x-codex-plan-type", "prolite")
                .insert_header("x-codex-primary-used-percent", "98")
                .insert_header("x-codex-primary-reset-after-seconds", "288059"),
        )
        .mount(&server)
        .await;
    let seen = Arc::new(Mutex::new(None));
    let sink = Arc::clone(&seen);
    let options = StreamOptions::default()
        .with_on_rate_limit(Arc::new(move |s| *sink.lock().unwrap() = Some(s.clone())));
    let stream_fn = CodexStreamFn::new(StaticResolver::token(&jwt_with_account("a")), "superswink")
        .unwrap()
        .with_base_url(server.uri());
    collect(&stream_fn, options).await;
    let snapshot = seen.lock().unwrap().clone().expect("callback fired");
    assert_eq!(snapshot.used_percent, Some(98.0));
    assert_eq!(snapshot.plan.as_deref(), Some("prolite"));
    assert_eq!(
        snapshot.resets_in,
        Some(std::time::Duration::from_secs(288_059))
    );
}

#[tokio::test]
async fn credential_failure_and_missing_claim_are_auth_errors_without_a_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(sse_response(""))
        .expect(0)
        .mount(&server)
        .await;

    let no_cred = CodexStreamFn::new(StaticResolver::failing(), "superswink")
        .unwrap()
        .with_base_url(server.uri());
    let events = collect(&no_cred, StreamOptions::default()).await;
    assert_eq!(
        events.iter().map(event_name).collect::<Vec<_>>(),
        ["Start", "Error"]
    );
    assert_eq!(find_error_kind(&events), Some(Some(StreamErrorKind::Auth)));
    assert!(
        find_error_message(&events)
            .unwrap()
            .contains("sign in again")
    );

    let no_claim = CodexStreamFn::new(StaticResolver::token("opaque-token"), "superswink")
        .unwrap()
        .with_base_url(server.uri());
    let events = collect(&no_claim, StreamOptions::default()).await;
    assert_eq!(find_error_kind(&events), Some(Some(StreamErrorKind::Auth)));
    assert!(
        find_error_message(&events)
            .unwrap()
            .contains("chatgpt_account_id")
    );
}

#[test]
fn refuses_openai_client_originators_and_never_defaults_to_one() {
    assert!(matches!(
        CodexStreamFn::new(StaticResolver::token("t"), "codex_cli_rs"),
        Err(CodexError::InvalidOriginator(_))
    ));
    assert!(CodexStreamFn::new(StaticResolver::token("t"), "").is_err());
    let ok = CodexStreamFn::new(StaticResolver::token("t"), "superswink").unwrap();
    assert_eq!(ok.originator(), "superswink");
    assert!(
        !swink_agent_adapters::DEFAULT_ORIGINATOR
            .to_lowercase()
            .starts_with("codex")
    );
}

/// The adapter must never touch the Codex CLI's own token file (shared
/// grants get rotated out from under each other). Enforced on the source.
#[test]
fn crate_source_never_references_the_codex_cli_token_file() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    fn walk(dir: &std::path::Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                if text.contains(concat!(".codex/", "auth.json"))
                    || text.contains(concat!("auth", ".json"))
                {
                    out.push(path.display().to_string());
                }
            }
        }
    }
    walk(&src, &mut offenders);
    assert!(
        offenders.is_empty(),
        "files referencing the Codex CLI token file: {offenders:?}"
    );
}

#[test]
fn factory_builds_a_codex_connection_without_an_api_key() {
    let preset = swink_agent_adapters::preset("gpt-5.6-luna").expect("openai row is first");
    assert_eq!(
        preset.provider_key, "openai",
        "provider-blind lookup keeps the metered row"
    );
    let codex = swink_agent::model_catalog()
        .preset("codex", "gpt_5_6_luna")
        .expect("codex preset");
    let connection = swink_agent_adapters::build_connection_from_preset(&codex, None, None)
        .expect("codex needs no API key");
    assert_eq!(connection.model_spec().provider, "codex");
}
