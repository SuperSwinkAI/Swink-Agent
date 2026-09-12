//! Tests for `oai_transport`.
#![cfg(test)]

use super::*;

#[test]
fn oai_body_context_length_exceeded_code_is_context_overflow() {
    let body = r#"{"error":{"message":"This model's maximum context length is 128000 tokens. However, your messages resulted in 131000 tokens.","type":"invalid_request_error","param":"messages","code":"context_length_exceeded"}}"#;
    let event = classify_oai_error_body(400, body, "OpenAI").expect("expected classification");
    match event {
        AssistantMessageEvent::Error { error_kind, .. } => {
            assert_eq!(
                error_kind,
                Some(swink_agent::StreamErrorKind::ContextWindowExceeded)
            );
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn mistral_top_level_body_too_large_is_context_overflow() {
    let body = r#"{"object":"error","message":"Prompt contains 40960 tokens, too large for model with 32768 maximum context length","type":"invalid_request_error","param":null,"code":null}"#;
    let event = classify_oai_error_body(400, body, "Mistral").expect("expected classification");
    match event {
        AssistantMessageEvent::Error { error_kind, .. } => {
            assert_eq!(
                error_kind,
                Some(swink_agent::StreamErrorKind::ContextWindowExceeded)
            );
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn xai_string_error_body_prompt_length_is_context_overflow() {
    let body = r#"{"code":"Client specified an invalid argument","error":"This model's maximum prompt length is 131072 but the request contains 200000 tokens."}"#;
    let event = classify_oai_error_body(400, body, "xAI").expect("expected classification");
    match event {
        AssistantMessageEvent::Error { error_kind, .. } => {
            assert_eq!(
                error_kind,
                Some(swink_agent::StreamErrorKind::ContextWindowExceeded)
            );
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn oai_body_content_filter_code_is_content_filtered() {
    let body = r#"{"error":{"message":"The response was filtered due to the prompt triggering content management policy.","type":null,"code":"content_filter"}}"#;
    let event = classify_oai_error_body(400, body, "Azure").expect("expected classification");
    match event {
        AssistantMessageEvent::Error { error_kind, .. } => {
            assert_eq!(
                error_kind,
                Some(swink_agent::StreamErrorKind::ContentFiltered)
            );
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn oai_body_classification_only_applies_to_4xx() {
    let body = r#"{"error":{"message":"maximum context length exceeded","code":"context_length_exceeded"}}"#;
    assert!(classify_oai_error_body(500, body, "OpenAI").is_none());
}

#[test]
fn unrecognized_oai_body_falls_through() {
    assert!(classify_oai_error_body(400, "not json", "OpenAI").is_none());
    assert!(classify_oai_error_body(400, r#"{"error":{"message":"nope"}}"#, "OpenAI").is_none());
}

#[test]
fn oai_body_invalid_api_key_is_auth() {
    let event = classify_oai_error_body(
        400,
        r#"{"error":{"message":"invalid api key","code":"invalid_api_key"}}"#,
        "OpenAI",
    )
    .expect("expected classification");
    match event {
        AssistantMessageEvent::Error { error_kind, .. } => {
            assert_eq!(error_kind, Some(swink_agent::StreamErrorKind::Auth));
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn custom_chat_path_is_used() {
    let shell = OaiAdapterShell::new_with_path(
        "Azure",
        "https://example.openai.azure.com/openai/deployments/gpt-4/",
        "",
        "/chat/completions",
    );

    assert_eq!(
        shell.chat_completions_url(),
        "https://example.openai.azure.com/openai/deployments/gpt-4/chat/completions"
    );
}
