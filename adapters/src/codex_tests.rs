//! Tests for `codex`.
#![cfg(test)]

use super::*;

fn jwt(claims: &serde_json::Value) -> String {
    let seg = |v: &serde_json::Value| {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(v).unwrap())
    };
    format!(
        "{}.{}.sig",
        seg(&serde_json::json!({"alg": "none"})),
        seg(claims)
    )
}

#[test]
fn account_id_is_read_from_the_auth_claim() {
    let token =
        jwt(&serde_json::json!({ ACCOUNT_CLAIM: { "chatgpt_account_id": "acct-1" }, "sub": "u" }));
    assert_eq!(account_id_from_token(&token).as_deref(), Some("acct-1"));
    // Padded base64 (some encoders) is tolerated.
    let padded = format!("{token}==");
    assert_eq!(account_id_from_token(&padded).as_deref(), Some("acct-1"));
    assert_eq!(
        account_id_from_token(&jwt(&serde_json::json!({"sub": "u"}))),
        None
    );
    assert_eq!(account_id_from_token("not.a.jwt"), None);
    assert_eq!(account_id_from_token("opaque-token"), None);
}

#[test]
fn originator_must_be_honest_and_printable() {
    assert!(validate_originator("superswink").is_ok());
    assert!(validate_originator("My Client 1.0").is_ok());
    assert_eq!(
        validate_originator(""),
        Err(CodexError::InvalidOriginator(String::new()))
    );
    assert!(validate_originator("   ").is_err());
    assert!(validate_originator("bad\nname").is_err());
    for c in OPENAI_CLIENT_ORIGINATORS {
        assert!(validate_originator(c).is_err(), "{c} must be rejected");
        assert!(validate_originator(&c.to_uppercase()).is_err());
    }
}

#[test]
fn entitlement_error_is_model_retired_not_generic() {
    let ev = classify_codex_error(400, r#"{"error":{"message":"The model `gpt-5.1-codex` is not supported when using Codex with a ChatGPT account."}}"#, "Codex").unwrap();
    assert!(
        matches!(
            ev,
            AssistantMessageEvent::Error {
                error_kind: Some(swink_agent::StreamErrorKind::ModelRetired),
                ..
            }
        ),
        "{ev:?}"
    );
    assert!(classify_codex_error(400, r#"{"error":{"message":"bad request"}}"#, "Codex").is_none());
}

#[test]
fn authorization_config_is_pkce_public_client() {
    let cfg = codex_authorization_config();
    assert!(cfg.use_pkce);
    assert_eq!(cfg.client_id, CODEX_CLIENT_ID);
    assert_eq!(cfg.client_secret, None);
    assert_eq!(cfg.redirect_uri, CODEX_REDIRECT_URI);
    assert!(
        cfg.scopes.iter().any(|s| s == "offline_access"),
        "refresh token needs offline_access"
    );
}
