//! Tests for `domain_filter`.
#![cfg(test)]

use serde_json::json;
use swink_agent::SessionState;

use super::*;

fn make_dispatch_ctx<'a>(
    tool_name: &'a str,
    tool_call_id: &'a str,
    args: &'a mut serde_json::Value,
    state: &'a SessionState,
) -> ToolDispatchContext<'a> {
    ToolDispatchContext::new(tool_name, tool_call_id, args, None, state)
}

#[test]
fn policy_ignores_non_web_tools() {
    let policy = DomainFilterPolicy::new(DomainFilter {
        denylist: vec!["evil.com".to_string()],
        ..Default::default()
    });
    let state = SessionState::default();
    let mut args = json!({"url": "https://evil.com"});
    let mut ctx = make_dispatch_ctx("bash", "call_1", &mut args, &state);
    assert!(matches!(
        policy.evaluate(&mut ctx),
        PreDispatchVerdict::Continue
    ));
}

#[test]
fn policy_continues_without_url_argument() {
    let policy = DomainFilterPolicy::new(DomainFilter::default());
    let state = SessionState::default();
    let mut args = json!({"query": "rust programming"});
    let mut ctx = make_dispatch_ctx("web_search", "call_2", &mut args, &state);
    assert!(matches!(
        policy.evaluate(&mut ctx),
        PreDispatchVerdict::Continue
    ));
}

#[test]
fn policy_blocks_denied_and_invalid_urls() {
    let policy = DomainFilterPolicy::new(DomainFilter {
        denylist: vec!["evil.com".to_string()],
        ..Default::default()
    });
    let state = SessionState::default();

    let mut denied = json!({"url": "https://evil.com/page"});
    let mut denied_ctx = make_dispatch_ctx("web_fetch", "call_3", &mut denied, &state);
    assert!(matches!(
        policy.evaluate(&mut denied_ctx),
        PreDispatchVerdict::Skip(_)
    ));

    let mut bad_scheme = json!({"url": "file:///etc/passwd"});
    let mut bad_scheme_ctx = make_dispatch_ctx("web_fetch", "call_4", &mut bad_scheme, &state);
    assert!(matches!(
        policy.evaluate(&mut bad_scheme_ctx),
        PreDispatchVerdict::Skip(_)
    ));

    let mut invalid = json!({"url": "not a url at all"});
    let mut invalid_ctx = make_dispatch_ctx("web_fetch", "call_5", &mut invalid, &state);
    assert!(matches!(
        policy.evaluate(&mut invalid_ctx),
        PreDispatchVerdict::Skip(_)
    ));
}
