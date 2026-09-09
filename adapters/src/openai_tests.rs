//! Tests for `openai`.
#![cfg(test)]

use super::*;

#[test]
fn trailing_slash_stripped_on_both_backends() {
    let responses = OpenAiStreamFn::new("https://api.openai.com/", "key");
    assert_eq!(responses.base_url(), "https://api.openai.com");
    assert!(format!("{responses:?}").contains("responses"));

    let chat = OpenAiStreamFn::new_chat_completions("https://api.openai.com/", "key");
    assert_eq!(chat.base_url(), "https://api.openai.com");
    assert!(format!("{chat:?}").contains("chat_completions"));
}

#[test]
fn debug_names_the_backend_and_redacts_the_key() {
    let rendered = format!(
        "{:?}",
        OpenAiStreamFn::new("https://api.openai.com", "sk-secret")
    );
    assert!(rendered.contains("responses"), "{rendered}");
    assert!(!rendered.contains("sk-secret"), "{rendered}");
    let rendered = format!(
        "{:?}",
        OpenAiStreamFn::new_chat_completions("https://api.openai.com", "sk-secret")
    );
    assert!(rendered.contains("chat_completions"), "{rendered}");
    assert!(!rendered.contains("sk-secret"), "{rendered}");
}

#[test]
fn wire_parses_both_names_and_rejects_the_rest() {
    assert_eq!(OpenAiWire::parse("responses"), Ok(OpenAiWire::Responses));
    assert_eq!(
        OpenAiWire::parse("Chat-Completions "),
        Ok(OpenAiWire::ChatCompletions)
    );
    assert_eq!(
        OpenAiWire::parse("chat_completions"),
        Ok(OpenAiWire::ChatCompletions)
    );
    let err = OpenAiWire::parse("completions").unwrap_err();
    assert_eq!(err.value, "completions");
    assert!(err.to_string().contains("OPENAI_API"), "{err}");
}

#[test]
fn from_env_defaults_to_responses_when_unset() {
    // Read-only: this workspace forbids unsafe code, and set_var is unsafe
    // as of edition 2024, so no test may mutate OPENAI_API. Skip rather
    // than assert if a developer's shell happens to export it.
    if std::env::var_os(OPENAI_API_ENV).is_none() {
        assert_eq!(OpenAiWire::from_env(), Ok(OpenAiWire::Responses));
    }
}

#[test]
fn new_for_wire_selects_the_backend() {
    assert!(OpenAiWire::default() == OpenAiWire::Responses);
    let responses = OpenAiStreamFn::new_for_wire(OpenAiWire::Responses, "u", "k");
    assert!(format!("{responses:?}").contains("responses"));
    let chat = OpenAiStreamFn::new_for_wire(OpenAiWire::ChatCompletions, "u", "k");
    assert!(format!("{chat:?}").contains("chat_completions"));
}

#[test]
fn serving_support_differs_by_backend() {
    assert!(
        OpenAiStreamFn::new("u", "k")
            .supported_serving_options()
            .reasoning_effort
    );
    assert!(
        !OpenAiStreamFn::new_chat_completions("u", "k")
            .supported_serving_options()
            .reasoning_effort
    );
}
