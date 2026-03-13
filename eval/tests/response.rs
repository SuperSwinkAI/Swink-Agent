//! Integration tests for response matching evaluator.

mod common;

use std::sync::Arc;

use swink_agent_eval::{Evaluator, ResponseCriteria, ResponseMatcher, Score, Verdict};

use common::{case_with_response, mock_invocation};

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
