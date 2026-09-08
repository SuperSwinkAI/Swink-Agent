//! Tests for `credential`.
#![cfg(test)]

use super::*;
use std::error::Error as _;

// T023: Credential serde roundtrip
#[test]
fn credential_serde_roundtrip_api_key() {
    let cred = Credential::ApiKey {
        key: "sk-test-123".into(),
    };
    let json = serde_json::to_string(&cred).unwrap();
    let decoded: Credential = serde_json::from_str(&json).unwrap();
    match decoded {
        Credential::ApiKey { key } => assert_eq!(key, "sk-test-123"),
        other => panic!("expected ApiKey, got {other:?}"),
    }
}

#[test]
fn credential_serde_roundtrip_bearer() {
    let cred = Credential::Bearer {
        token: "tok-abc".into(),
        expires_at: Some(chrono::Utc::now()),
    };
    let json = serde_json::to_string(&cred).unwrap();
    let decoded: Credential = serde_json::from_str(&json).unwrap();
    match decoded {
        Credential::Bearer { token, expires_at } => {
            assert_eq!(token, "tok-abc");
            assert!(expires_at.is_some());
        }
        other => panic!("expected Bearer, got {other:?}"),
    }
}

#[test]
fn credential_serde_roundtrip_oauth2() {
    let cred = Credential::OAuth2 {
        access_token: "access-123".into(),
        refresh_token: Some("refresh-456".into()),
        expires_at: None,
        token_url: "https://auth.example.com/token".into(),
        client_id: "client-1".into(),
        client_secret: Some("secret".into()),
        scopes: vec!["read".into(), "write".into()],
    };
    let json = serde_json::to_string(&cred).unwrap();
    let decoded: Credential = serde_json::from_str(&json).unwrap();
    match decoded {
        Credential::OAuth2 {
            access_token,
            refresh_token,
            client_id,
            scopes,
            ..
        } => {
            assert_eq!(access_token, "access-123");
            assert_eq!(refresh_token.as_deref(), Some("refresh-456"));
            assert_eq!(client_id, "client-1");
            assert_eq!(scopes, vec!["read", "write"]);
        }
        other => panic!("expected OAuth2, got {other:?}"),
    }
}

// T024: CredentialError Display contains no secrets
#[test]
fn credential_error_display_no_secrets() {
    let errors = vec![
        CredentialError::NotFound {
            key: "my-key".into(),
        },
        CredentialError::Expired {
            key: "my-key".into(),
        },
        CredentialError::RefreshFailed {
            key: "my-key".into(),
            reason: "bad response".into(),
        },
        CredentialError::TypeMismatch {
            key: "my-key".into(),
            expected: CredentialType::Bearer,
            actual: CredentialType::ApiKey,
        },
        CredentialError::Timeout {
            key: "my-key".into(),
        },
        CredentialError::AuthorizationFailed {
            key: "my-key".into(),
            reason: "user denied access".into(),
        },
        CredentialError::AuthorizationTimeout {
            key: "my-key".into(),
        },
    ];

    let secret_values = [
        "sk-test-123",
        "tok-abc",
        "access-123",
        "refresh-456",
        "secret",
    ];
    for err in &errors {
        let display = format!("{err}");
        for secret in &secret_values {
            assert!(
                !display.contains(secret),
                "Display of {err:?} leaks secret {secret}"
            );
        }
        // Should contain the key name for diagnostics
        assert!(
            display.contains("my-key"),
            "Display of {err:?} should contain key name"
        );
    }
}

#[test]
fn credential_store_error_display_redacts_backend_details() {
    let err = CredentialError::StoreError(Box::new(std::io::Error::other(
        "backend exploded with token=secret-value",
    )));

    assert_eq!(err.to_string(), "credential store error");

    let source = err.source().expect("store errors should retain the source");
    assert!(
        source.to_string().contains("token=secret-value"),
        "store error source should keep the backend detail for internal diagnostics"
    );
}

#[test]
fn credential_store_error_debug_redacts_backend_details() {
    let err = CredentialError::StoreError(Box::new(std::io::Error::other(
        "backend exploded with token=secret-value",
    )));

    let debug = format!("{err:?}");

    assert!(
        !debug.contains("token=secret-value"),
        "Debug leaks backend secret"
    );
    assert!(debug.contains("[REDACTED]"));
}

#[test]
fn oauth2_debug_redacts_token_url() {
    let cred = Credential::OAuth2 {
        access_token: "access-secret".into(),
        refresh_token: Some("refresh-secret".into()),
        expires_at: None,
        token_url: "https://client:token-secret@auth.example.com/token?api_key=query-secret".into(),
        client_id: "client-1".into(),
        client_secret: Some("client-secret".into()),
        scopes: vec!["read".into()],
    };

    let debug = format!("{cred:?}");

    for secret in [
        "access-secret",
        "refresh-secret",
        "client-secret",
        "token-secret",
        "query-secret",
    ] {
        assert!(
            !debug.contains(secret),
            "Debug leaks OAuth2 secret {secret}"
        );
    }
    assert!(debug.contains("token_url"));
    assert!(debug.contains("[REDACTED]"));
}

// T011: credential_type helper
#[test]
fn credential_type_helper() {
    let api_key = Credential::ApiKey { key: "k".into() };
    assert_eq!(api_key.credential_type(), CredentialType::ApiKey);

    let bearer = Credential::Bearer {
        token: "t".into(),
        expires_at: None,
    };
    assert_eq!(bearer.credential_type(), CredentialType::Bearer);

    let oauth2 = Credential::OAuth2 {
        access_token: "a".into(),
        refresh_token: None,
        expires_at: None,
        token_url: "https://example.com/token".into(),
        client_id: "c".into(),
        client_secret: None,
        scopes: vec![],
    };
    assert_eq!(oauth2.credential_type(), CredentialType::OAuth2);
}

// T023 additional: Debug impl redacts secrets
#[test]
fn debug_impl_redacts_secrets() {
    let cred = Credential::ApiKey {
        key: "super-secret".into(),
    };
    let debug = format!("{cred:?}");
    assert!(!debug.contains("super-secret"), "Debug leaks secret");
    assert!(debug.contains("[REDACTED]"));

    let resolved = ResolvedCredential::ApiKey("my-secret".into());
    let debug = format!("{resolved:?}");
    assert!(!debug.contains("my-secret"), "Debug leaks secret");
    assert!(debug.contains("[REDACTED]"));
}

// T057/T058: new US4 error variants carry no secrets and surface the key
// for diagnostics, matching the existing CredentialError hygiene pattern.
#[test]
fn authorization_failed_display_and_debug_contain_no_secrets() {
    let err = CredentialError::AuthorizationFailed {
        key: "google-calendar".into(),
        reason: "token endpoint rejected code: HTTP 400 (invalid_grant)".into(),
    };
    let display = format!("{err}");
    let debug = format!("{err:?}");
    assert!(display.contains("google-calendar"));
    assert!(debug.contains("google-calendar"));
    assert!(!display.contains("access_token"));
    assert!(!debug.contains("access_token"));
}

#[test]
fn authorization_timeout_display_and_debug_contain_key() {
    let err = CredentialError::AuthorizationTimeout {
        key: "google-calendar".into(),
    };
    assert!(format!("{err}").contains("google-calendar"));
    assert!(format!("{err:?}").contains("google-calendar"));
}

#[test]
// The clone is the behavior under test, not an accident.
#[allow(clippy::redundant_clone)]
fn authorization_error_clone_preserves_fields() {
    let failed = CredentialError::AuthorizationFailed {
        key: "k".into(),
        reason: "denied".into(),
    };
    match failed.clone() {
        CredentialError::AuthorizationFailed { key, reason } => {
            assert_eq!(key, "k");
            assert_eq!(reason, "denied");
        }
        other => panic!("expected AuthorizationFailed, got {other:?}"),
    }

    let timed_out = CredentialError::AuthorizationTimeout { key: "k".into() };
    match timed_out.clone() {
        CredentialError::AuthorizationTimeout { key } => assert_eq!(key, "k"),
        other => panic!("expected AuthorizationTimeout, got {other:?}"),
    }
}
