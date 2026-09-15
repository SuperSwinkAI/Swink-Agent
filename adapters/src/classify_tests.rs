//! Tests for `classify`.
#![cfg(test)]

use super::*;

#[test]
fn context_overflow_matches_documented_provider_wordings() {
    // Anthropic
    assert!(is_context_overflow_message(
        "prompt is too long: 210510 tokens > 200000 maximum"
    ));
    assert!(is_context_overflow_message(
        "input length and `max_tokens` exceed context limit: 199999 + 4096 > 200000"
    ));
    // Bedrock
    assert!(is_context_overflow_message(
        "Input is too long for requested model."
    ));
    assert!(is_context_overflow_message(
        "too many input tokens for this model"
    ));
    // OpenAI / Azure
    assert!(is_context_overflow_message(
        "This model's maximum context length is 128000 tokens. However, your messages resulted in 131000 tokens."
    ));
    assert!(is_context_overflow_message(
        "Your input exceeds the context window of this model."
    ));
    // Google
    assert!(is_context_overflow_message(
        "The input token count (32000) exceeds the maximum number of tokens allowed (30720)."
    ));
    // Mistral
    assert!(is_context_overflow_message(
        "Prompt contains 40960 tokens, too large for model with 32768 maximum context length"
    ));
    // xAI
    assert!(is_context_overflow_message(
        "This model's maximum prompt length is 131072 but the request contains 200000 tokens."
    ));
}

#[test]
fn context_overflow_does_not_match_unrelated_messages() {
    assert!(!is_context_overflow_message("invalid api key"));
    assert!(!is_context_overflow_message(
        "max_tokens: must be greater than 0"
    ));
    assert!(!is_context_overflow_message("rate limit exceeded"));
    assert!(!is_context_overflow_message(""));
}

#[test]
fn classify_401_is_auth() {
    assert_eq!(classify_http_status(401), Some(HttpErrorKind::Auth));
}

#[test]
fn classify_403_is_auth() {
    assert_eq!(classify_http_status(403), Some(HttpErrorKind::Auth));
}

#[test]
fn classify_429_is_throttled() {
    assert_eq!(classify_http_status(429), Some(HttpErrorKind::Throttled));
}

#[test]
fn zero_rate_limit_allowance_detects_limit_headers_only() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("x-ratelimit-limit-req-minute", "0".parse().unwrap());

    assert!(has_zero_rate_limit_allowance(&headers));
}

#[test]
fn zero_rate_limit_allowance_ignores_remaining_headers() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("x-ratelimit-remaining-requests", "0".parse().unwrap());
    headers.insert("retry-after", "0".parse().unwrap());

    assert!(!has_zero_rate_limit_allowance(&headers));
}

#[test]
fn zero_rate_limit_allowance_ignores_nonzero_or_unparseable_limits() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("x-ratelimit-limit-requests", "125".parse().unwrap());
    assert!(!has_zero_rate_limit_allowance(&headers));

    headers.insert("ratelimit-limit", "0;w=60".parse().unwrap());
    assert!(!has_zero_rate_limit_allowance(&headers));
}

#[test]
fn classify_408_is_network() {
    assert_eq!(classify_http_status(408), Some(HttpErrorKind::Network));
}

#[test]
fn classify_500_is_network() {
    assert_eq!(classify_http_status(500), Some(HttpErrorKind::Network));
}

#[test]
fn classify_200_is_none() {
    assert_eq!(classify_http_status(200), None);
}

#[test]
fn classify_with_overrides_applies_first() {
    // Override 429 to be Auth instead of Throttled
    let overrides = vec![(429, HttpErrorKind::Auth)];
    assert_eq!(
        classify_with_overrides(429, &overrides),
        Some(HttpErrorKind::Auth),
    );

    // Non-overridden codes still use defaults
    assert_eq!(
        classify_with_overrides(500, &overrides),
        Some(HttpErrorKind::Network),
    );
}

#[test]
fn error_event_401_is_auth() {
    let event = error_event_from_status(401, "bad key", "TestProvider");
    match event {
        AssistantMessageEvent::Error {
            error_message,
            error_kind,
            ..
        } => {
            assert!(error_message.contains("TestProvider"));
            assert!(error_message.contains("401"));
            assert!(error_message.contains("bad key"));
            assert_eq!(error_kind, Some(swink_agent::StreamErrorKind::Auth));
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn error_event_429_is_throttled() {
    let event = error_event_from_status(429, "slow down", "TestProvider");
    match event {
        AssistantMessageEvent::Error {
            error_kind,
            error_message,
            ..
        } => {
            assert!(error_message.contains("429"));
            assert_eq!(error_kind, Some(swink_agent::StreamErrorKind::Throttled));
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn error_event_500_is_network() {
    let event = error_event_from_status(500, "internal", "TestProvider");
    match event {
        AssistantMessageEvent::Error {
            error_kind,
            error_message,
            ..
        } => {
            assert!(error_message.contains("500"));
            assert_eq!(error_kind, Some(swink_agent::StreamErrorKind::Network));
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn error_event_408_is_network() {
    let event = error_event_from_status(408, "timeout", "TestProvider");
    match event {
        AssistantMessageEvent::Error {
            error_kind,
            error_message,
            ..
        } => {
            assert!(error_message.contains("408"));
            assert_eq!(error_kind, Some(swink_agent::StreamErrorKind::Network));
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn error_event_400_is_generic_client_error() {
    let event = error_event_from_status(400, "bad request", "TestProvider");
    match event {
        AssistantMessageEvent::Error {
            error_kind,
            error_message,
            ..
        } => {
            assert!(error_message.contains("client error"));
            assert!(error_message.contains("400"));
            assert_eq!(error_kind, None);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn parse_retry_after_value_seconds_form() {
    assert_eq!(parse_retry_after_value("30"), Some(Duration::from_secs(30)));
}

#[test]
fn parse_retry_after_value_zero_seconds() {
    assert_eq!(parse_retry_after_value("0"), Some(Duration::ZERO));
}

#[test]
fn parse_retry_after_value_ignores_surrounding_whitespace() {
    assert_eq!(
        parse_retry_after_value("  30  "),
        Some(Duration::from_secs(30))
    );
}

#[test]
fn parse_retry_after_value_http_date_in_future() {
    let future = chrono::Utc::now() + chrono::Duration::seconds(120);
    let header = future.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
    let parsed = parse_retry_after_value(&header).expect("should parse HTTP-date form");
    // Allow a little slack for test execution time between formatting
    // the fixture and parsing it.
    assert!(
        (110..=120).contains(&parsed.as_secs()),
        "expected ~120s, got {parsed:?}"
    );
}

#[test]
fn parse_retry_after_value_http_date_in_past_clamps_to_zero() {
    let past = chrono::Utc::now() - chrono::Duration::seconds(60);
    let header = past.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
    assert_eq!(parse_retry_after_value(&header), Some(Duration::ZERO));
}

#[test]
fn parse_retry_after_value_garbage_is_none() {
    assert_eq!(parse_retry_after_value("not-a-valid-value"), None);
}

#[test]
fn parse_retry_after_value_empty_is_none() {
    assert_eq!(parse_retry_after_value(""), None);
}

#[test]
fn with_retry_after_sets_field_on_error_event() {
    let event = AssistantMessageEvent::error_throttled("slow down");
    let event = with_retry_after(event, Some(Duration::from_secs(5)));
    match event {
        AssistantMessageEvent::Error { retry_after, .. } => {
            assert_eq!(retry_after, Some(Duration::from_secs(5)));
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn with_retry_after_none_is_none() {
    let event = AssistantMessageEvent::error_throttled("slow down");
    let event = with_retry_after(event, None);
    match event {
        AssistantMessageEvent::Error { retry_after, .. } => {
            assert_eq!(retry_after, None);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn with_retry_after_is_noop_for_non_error_events() {
    let event = AssistantMessageEvent::Start;
    let event = with_retry_after(event, Some(Duration::from_secs(5)));
    assert!(matches!(event, AssistantMessageEvent::Start));
}

#[test]
fn error_event_with_override_529_network() {
    let event = error_event_from_status_with_overrides(
        529,
        "overloaded",
        "Anthropic",
        &[(529, HttpErrorKind::Network)],
    );
    match event {
        AssistantMessageEvent::Error {
            error_kind,
            error_message,
            ..
        } => {
            assert!(error_message.contains("Anthropic"));
            assert!(error_message.contains("529"));
            assert_eq!(error_kind, Some(swink_agent::StreamErrorKind::Network));
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

// ─── Model retirement classification ─────────────────────────────────

#[test]
fn openai_model_not_found_code_is_model_retired() {
    // OpenAI reports retired and unknown models with HTTP 404 and the
    // `model_not_found` error code.
    let body = r#"{"error":{"message":"The model `gpt-4-32k` has been deprecated","type":"invalid_request_error","code":"model_not_found"}}"#;
    assert!(is_model_retired_response(404, body));
    let event = error_event_from_status(404, body, "OpenAI");
    match event {
        AssistantMessageEvent::Error {
            error_kind,
            error_message,
            ..
        } => {
            assert_eq!(error_kind, Some(swink_agent::StreamErrorKind::ModelRetired));
            assert!(error_message.contains("OpenAI"));
            assert!(error_message.contains("404"));
            assert!(error_message.contains("gpt-4-32k"));
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn decommissioned_model_410_is_model_retired() {
    let body = "The model claude-1 has been decommissioned";
    assert!(is_model_retired_response(410, body));
    let event = error_event_from_status(410, body, "Anthropic");
    match event {
        AssistantMessageEvent::Error { error_kind, .. } => {
            assert_eq!(error_kind, Some(swink_agent::StreamErrorKind::ModelRetired));
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn gemini_model_not_found_404_is_model_retired() {
    let body = "models/gemini-1.0-pro is not found for API version v1beta";
    assert!(is_model_retired_response(404, body));
}

#[test]
fn generic_400_body_is_not_model_retired() {
    assert!(!is_model_retired_response(
        400,
        "missing required field: messages"
    ));
    let event = error_event_from_status(400, "bad request", "TestProvider");
    match event {
        AssistantMessageEvent::Error { error_kind, .. } => {
            assert_eq!(error_kind, None, "generic 400 must stay unclassified");
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn model_retirement_wording_on_wrong_status_is_not_model_retired() {
    // Classified statuses (auth/throttle/server) win over body heuristics.
    let body = r#"{"error":{"code":"model_not_found"}}"#;
    assert!(!is_model_retired_response(500, body));
    assert!(!is_model_retired_response(401, body));
    let event = error_event_from_status(401, body, "TestProvider");
    match event {
        AssistantMessageEvent::Error { error_kind, .. } => {
            assert_eq!(error_kind, Some(swink_agent::StreamErrorKind::Auth));
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

// ─── Cross-adapter error classification ──────────────────────────────

/// Verify that all adapter-emitted error events carry a `StreamErrorKind`
/// for the common error patterns (network, content filter, throttle).
#[test]
fn cross_adapter_unexpected_eof_is_network() {
    // All adapters should emit Network kind for unexpected stream EOF
    let providers = ["Anthropic", "OpenAI", "Google", "Ollama", "Bedrock"];
    for provider in providers {
        let event =
            AssistantMessageEvent::error_network(format!("{provider} stream ended unexpectedly"));
        match event {
            AssistantMessageEvent::Error { error_kind, .. } => {
                assert_eq!(
                    error_kind,
                    Some(swink_agent::StreamErrorKind::Network),
                    "{provider} unexpected EOF should have Network kind"
                );
            }
            other => panic!("expected Error for {provider}, got {other:?}"),
        }
    }
}

#[test]
fn cross_adapter_content_filter_is_classified() {
    let event = AssistantMessageEvent::error_content_filtered("response blocked by safety filter");
    match event {
        AssistantMessageEvent::Error { error_kind, .. } => {
            assert_eq!(
                error_kind,
                Some(swink_agent::StreamErrorKind::ContentFiltered),
            );
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn http_error_classification_covers_all_adapter_status_codes() {
    // Auth errors
    for code in [401, 403] {
        let event = error_event_from_status(code, "forbidden", "TestAdapter");
        match event {
            AssistantMessageEvent::Error { error_kind, .. } => {
                assert_eq!(
                    error_kind,
                    Some(swink_agent::StreamErrorKind::Auth),
                    "HTTP {code} should be Auth"
                );
            }
            other => panic!("expected Error for HTTP {code}, got {other:?}"),
        }
    }

    // Throttle
    let event = error_event_from_status(429, "too many requests", "TestAdapter");
    match event {
        AssistantMessageEvent::Error { error_kind, .. } => {
            assert_eq!(error_kind, Some(swink_agent::StreamErrorKind::Throttled));
        }
        other => panic!("expected Error, got {other:?}"),
    }

    // Request timeout and server errors
    for code in [408, 500, 502, 503, 529] {
        let event = error_event_from_status_with_overrides(
            code,
            "server error",
            "TestAdapter",
            &[(529, HttpErrorKind::Network)],
        );
        match event {
            AssistantMessageEvent::Error { error_kind, .. } => {
                assert_eq!(
                    error_kind,
                    Some(swink_agent::StreamErrorKind::Network),
                    "HTTP {code} should be Network"
                );
            }
            other => panic!("expected Error for HTTP {code}, got {other:?}"),
        }
    }
}
