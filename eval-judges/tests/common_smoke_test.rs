mod common;

use std::time::Duration;

use common::{
    delayed_verdict_response, malformed_response, provider_error_response, rate_limited_response,
    verdict_response,
};

#[test]
fn shared_judge_fixture_builders_construct_response_templates() {
    let _ = verdict_response(0.8, true, "accepted", Some("equivalent"));
    let _ = delayed_verdict_response(0.2, false, "timed out", None, Duration::from_millis(10));
    let _ = malformed_response(r#"{"score":"bad"}"#);
    let _ = provider_error_response(503, "temporarily unavailable");
    let _ = rate_limited_response(1);
}
