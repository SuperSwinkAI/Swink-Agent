//! Cross-platform credential management using native OS keychains.
//!
//! - macOS: Keychain Services
//! - Windows: Credential Manager
//! - Linux: secret-service (D-Bus)
//!
//! This stores **LLM provider API keys** (Ollama/OpenAI/Anthropic/Custom Proxy) for the
//! TUI itself. It is unrelated to `swink-agent-auth`'s `CredentialStore` (spec
//! `035-credential-management`), which stores *tool* authentication secrets. See
//! `specs/035-credential-management/spec.md`'s terminology note for the distinction.
//!
//! `swink-agent-auth` also has a keychain-backed `CredentialStore` behind its
//! `keychain` feature, so both now use the `keyring` crate. They are still
//! separate systems: this module writes under the `swink-agent` service name,
//! that one under `swink-agent-auth`, and neither reads the other's entries.
//!
//! # Test isolation
//!
//! Every keychain operation goes through the `KeychainBackend` seam, mirroring
//! `swink-agent-auth`'s `keychain` module. The real backend, `SystemKeychain`,
//! is the only place `keyring::Entry` is ever constructed, and it is compiled
//! `#[cfg(not(test))]`. Under `cfg(test)` it does not exist, so a unit test
//! *cannot* reach the OS keychain even by calling these functions directly — the
//! guarantee is enforced by the compiler rather than by convention. Tests
//! transparently get `FakeKeychain`, a thread-local in-memory map.
//!
//! This closes issue #1111, where `wizard::SetupWizard::new_for_test` advertised
//! that it "bypasses keychain lookups" but wired the real writer, and `#key`
//! submissions through `App` blocked forever on macOS's SecurityAgent prompt.
//!
//! ## Scope of the guarantee
//!
//! `cfg(test)` is set only while compiling *this crate's own* unit tests, which
//! is where every current test lives (and where both #1111 hangs were). It is
//! **not** set for the `tui/tests/*` integration tests or for `src/main.rs`'s
//! test module: those link the library compiled normally, so they resolve
//! `SystemKeychain` and would reach a real keychain if they called these
//! functions. None do today — `ac_tui.rs` only calls [`providers`], which is
//! pure, and `main.rs`'s `try_proxy` test short-circuits on an unset
//! `LLM_BASE_URL` before any credential lookup. Anything added there that
//! stores or reads credentials must inject its own double; prefer keeping such
//! tests in this crate's unit-test tree, where the seam applies automatically.

use std::collections::HashMap;

const SERVICE_NAME: &str = "swink-agent";

// ─── KeychainBackend ────────────────────────────────────────────────────────

/// Seam over the native secret store.
///
/// Implementors move opaque strings in and out of storage; this module owns the
/// provider/env-var policy on top. The production implementation is
/// `SystemKeychain`; tests get `FakeKeychain` instead. See the module docs
/// for why the selection is a `cfg` rather than a constructor argument.
trait KeychainBackend {
    /// Read the secret stored for `service`/`account`, or `None` if absent.
    fn get(&self, service: &str, account: &str) -> Option<String>;

    /// Write `secret` for `service`/`account`, replacing any existing value.
    fn set(&self, service: &str, account: &str, secret: &str) -> Result<(), String>;

    /// Remove the entry for `service`/`account`.
    fn delete(&self, service: &str, account: &str) -> Result<(), String>;
}

// ─── SystemKeychain (production only) ───────────────────────────────────────

/// `KeychainBackend` backed by the real OS keychain via `keyring`.
///
/// Deliberately **not** compiled under `cfg(test)`: this is the sole
/// construction site of `keyring::Entry` in this crate, so excluding it from
/// test builds makes "no test touches the real keychain" a compile-time fact.
#[cfg(not(test))]
struct SystemKeychain;

#[cfg(not(test))]
impl KeychainBackend for SystemKeychain {
    fn get(&self, service: &str, account: &str) -> Option<String> {
        keyring::Entry::new(service, account)
            .ok()?
            .get_password()
            .ok()
    }

    fn set(&self, service: &str, account: &str, secret: &str) -> Result<(), String> {
        keyring::Entry::new(service, account)
            .map_err(|e| format!("keyring init error: {e}"))?
            .set_password(secret)
            .map_err(|e| format!("keyring store error: {e}"))
    }

    fn delete(&self, service: &str, account: &str) -> Result<(), String> {
        keyring::Entry::new(service, account)
            .map_err(|e| format!("keyring init error: {e}"))?
            .delete_credential()
            .map_err(|e| format!("keyring delete error: {e}"))
    }
}

// ─── FakeKeychain (test only) ───────────────────────────────────────────────

/// In-memory `KeychainBackend` used for every `cfg(test)` build.
///
/// Storage is thread-local so tests running in parallel cannot observe each
/// other's writes — `cargo test` gives each test its own thread, and the
/// `#[tokio::test]` default (`current_thread`) keeps the runtime on it.
#[cfg(test)]
struct FakeKeychain;

#[cfg(test)]
thread_local! {
    static FAKE_KEYCHAIN: std::cell::RefCell<HashMap<(String, String), String>> =
        std::cell::RefCell::new(HashMap::new());
}

#[cfg(test)]
impl KeychainBackend for FakeKeychain {
    fn get(&self, service: &str, account: &str) -> Option<String> {
        FAKE_KEYCHAIN.with_borrow(|m| m.get(&(service.to_string(), account.to_string())).cloned())
    }

    fn set(&self, service: &str, account: &str, secret: &str) -> Result<(), String> {
        FAKE_KEYCHAIN.with_borrow_mut(|m| {
            m.insert(
                (service.to_string(), account.to_string()),
                secret.to_string(),
            );
        });
        Ok(())
    }

    fn delete(&self, service: &str, account: &str) -> Result<(), String> {
        FAKE_KEYCHAIN.with_borrow_mut(|m| {
            m.remove(&(service.to_string(), account.to_string()));
        });
        Ok(())
    }
}

/// Clear the thread-local fake keychain. Test hygiene helper.
#[cfg(test)]
fn reset_fake_keychain() {
    FAKE_KEYCHAIN.with_borrow_mut(HashMap::clear);
}

// ─── Backend selection ──────────────────────────────────────────────────────

/// Run `f` against the active backend: `SystemKeychain` in production builds,
/// `FakeKeychain` under `cfg(test)`.
fn with_backend<R>(f: impl FnOnce(&dyn KeychainBackend) -> R) -> R {
    #[cfg(test)]
    {
        f(&FakeKeychain)
    }
    #[cfg(not(test))]
    {
        f(&SystemKeychain)
    }
}

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
///
/// Under `cfg(test)` this writes to an in-memory fake; see the module docs.
pub fn store_credential(provider_key: &str, secret: &str) -> Result<(), String> {
    with_backend(|b| b.set(SERVICE_NAME, provider_key, secret))
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

    with_backend(|b| b.get(SERVICE_NAME, provider.key_name))
}

/// Delete a credential from the native keychain.
///
/// Reserved for future use by credential management UI (e.g. key rotation).
#[allow(dead_code)]
pub fn delete_credential(provider_key: &str) -> Result<(), String> {
    with_backend(|b| b.delete(SERVICE_NAME, provider_key))
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

    /// A provider whose env var is guaranteed unset, so `credential()` falls
    /// through to the backend instead of short-circuiting on the environment.
    /// (`OPENAI_API_KEY` and friends are often set on developer machines.)
    fn test_provider() -> ProviderInfo {
        ProviderInfo {
            name: "Test Provider",
            key_name: "test-provider-1111",
            env_var: "SWINK_TEST_ENV_VAR_DEFINITELY_UNSET_1111",
            description: "isolation test fixture",
            requires_key: true,
        }
    }

    /// Regression for #1111: the backend under `cfg(test)` must be the
    /// in-memory fake, never the OS keychain.
    ///
    /// This asserts on a property that *discriminates* the two backends rather
    /// than merely passing: the fake is thread-local, while a real OS keychain
    /// is process- and system-wide. A value stored on this thread must be
    /// invisible from another thread. If someone ever rewires these functions
    /// to `keyring` again, the spawned thread would observe the stored secret
    /// and this test fails.
    ///
    /// The primary guarantee is stronger still and lives in the type system:
    /// `SystemKeychain` — the sole construction site of `keyring::Entry` — is
    /// `#[cfg(not(test))]`, so it is not compiled into the test binary at all.
    #[test]
    fn test_builds_cannot_reach_the_real_keychain() {
        reset_fake_keychain();
        let provider = test_provider();
        let sentinel = "sk-isolation-sentinel-1111";

        store_credential(provider.key_name, sentinel).expect("fake store should succeed");
        assert_eq!(
            credential(&provider).as_deref(),
            Some(sentinel),
            "same thread should read its own fake-backed write"
        );

        let seen_on_other_thread = std::thread::spawn(|| credential(&test_provider()))
            .join()
            .unwrap();

        assert_eq!(
            seen_on_other_thread, None,
            "another thread saw the stored secret — the backend is process-wide, \
             which means these functions are hitting the REAL keychain (issue #1111)"
        );
    }

    #[test]
    fn store_then_delete_round_trips_through_the_fake() {
        reset_fake_keychain();
        let provider = test_provider();

        assert_eq!(credential(&provider), None, "starts empty");
        store_credential(provider.key_name, "secret-value").unwrap();
        assert_eq!(credential(&provider).as_deref(), Some("secret-value"));
        delete_credential(provider.key_name).unwrap();
        assert_eq!(credential(&provider), None, "delete removes the entry");
    }

    #[test]
    fn check_credentials_does_not_touch_the_real_keychain() {
        reset_fake_keychain();
        // Exercises `credential()` for every provider. Before #1111 this was a
        // live keychain read per provider; it must now be pure in-memory.
        let status = check_credentials();
        assert_eq!(status.len(), providers().len());
        assert_eq!(
            status.get("ollama"),
            Some(&true),
            "ollama needs no key so it is always configured"
        );
    }

    #[test]
    fn providers_returns_four_entries() {
        let p = providers();
        assert_eq!(p.len(), 4);
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
