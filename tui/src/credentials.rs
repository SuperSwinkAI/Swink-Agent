//! Cross-platform credential management using native OS keychains.
//!
//! - macOS: Keychain Services
//! - Windows: Credential Manager
//! - Linux: secret-service (D-Bus)

use std::collections::HashMap;

const SERVICE_NAME: &str = "swink-agent";

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
        ProviderInfo {
            name: "Local (SmolLM3-3B)",
            key_name: "local",
            env_var: "LOCAL_MODEL_REPO",
            description: "Local on-device inference (no API key needed)",
            requires_key: false,
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
pub fn credential(provider: &ProviderInfo) -> Option<String> {
    // Check env var first (explicit env override always wins)
    if let Ok(val) = std::env::var(provider.env_var)
        && !val.is_empty()
    {
        return Some(val);
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
            credential(&p).is_some()
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
        .any(|p| credential(p).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn providers_returns_five_entries() {
        let p = providers();
        assert_eq!(p.len(), 5);
    }

    #[test]
    fn providers_key_names_are_unique() {
        let p = providers();
        let mut names: Vec<&str> = p.iter().map(|info| info.key_name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), p.len(), "provider key_names must be unique");
    }

    #[test]
    fn providers_env_vars_are_unique() {
        let p = providers();
        let mut vars: Vec<&str> = p.iter().map(|info| info.env_var).collect();
        vars.sort_unstable();
        vars.dedup();
        assert_eq!(vars.len(), p.len(), "provider env_vars must be unique");
    }

    #[test]
    fn ollama_does_not_require_key() {
        let p = providers();
        let ollama = p.iter().find(|info| info.key_name == "ollama").unwrap();
        assert!(!ollama.requires_key);
    }

    #[test]
    fn key_requiring_providers() {
        let p = providers();
        let requires_key: Vec<&str> = p
            .iter()
            .filter(|info| info.requires_key)
            .map(|info| info.key_name)
            .collect();
        assert!(requires_key.contains(&"openai"));
        assert!(requires_key.contains(&"anthropic"));
        assert!(requires_key.contains(&"proxy"));
    }

    #[test]
    fn known_provider_key_names() {
        let p = providers();
        let names: Vec<&str> = p.iter().map(|info| info.key_name).collect();
        assert!(names.contains(&"ollama"));
        assert!(names.contains(&"openai"));
        assert!(names.contains(&"anthropic"));
        assert!(names.contains(&"proxy"));
    }
}
