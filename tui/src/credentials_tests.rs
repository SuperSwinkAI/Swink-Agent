//! Tests for `credentials`.
#![cfg(test)]

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
