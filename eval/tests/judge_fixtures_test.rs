mod common;

use std::time::Duration;

use common::judge_fixtures::{
    delayed_verdict_response, malformed_response, provider_error_response, rate_limited_response,
    verdict_response,
};

#[test]
fn judge_fixture_builders_construct_response_templates() {
    let _ = verdict_response(0.9, true, "looks good", Some("pass"));
    let _ = delayed_verdict_response(0.1, false, "too slow", None, Duration::from_millis(25));
    let _ = malformed_response("{not json");
    let _ = provider_error_response(500, "upstream failed");
    let _ = rate_limited_response(3);
}
