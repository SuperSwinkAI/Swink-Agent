//! Integration tests for response matching evaluator.

mod common;

use std::sync::Arc;

use swink_agent_eval::{Evaluator, ResponseCriteria, ResponseMatcher, Score, Verdict};

use common::{case_with_response, mock_invocation, mock_invocation_with_response};

#[test]
fn exact_match_passes() {
    let case = case_with_response(ResponseCriteria::Exact {
        expected: "hello world".to_string(),
    });
    let invocation = mock_invocation(&[], Some("hello world"), 0.0, 0);
    let result = ResponseMatcher.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Pass);
}

#[test]
fn exact_match_fails() {
    let case = case_with_response(ResponseCriteria::Exact {
        expected: "hello world".to_string(),
    });
    let invocation = mock_invocation(&[], Some("hello"), 0.0, 0);
    let result = ResponseMatcher.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Fail);
}

#[test]
fn contains_match_passes() {
    let case = case_with_response(ResponseCriteria::Contains {
        substring: "world".to_string(),
    });
    let invocation = mock_invocation(&[], Some("hello world"), 0.0, 0);
    let result = ResponseMatcher.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Pass);
}

#[test]
fn regex_match_passes() {
    let case = case_with_response(ResponseCriteria::Regex {
        pattern: r"hello \w+".to_string(),
    });
    let invocation = mock_invocation(&[], Some("hello world"), 0.0, 0);
    let result = ResponseMatcher.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Pass);
}

#[test]
fn custom_scorer() {
    let case = case_with_response(ResponseCriteria::Custom(Arc::new(|text| {
        if text.contains("hello") {
            Score::pass()
        } else {
            Score::fail()
        }
    })));
    let invocation = mock_invocation(&[], Some("hello!"), 0.0, 0);
    let result = ResponseMatcher.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Pass);
}

// ── Spec 023 Acceptance Tests (US4) ──────────────────────────────────────────

/// AS-4.1: Exact match pass and fail.
#[test]
fn us4_exact_match_pass_and_fail() {
    let case = case_with_response(ResponseCriteria::Exact {
        expected: "42".to_string(),
    });

    let pass = mock_invocation_with_response(&[], "42");
    assert_eq!(ResponseMatcher.evaluate(&case, &pass).unwrap().score.verdict(), Verdict::Pass);

    let fail = mock_invocation_with_response(&[], "43");
    assert_eq!(ResponseMatcher.evaluate(&case, &fail).unwrap().score.verdict(), Verdict::Fail);
}

/// AS-4.2: Contains match pass and fail.
#[test]
fn us4_contains_match_pass_and_fail() {
    let case = case_with_response(ResponseCriteria::Contains {
        substring: "success".to_string(),
    });

    let pass = mock_invocation_with_response(&[], "operation success!");
    assert_eq!(ResponseMatcher.evaluate(&case, &pass).unwrap().score.verdict(), Verdict::Pass);

    let fail = mock_invocation_with_response(&[], "operation failed");
    assert_eq!(ResponseMatcher.evaluate(&case, &fail).unwrap().score.verdict(), Verdict::Fail);
}

/// AS-4.3: Regex match pass and fail.
#[test]
fn us4_regex_match_pass_and_fail() {
    let case = case_with_response(ResponseCriteria::Regex {
        pattern: r"\d+ files processed".to_string(),
    });

    let pass = mock_invocation_with_response(&[], "42 files processed");
    assert_eq!(ResponseMatcher.evaluate(&case, &pass).unwrap().score.verdict(), Verdict::Pass);

    let fail = mock_invocation_with_response(&[], "no files");
    assert_eq!(ResponseMatcher.evaluate(&case, &fail).unwrap().score.verdict(), Verdict::Fail);
}

/// AS-4.4: Custom function match pass and fail.
#[test]
fn us4_custom_fn_pass_and_fail() {
    let case = case_with_response(ResponseCriteria::Custom(Arc::new(|text| {
        if text.len() < 100 && text.contains("done") {
            Score::pass()
        } else {
            Score::fail()
        }
    })));

    let pass = mock_invocation_with_response(&[], "task done");
    assert_eq!(ResponseMatcher.evaluate(&case, &pass).unwrap().score.verdict(), Verdict::Pass);

    let fail = mock_invocation_with_response(&[], "task in progress");
    assert_eq!(ResponseMatcher.evaluate(&case, &fail).unwrap().score.verdict(), Verdict::Fail);
}

/// AS-4.5: Custom criterion combining multiple sub-checks.
#[test]
fn us4_custom_composite_criterion() {
    let case = case_with_response(ResponseCriteria::Custom(Arc::new(|text| {
        let has_summary = text.contains("Summary:");
        let has_count = text.contains("total:");
        if has_summary && has_count {
            Score::pass()
        } else {
            Score::fail()
        }
    })));

    let pass = mock_invocation_with_response(&[], "Summary: done. total: 5");
    assert_eq!(ResponseMatcher.evaluate(&case, &pass).unwrap().score.verdict(), Verdict::Pass);

    let fail = mock_invocation_with_response(&[], "Summary: done.");
    assert_eq!(ResponseMatcher.evaluate(&case, &fail).unwrap().score.verdict(), Verdict::Fail);
}

/// Edge case: Invalid regex returns Score::fail with compilation error.
#[test]
fn us4_invalid_regex_fails_with_diagnostic() {
    let case = case_with_response(ResponseCriteria::Regex {
        pattern: "[invalid".to_string(),
    });
    let invocation = mock_invocation_with_response(&[], "anything");
    let result = ResponseMatcher.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Fail);
    let details = result.details.unwrap();
    assert!(details.contains("invalid regex"), "expected regex error, got: {details}");
}

/// Edge case: Custom function panic caught and reported as failure.
#[test]
fn us4_custom_fn_panic_caught() {
    let case = case_with_response(ResponseCriteria::Custom(Arc::new(|_: &str| -> Score {
        panic!("oops");
    })));
    let invocation = mock_invocation_with_response(&[], "anything");
    let result = ResponseMatcher.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Fail);
    let details = result.details.unwrap();
    assert!(details.contains("panicked"), "expected panic in details, got: {details}");
    assert!(details.contains("oops"), "expected panic message, got: {details}");
}

/// Edge case: None final_response falls back to empty string.
#[test]
fn us4_none_response_falls_back_to_empty() {
    let case = case_with_response(ResponseCriteria::Exact {
        expected: String::new(),
    });
    let invocation = mock_invocation(&[], None, 0.0, 0);
    let result = ResponseMatcher.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Pass, "empty expected should match None response");
}
