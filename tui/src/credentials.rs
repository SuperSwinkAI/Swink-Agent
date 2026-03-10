//! Cross-platform credential management using native OS keychains.
//!
//! - macOS: Keychain Services
//! - Windows: Credential Manager
//! - Linux: secret-service (D-Bus)

use std::collections::HashMap;

const SERVICE_NAME: &str = "agent-harness";

/// Known provider configurations.
#[derive(Debug, Clone)]
pub struct ProviderInfo {
    pub name: &'static str,
    pub key_name: &'static str,
    pub env_var: &'static str,
    pub description: &'static str,
    pub requires_key: bool,
}

/// All supported providers.
pub fn providers() -> Vec<ProviderInfo> {
    vec![
        ProviderInfo {
            name: "Ollama",
            key_name: "ollama",
            env_var: "OLLAMA_HOST",
            description: "Local Ollama instance (no API key needed)",
            requires_key: false,
        },
        ProviderInfo {
            name: "OpenAI",
            key_name: "openai",
            env_var: "OPENAI_API_KEY",
            description: "OpenAI API (GPT-4, etc.)",
            requires_key: true,
        },
        ProviderInfo {
            name: "Anthropic",
            key_name: "anthropic",
            env_var: "ANTHROPIC_API_KEY",
            description: "Anthropic API (Claude models)",
            requires_key: true,
        },
        ProviderInfo {
            name: "Custom Proxy",
            key_name: "proxy",
            env_var: "LLM_API_KEY",
            description: "Custom SSE proxy endpoint",
            requires_key: true,
        },
    ]
}

/// Store a credential in the native keychain.
pub fn store_credential(provider_key: &str, secret: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE_NAME, provider_key)
        .map_err(|e| format!("keyring init error: {e}"))?;
    entry
        .set_password(secret)
        .map_err(|e| format!("keyring store error: {e}"))
}

/// Retrieve a credential from the native keychain.
/// Falls back to the environment variable if keychain lookup fails.
pub fn get_credential(provider: &ProviderInfo) -> Option<String> {
    // Check env var first (explicit env override always wins)
    if let Ok(val) = std::env::var(provider.env_var) {
        if !val.is_empty() {
            return Some(val);
        }
    }

    // Try keychain
    let entry = keyring::Entry::new(SERVICE_NAME, provider.key_name).ok()?;
    entry.get_password().ok()
}

/// Delete a credential from the native keychain.
///
/// Reserved for future use by credential management UI (e.g. key rotation).
#[allow(dead_code)]
pub fn delete_credential(provider_key: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE_NAME, provider_key)
        .map_err(|e| format!("keyring init error: {e}"))?;
    entry
        .delete_credential()
        .map_err(|e| format!("keyring delete error: {e}"))
}

/// Check which providers have credentials configured.
pub fn check_credentials() -> HashMap<String, bool> {
    let mut result = HashMap::new();
    for p in providers() {
        let has_cred = if p.requires_key {
            get_credential(&p).is_some()
        } else {
            true // Ollama doesn't need a key
        };
        result.insert(p.key_name.to_string(), has_cred);
    }
    result
}

/// Returns true if at least one provider that requires a key has credentials.
pub fn any_key_configured() -> bool {
    providers()
        .iter()
        .filter(|p| p.requires_key)
        .any(|p| get_credential(p).is_some())
}
