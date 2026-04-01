//! Shared test helpers for auth crate tests.

#![allow(dead_code)]

use std::sync::Arc;

use swink_agent::credential::CredentialFuture;
use swink_agent::{AuthorizationHandler, CredentialError};

/// Mock authorization handler that returns a fixed code.
pub struct MockAuthHandler {
    pub code: String,
}

impl MockAuthHandler {
    pub fn new(code: impl Into<String>) -> Arc<Self> {
        Arc::new(Self { code: code.into() })
    }

    pub fn failing(reason: impl Into<String>) -> Arc<FailingAuthHandler> {
        Arc::new(FailingAuthHandler { reason: reason.into() })
    }
}

impl AuthorizationHandler for MockAuthHandler {
    fn authorize(
        &self,
        _auth_url: &str,
        _state: &str,
    ) -> CredentialFuture<'_, String> {
        let code = self.code.clone();
        Box::pin(async move { Ok(code) })
    }
}

/// Authorization handler that always fails.
pub struct FailingAuthHandler {
    pub reason: String,
}

impl AuthorizationHandler for FailingAuthHandler {
    fn authorize(
        &self,
        _auth_url: &str,
        _state: &str,
    ) -> CredentialFuture<'_, String> {
        let reason = self.reason.clone();
        Box::pin(async move {
            Err(CredentialError::AuthorizationFailed {
                key: String::new(),
                reason,
            })
        })
    }
}
