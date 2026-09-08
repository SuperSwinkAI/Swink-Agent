//! Tests for `wizard`.
#![cfg(test)]

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::*;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn failing_store_credential(_: &str, _: &str) -> Result<(), String> {
    Err("keychain unavailable".to_string())
}

#[test]
fn initial_state_is_welcome() {
    let wizard = SetupWizard::new_for_test();
    assert!(matches!(wizard.step, WizardStep::Welcome));
}

#[test]
fn enter_on_welcome_goes_to_provider_list() {
    let mut wizard = SetupWizard::new_for_test();
    wizard.handle_key(key(KeyCode::Enter));
    assert!(matches!(wizard.step, WizardStep::ProviderList));
}

#[test]
fn enter_on_provider_list_goes_to_key_entry() {
    let mut wizard = SetupWizard::new_for_test();
    wizard.step = WizardStep::ProviderList;

    // Find a provider that requires a key
    let key_provider_idx = wizard
        .providers
        .iter()
        .position(|p| p.requires_key)
        .expect("should have at least one provider requiring a key");
    wizard.selected = key_provider_idx;

    wizard.handle_key(key(KeyCode::Enter));

    match &wizard.step {
        WizardStep::KeyEntry {
            provider_index,
            input,
            cursor,
        } => {
            assert_eq!(*provider_index, key_provider_idx);
            assert!(input.is_empty());
            assert_eq!(*cursor, 0);
        }
        other => panic!(
            "expected KeyEntry step, got {:?}",
            std::mem::discriminant(other)
        ),
    }
}

#[test]
fn esc_on_welcome_sets_quit() {
    let mut wizard = SetupWizard::new_for_test();
    wizard.handle_key(key(KeyCode::Esc));
    assert!(wizard.should_quit);
}

#[test]
fn esc_on_provider_list_sets_quit() {
    let mut wizard = SetupWizard::new_for_test();
    wizard.step = WizardStep::ProviderList;
    wizard.handle_key(key(KeyCode::Esc));
    assert!(wizard.should_quit);
}

#[test]
fn navigation_clamps_in_provider_list() {
    let mut wizard = SetupWizard::new_for_test();
    wizard.step = WizardStep::ProviderList;
    let max_index = wizard.providers.len(); // includes "Continue" item

    // At top, pressing Up should stay at 0
    wizard.selected = 0;
    wizard.handle_key(key(KeyCode::Up));
    assert_eq!(wizard.selected, 0);

    // At bottom, pressing Down should stay at max
    wizard.selected = max_index;
    wizard.handle_key(key(KeyCode::Down));
    assert_eq!(wizard.selected, max_index);
}

#[test]
fn navigation_moves_up_and_down() {
    let mut wizard = SetupWizard::new_for_test();
    wizard.step = WizardStep::ProviderList;
    wizard.selected = 1;

    wizard.handle_key(key(KeyCode::Down));
    assert_eq!(wizard.selected, 2);

    wizard.handle_key(key(KeyCode::Up));
    assert_eq!(wizard.selected, 1);
}

#[test]
fn key_entry_accepts_input() {
    let mut wizard = SetupWizard::new_for_test();
    wizard.step = WizardStep::KeyEntry {
        provider_index: 1,
        input: String::new(),
        cursor: 0,
    };

    wizard.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    wizard.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
    wizard.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));

    match &wizard.step {
        WizardStep::KeyEntry { input, cursor, .. } => {
            assert_eq!(input, "abc");
            assert_eq!(*cursor, 3);
        }
        _ => panic!("should still be in KeyEntry"),
    }
}

#[test]
fn backspace_in_key_entry_removes_char() {
    let mut wizard = SetupWizard::new_for_test();
    wizard.step = WizardStep::KeyEntry {
        provider_index: 1,
        input: "abc".to_string(),
        cursor: 3,
    };

    wizard.handle_key(key(KeyCode::Backspace));

    match &wizard.step {
        WizardStep::KeyEntry { input, cursor, .. } => {
            assert_eq!(input, "ab");
            assert_eq!(*cursor, 2);
        }
        _ => panic!("should still be in KeyEntry"),
    }
}

#[test]
fn backspace_at_start_is_noop() {
    let mut wizard = SetupWizard::new_for_test();
    wizard.step = WizardStep::KeyEntry {
        provider_index: 1,
        input: "abc".to_string(),
        cursor: 0,
    };

    wizard.handle_key(key(KeyCode::Backspace));

    match &wizard.step {
        WizardStep::KeyEntry { input, cursor, .. } => {
            assert_eq!(input, "abc");
            assert_eq!(*cursor, 0);
        }
        _ => panic!("should still be in KeyEntry"),
    }
}

#[test]
fn esc_in_key_entry_returns_to_provider_list() {
    let mut wizard = SetupWizard::new_for_test();
    wizard.step = WizardStep::KeyEntry {
        provider_index: 1,
        input: "some-key".to_string(),
        cursor: 8,
    };

    wizard.handle_key(key(KeyCode::Esc));

    assert!(matches!(wizard.step, WizardStep::ProviderList));
}

/// Regression for #1111: `new_for_test` must default to a fake writer.
///
/// The old default was `credentials::store_credential`, so this exact
/// sequence — the one most key-entry tests perform — issued a live keychain
/// write and blocked on macOS's SecurityAgent prompt. Asserting on the
/// recording fake proves the write was intercepted: a real write would
/// leave `take_recorded_writes()` empty.
#[test]
fn new_for_test_stores_keys_through_a_fake_not_the_real_keychain() {
    let _ = take_recorded_writes(); // isolate from earlier tests on this thread
    let mut wizard = SetupWizard::new_for_test();
    let provider_index = wizard
        .providers
        .iter()
        .position(|provider| provider.key_name == "openai")
        .expect("openai provider should exist");
    wizard.step = WizardStep::KeyEntry {
        provider_index,
        input: "sk-wizard-sentinel-1111".to_string(),
        cursor: 22,
    };

    wizard.handle_key(key(KeyCode::Enter));

    assert_eq!(
        take_recorded_writes(),
        vec![("openai".to_string(), "sk-wizard-sentinel-1111".to_string())],
        "the wizard's default writer must be the recording fake; an empty log \
             means the key went to the real keychain instead (issue #1111)"
    );
    assert!(
        wizard.configured[provider_index],
        "a successful fake store should still mark the provider configured"
    );
    assert!(matches!(wizard.step, WizardStep::ProviderList));
}

#[test]
fn failed_key_storage_keeps_user_on_entry_screen_with_env_fallback() {
    let mut wizard = SetupWizard::new_for_test();
    let provider_index = wizard
        .providers
        .iter()
        .position(|provider| provider.key_name == "openai")
        .expect("openai provider should exist");
    wizard.store_credential_fn = failing_store_credential;
    wizard.step = WizardStep::KeyEntry {
        provider_index,
        input: "sk-test".to_string(),
        cursor: 7,
    };

    wizard.handle_key(key(KeyCode::Enter));

    match &wizard.step {
        WizardStep::KeyEntry { input, cursor, .. } => {
            assert_eq!(input, "sk-test");
            assert_eq!(*cursor, 7);
        }
        _ => panic!("should remain in KeyEntry after a store failure"),
    }
    assert!(!wizard.configured[provider_index]);
    let save_error = wizard
        .save_error
        .as_deref()
        .expect("save error should be recorded");
    assert!(save_error.contains("keychain unavailable"));
    assert!(save_error.contains("OPENAI_API_KEY"));
}

#[test]
fn continue_option_goes_to_done() {
    let mut wizard = SetupWizard::new_for_test();
    wizard.step = WizardStep::ProviderList;
    wizard.selected = wizard.providers.len(); // "Continue" item

    wizard.handle_key(key(KeyCode::Enter));

    assert!(matches!(wizard.step, WizardStep::Done));
}

#[test]
fn s_key_skips_to_done() {
    let mut wizard = SetupWizard::new_for_test();
    wizard.step = WizardStep::ProviderList;

    wizard.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));

    assert!(matches!(wizard.step, WizardStep::Done));
}

#[test]
fn enter_on_done_sets_continue() {
    let mut wizard = SetupWizard::new_for_test();
    wizard.step = WizardStep::Done;

    wizard.handle_key(key(KeyCode::Enter));

    assert!(wizard.should_continue);
    assert!(!wizard.should_quit);
}

#[test]
fn esc_on_done_sets_quit() {
    let mut wizard = SetupWizard::new_for_test();
    wizard.step = WizardStep::Done;

    wizard.handle_key(key(KeyCode::Esc));

    assert!(wizard.should_quit);
    assert!(!wizard.should_continue);
}

#[test]
fn shift_char_in_key_entry_inserts() {
    let mut wizard = SetupWizard::new_for_test();
    wizard.step = WizardStep::KeyEntry {
        provider_index: 1,
        input: String::new(),
        cursor: 0,
    };

    wizard.handle_key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT));

    match &wizard.step {
        WizardStep::KeyEntry { input, cursor, .. } => {
            assert_eq!(input, "A");
            assert_eq!(*cursor, 1);
        }
        _ => panic!("should still be in KeyEntry"),
    }
}

#[test]
fn q_on_welcome_sets_quit() {
    let mut wizard = SetupWizard::new_for_test();
    wizard.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert!(wizard.should_quit);
}

#[test]
fn enter_on_no_key_provider_is_noop() {
    let mut wizard = SetupWizard::new_for_test();
    wizard.step = WizardStep::ProviderList;

    // Find Ollama (no key required)
    let ollama_idx = wizard
        .providers
        .iter()
        .position(|p| !p.requires_key)
        .expect("should have a no-key provider");
    wizard.selected = ollama_idx;

    wizard.handle_key(key(KeyCode::Enter));

    // Should remain on ProviderList since Ollama doesn't need a key
    assert!(matches!(wizard.step, WizardStep::ProviderList));
}
