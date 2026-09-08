//! `codex` provider: a ChatGPT **subscription** (Free/Go/Plus/Pro/Business)
//! backing agent turns instead of a metered API key.
//!
//! Thin wrapper over the Responses shell ([`ResponsesStreamFn`]): an OAuth
//! credential resolved per request, four fixed headers, a different base
//! URL. Refresh, expiry and single-flight dedup all live in the host's
//! [`CredentialResolver`] — this adapter owns **no** refresh logic.
//!
//! # Read before enabling
//!
//! * **Personal use only, bring your own login.** The endpoint is
//!   undocumented; OpenAI may change or withdraw it without notice. OpenAI's
//!   Services Agreement prohibits reselling access or using ChatGPT to power
//!   third-party services: one user, their own login, their own machine is a
//!   personal client — serving several users from one subscription is not.
//!   Do not do that.
//! * **Off by default.** Enable the `codex` Cargo feature deliberately.
//! * **Token ownership.** This adapter runs its *own* PKCE login and keeps
//!   its *own* token bundle in the host `CredentialStore`. It never reads or
//!   writes the Codex CLI's token file, and no "import from the CLI" option
//!   will be added: OpenAI rotates refresh tokens, so two clients sharing one
//!   grant silently log each other out at unpredictable times. Two
//!   independent grants cannot collide.
//! * **Failures degrade, never retry-storm.** A model the plan does not
//!   entitle, a missing account claim, or an expired grant each surface as a
//!   typed, non-retryable error with an actionable message.
//! * `originator` must be **honest**: it names *your* client. It is
//!   validated to reject OpenAI's own client identifiers.
//!
//! # Wire contract
//!
//! `POST {base_url}/responses` with the standard Responses body plus
//! `store: false` and a non-empty `instructions`, and these headers on every
//! request:
//!
//! ```text
//! Authorization: Bearer <oauth access_token>
//! chatgpt-account-id: <the token's `https://api.openai.com/auth`.`chatgpt_account_id` claim>
//! OpenAI-Beta: responses=experimental
//! originator: <your client name>
//! session_id: <uuid v4, fresh per request>
//! ```
//!
//! Quota arrives in `x-codex-*` response headers and is surfaced through
//! [`StreamOptions::on_rate_limit`] like every other adapter's.

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use futures::stream::{self, Stream, StreamExt as _};
use tokio_util::sync::CancellationToken;

use swink_agent::{
    AgentContext, AssistantMessageEvent, CredentialResolver, ModelSpec, ResolvedCredential,
    ServingOptionSupport, StreamFn, StreamOptions,
};
use swink_agent_auth::AuthorizationConfig;

use crate::responses::ResponsesStreamFn;
use crate::{HeaderMap, HeaderName, HeaderValue};

/// Base URL of the subscription-backed Responses endpoint.
pub const CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
/// OpenAI's public OAuth client for ChatGPT sign-in.
pub const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
/// Redirect URI registered for that client; see
/// [`LoopbackAuthorizationHandler`](swink_agent_auth::LoopbackAuthorizationHandler).
pub const CODEX_REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
/// Credential-store key the adapter resolves by default.
pub const DEFAULT_CREDENTIAL_KEY: &str = "codex";
/// `originator` used by [`CodexStreamFn::from_env`] when `CODEX_ORIGINATOR`
/// is unset. Names this library, truthfully.
pub const DEFAULT_ORIGINATOR: &str = "swink-agent";

const AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const SCOPES: [&str; 4] = ["openid", "profile", "email", "offline_access"];
const ACCOUNT_CLAIM: &str = "https://api.openai.com/auth";
const ENTITLEMENT_MARKER: &str = "not supported when using Codex with a ChatGPT account";
/// OpenAI's own client identifiers. Sending one would impersonate their
/// client, which is unnecessary (a truthful originator works) and indefensible.
const OPENAI_CLIENT_ORIGINATORS: [&str; 5] = [
    "codex_cli_rs",
    "codex_cli",
    "codex-cli",
    "codex_vscode",
    "codex_exec",
];

/// The PKCE public-client configuration for ChatGPT sign-in. Register it on
/// the resolver under [`DEFAULT_CREDENTIAL_KEY`] (or the key you pass to
/// [`CodexStreamFn::with_credential_key`]).
#[must_use]
pub fn codex_authorization_config() -> AuthorizationConfig {
    AuthorizationConfig::new(
        AUTHORIZE_URL,
        TOKEN_URL,
        CODEX_CLIENT_ID,
        CODEX_REDIRECT_URI,
    )
    .with_scopes(SCOPES)
    .with_pkce()
}

/// Construction-time errors.
#[non_exhaustive]
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CodexError {
    /// `originator` was empty, non-printable, or one of OpenAI's own client
    /// identifiers.
    #[error(
        "invalid originator {0:?}: must be a non-empty printable name for *your* client, not an OpenAI client id"
    )]
    InvalidOriginator(String),
}

fn validate_originator(originator: &str) -> Result<(), CodexError> {
    let trimmed = originator.trim();
    let printable =
        !trimmed.is_empty() && trimmed.bytes().all(|b| b.is_ascii_graphic() || b == b' ');
    let impersonates = OPENAI_CLIENT_ORIGINATORS
        .iter()
        .any(|c| c.eq_ignore_ascii_case(trimmed));
    if printable && !impersonates {
        Ok(())
    } else {
        Err(CodexError::InvalidOriginator(originator.to_owned()))
    }
}

/// Read `chatgpt_account_id` from the access token's `https://api.openai.com/auth`
/// claim. The JWT is *not* verified — the server does that; this only needs
/// the routing id it already trusts the token for.
fn account_id_from_token(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?.trim_end_matches('=');
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    claims
        .get(ACCOUNT_CLAIM)?
        .get("chatgpt_account_id")?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Entitlement failures are HTTP 400 with a recognisable message; surface
/// them as "switch models", never as something to retry.
fn classify_codex_error(status: u16, body: &str, provider: &str) -> Option<AssistantMessageEvent> {
    if status == 400 && body.contains(ENTITLEMENT_MARKER) {
        return Some(AssistantMessageEvent::error_model_retired(format!(
            "{provider}: this model is not available on your ChatGPT plan; choose another model (HTTP 400)"
        )));
    }
    crate::oai_transport::classify_oai_error_body(status, body, provider)
}

/// [`StreamFn`] for the ChatGPT-subscription Codex endpoint. See the
/// `codex` module documentation for the terms this comes with.
pub struct CodexStreamFn {
    inner: Arc<ResponsesStreamFn>,
    resolver: Arc<dyn CredentialResolver>,
    credential_key: String,
    originator: String,
    base_url: String,
}

impl CodexStreamFn {
    /// Build against [`CODEX_BASE_URL`], resolving the OAuth credential under
    /// [`DEFAULT_CREDENTIAL_KEY`] through `resolver` on every request.
    ///
    /// # Errors
    /// [`CodexError::InvalidOriginator`] when `originator` is empty,
    /// non-printable, or names an OpenAI client.
    pub fn new(
        resolver: Arc<dyn CredentialResolver>,
        originator: impl Into<String>,
    ) -> Result<Self, CodexError> {
        let originator = originator.into();
        validate_originator(&originator)?;
        let mut this = Self {
            inner: Arc::new(ResponsesStreamFn::new(CODEX_BASE_URL, "")),
            resolver,
            credential_key: DEFAULT_CREDENTIAL_KEY.to_owned(),
            originator,
            base_url: CODEX_BASE_URL.to_owned(),
        };
        this.rebuild_inner();
        Ok(this)
    }

    /// A ready-to-run adapter for CLI-style hosts: in-memory credential
    /// store (so sign-in repeats per process — persist by passing your own
    /// resolver to [`new`](Self::new)), the loopback + paste sign-in
    /// handler printing the URL to stderr, and `originator` from
    /// `CODEX_ORIGINATOR` or [`DEFAULT_ORIGINATOR`].
    ///
    /// # Errors
    /// See [`new`](Self::new).
    pub fn from_env() -> Result<Self, CodexError> {
        use swink_agent_auth::{
            DefaultCredentialResolver, InMemoryCredentialStore, LoopbackAuthorizationHandler,
        };
        let originator = std::env::var("CODEX_ORIGINATOR")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_ORIGINATOR.to_owned());
        let handler = LoopbackAuthorizationHandler::new(Arc::new(|url| {
            eprintln!("Sign in to ChatGPT to use the codex provider:\n\n  {url}\n\nIf this machine has no browser, open the URL elsewhere and paste the redirected URL (or the code) here:");
        }))
        .with_manual_code(Arc::new(|| {
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).ok()?;
            let line = line.trim();
            (!line.is_empty()).then(|| line.to_owned())
        }));
        let resolver = DefaultCredentialResolver::new(Arc::new(InMemoryCredentialStore::empty()))
            .with_authorization_handler(Arc::new(handler))
            .with_authorization_config(DEFAULT_CREDENTIAL_KEY, codex_authorization_config())
            .with_authorization_timeout(Duration::from_secs(600));
        Self::new(Arc::new(resolver), originator)
    }

    /// Point at a different backend (tests, proxies). Path stays `/responses`.
    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self.rebuild_inner();
        self
    }

    /// Resolve the credential under a different store key.
    #[must_use]
    pub fn with_credential_key(mut self, key: impl Into<String>) -> Self {
        self.credential_key = key.into();
        self
    }

    /// The validated `originator` sent on every request.
    #[must_use]
    pub fn originator(&self) -> &str {
        &self.originator
    }

    fn rebuild_inner(&mut self) {
        // The bearer token is per request (`StreamOptions::api_key`), so the
        // static key is empty; static headers are the two fixed ones.
        let originator = HeaderValue::from_str(&self.originator)
            .expect("validate_originator guarantees a printable ASCII value");
        self.inner = Arc::new(
            ResponsesStreamFn::new(self.base_url.clone(), "")
                .with_responses_path("/responses")
                .with_provider_label("Codex")
                .with_error_classifier(classify_codex_error)
                .with_header(
                    HeaderName::from_static("openai-beta"),
                    HeaderValue::from_static("responses=experimental"),
                )
                .with_header(HeaderName::from_static("originator"), originator),
        );
    }
}

impl std::fmt::Debug for CodexStreamFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodexStreamFn")
            .field("base_url", &self.base_url)
            .field("credential_key", &self.credential_key)
            .field("originator", &self.originator)
            .finish_non_exhaustive()
    }
}

impl StreamFn for CodexStreamFn {
    fn supported_serving_options(&self) -> ServingOptionSupport {
        self.inner.supported_serving_options()
    }

    fn stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        let resolver = Arc::clone(&self.resolver);
        let inner = Arc::clone(&self.inner);
        let key = self.credential_key.clone();
        let options = options.clone();

        Box::pin(
            stream::once(async move {
                let fail = |event| stream::iter(crate::base::pre_stream_error(event)).left_stream();

                let token = match resolver.resolve(&key).await {
                    Ok(
                        ResolvedCredential::OAuth2AccessToken(token)
                        | ResolvedCredential::Bearer(token)
                        | ResolvedCredential::ApiKey(token),
                    ) => token,
                    Ok(_) => {
                        return fail(AssistantMessageEvent::error_auth(
                            "Codex: credential resolved to an unsupported type; expected an OAuth2 access token",
                        ));
                    }
                    Err(error) => {
                        return fail(AssistantMessageEvent::error_auth(format!(
                            "Codex: could not resolve the ChatGPT credential ({error}); sign in again"
                        )));
                    }
                };
                let Some(account_id) = account_id_from_token(&token) else {
                    return fail(AssistantMessageEvent::error_auth(
                        "Codex: the access token carries no chatgpt_account_id claim; sign in again",
                    ));
                };
                let Ok(account_id) = HeaderValue::from_str(&account_id) else {
                    return fail(AssistantMessageEvent::error_auth(
                        "Codex: the chatgpt_account_id claim is not a valid header value",
                    ));
                };

                let mut headers = HeaderMap::new();
                headers.insert(HeaderName::from_static("chatgpt-account-id"), account_id);
                headers.insert(
                    HeaderName::from_static("session_id"),
                    HeaderValue::from_str(&uuid::Uuid::new_v4().to_string())
                        .expect("uuid is ASCII"),
                );

                let options = options.with_api_key(token);
                inner
                    .shell
                    .stream_with_headers(model, context, &options, cancellation_token, &headers)
                    .right_stream()
            })
            .flatten(),
        )
    }
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<CodexStreamFn>();
};

#[cfg(test)]
#[path = "codex_tests.rs"]
mod tests;
