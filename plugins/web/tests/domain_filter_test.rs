use serde_json::json;
use swink_agent::policy::{PolicyContext, PreDispatchPolicy, PreDispatchVerdict, ToolPolicyContext};
use swink_agent::{Cost, SessionState, Usage};
use swink_agent_plugin_web::domain::{DomainFilter, DomainFilterError};
use swink_agent_plugin_web::policy::DomainFilterPolicy;
use url::Url;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_policy_context() -> (Usage, Cost, SessionState) {
    (Usage::default(), Cost::default(), SessionState::default())
}

fn ctx_from<'a>(
    usage: &'a Usage,
    cost: &'a Cost,
    state: &'a SessionState,
) -> PolicyContext<'a> {
    PolicyContext {
        turn_index: 0,
        accumulated_usage: usage,
        accumulated_cost: cost,
        message_count: 0,
        overflow_signal: false,
        new_messages: &[],
        state,
    }
}

// ---------------------------------------------------------------------------
// DomainFilter — scheme checks
// ---------------------------------------------------------------------------

#[test]
fn rejects_file_scheme() {
    let filter = DomainFilter::default();
    let url = Url::parse("file:///etc/passwd").unwrap();
    let err = filter.is_allowed(&url).unwrap_err();
    assert!(matches!(err, DomainFilterError::InvalidScheme(_)));
}

#[test]
fn rejects_ftp_scheme() {
    let filter = DomainFilter::default();
    let url = Url::parse("ftp://example.com/pub").unwrap();
    let err = filter.is_allowed(&url).unwrap_err();
    assert!(matches!(err, DomainFilterError::InvalidScheme(_)));
}

#[test]
fn allows_http_scheme() {
    let filter = DomainFilter::default();
    let url = Url::parse("http://example.com").unwrap();
    assert!(filter.is_allowed(&url).is_ok());
}

#[test]
fn allows_https_scheme() {
    let filter = DomainFilter::default();
    let url = Url::parse("https://example.com").unwrap();
    assert!(filter.is_allowed(&url).is_ok());
}

// ---------------------------------------------------------------------------
// DomainFilter — allowlist
// ---------------------------------------------------------------------------

#[test]
fn allowlist_permits_listed_domain() {
    let filter = DomainFilter {
        allowlist: vec!["example.com".to_string()],
        ..Default::default()
    };
    let url = Url::parse("https://example.com/page").unwrap();
    assert!(filter.is_allowed(&url).is_ok());
}

#[test]
fn allowlist_rejects_unlisted_domain() {
    let filter = DomainFilter {
        allowlist: vec!["example.com".to_string()],
        ..Default::default()
    };
    let url = Url::parse("https://evil.com").unwrap();
    let err = filter.is_allowed(&url).unwrap_err();
    assert!(matches!(err, DomainFilterError::NotAllowlisted(_)));
}

// ---------------------------------------------------------------------------
// DomainFilter — denylist
// ---------------------------------------------------------------------------

#[test]
fn denylist_blocks_listed_domain() {
    let filter = DomainFilter {
        denylist: vec!["evil.com".to_string()],
        ..Default::default()
    };
    let url = Url::parse("https://evil.com/malware").unwrap();
    let err = filter.is_allowed(&url).unwrap_err();
    assert!(matches!(err, DomainFilterError::DeniedDomain(_)));
}

#[test]
fn denylist_allows_unlisted_domain() {
    let filter = DomainFilter {
        denylist: vec!["evil.com".to_string()],
        ..Default::default()
    };
    let url = Url::parse("https://good.com").unwrap();
    assert!(filter.is_allowed(&url).is_ok());
}

// ---------------------------------------------------------------------------
// DomainFilter — empty config allows all public domains
// ---------------------------------------------------------------------------

#[test]
fn empty_config_allows_any_public_domain() {
    let filter = DomainFilter::default();
    for domain in &["example.com", "docs.rs", "crates.io", "github.com"] {
        let url = Url::parse(&format!("https://{domain}")).unwrap();
        assert!(filter.is_allowed(&url).is_ok(), "should allow {domain}");
    }
}

// ---------------------------------------------------------------------------
// DomainFilter — private IP blocking
// ---------------------------------------------------------------------------

#[test]
fn blocks_localhost_127_0_0_1() {
    let filter = DomainFilter {
        block_private_ips: true,
        ..Default::default()
    };
    let url = Url::parse("http://127.0.0.1/admin").unwrap();
    let result = filter.is_allowed(&url);
    assert!(result.is_err(), "127.0.0.1 should be blocked");
}

#[test]
fn blocks_10_x_range() {
    let filter = DomainFilter {
        block_private_ips: true,
        ..Default::default()
    };
    let url = Url::parse("http://10.0.0.1/internal").unwrap();
    let result = filter.is_allowed(&url);
    assert!(result.is_err(), "10.0.0.1 should be blocked");
}

#[test]
fn blocks_172_16_x_range() {
    let filter = DomainFilter {
        block_private_ips: true,
        ..Default::default()
    };
    let url = Url::parse("http://172.16.0.1/secret").unwrap();
    let result = filter.is_allowed(&url);
    assert!(result.is_err(), "172.16.0.1 should be blocked");
}

#[test]
fn blocks_192_168_x_range() {
    let filter = DomainFilter {
        block_private_ips: true,
        ..Default::default()
    };
    let url = Url::parse("http://192.168.1.1/router").unwrap();
    let result = filter.is_allowed(&url);
    assert!(result.is_err(), "192.168.1.1 should be blocked");
}

// ---------------------------------------------------------------------------
// DomainFilterPolicy — passthrough for non-web tools
// ---------------------------------------------------------------------------

#[test]
fn policy_ignores_non_web_tools() {
    let policy = DomainFilterPolicy::new(DomainFilter {
        denylist: vec!["evil.com".to_string()],
        ..Default::default()
    });
    let (usage, cost, state) = make_policy_context();
    let ctx = ctx_from(&usage, &cost, &state);
    let mut args = json!({"url": "https://evil.com"});
    let mut tool = ToolPolicyContext {
        tool_name: "bash",
        tool_call_id: "call_1",
        arguments: &mut args,
    };
    let verdict = policy.evaluate(&ctx, &mut tool);
    assert!(matches!(verdict, PreDispatchVerdict::Continue));
}

// ---------------------------------------------------------------------------
// DomainFilterPolicy — passthrough when no url argument
// ---------------------------------------------------------------------------

#[test]
fn policy_continues_when_no_url_arg() {
    let policy = DomainFilterPolicy::new(DomainFilter {
        denylist: vec!["evil.com".to_string()],
        ..Default::default()
    });
    let (usage, cost, state) = make_policy_context();
    let ctx = ctx_from(&usage, &cost, &state);
    let mut args = json!({"query": "rust programming"});
    let mut tool = ToolPolicyContext {
        tool_name: "web.search",
        tool_call_id: "call_2",
        arguments: &mut args,
    };
    let verdict = policy.evaluate(&ctx, &mut tool);
    assert!(matches!(verdict, PreDispatchVerdict::Continue));
}

// ---------------------------------------------------------------------------
// DomainFilterPolicy — blocks denied domain
// ---------------------------------------------------------------------------

#[test]
fn policy_blocks_denied_domain() {
    let policy = DomainFilterPolicy::new(DomainFilter {
        denylist: vec!["evil.com".to_string()],
        ..Default::default()
    });
    let (usage, cost, state) = make_policy_context();
    let ctx = ctx_from(&usage, &cost, &state);
    let mut args = json!({"url": "https://evil.com/page"});
    let mut tool = ToolPolicyContext {
        tool_name: "web.fetch",
        tool_call_id: "call_3",
        arguments: &mut args,
    };
    let verdict = policy.evaluate(&ctx, &mut tool);
    assert!(
        matches!(verdict, PreDispatchVerdict::Skip(_)),
        "expected Skip, got {verdict:?}"
    );
}

// ---------------------------------------------------------------------------
// DomainFilterPolicy — allows valid domain
// ---------------------------------------------------------------------------

#[test]
fn policy_allows_valid_domain() {
    let policy = DomainFilterPolicy::new(DomainFilter::default());
    let (usage, cost, state) = make_policy_context();
    let ctx = ctx_from(&usage, &cost, &state);
    let mut args = json!({"url": "https://example.com"});
    let mut tool = ToolPolicyContext {
        tool_name: "web.fetch",
        tool_call_id: "call_4",
        arguments: &mut args,
    };
    let verdict = policy.evaluate(&ctx, &mut tool);
    assert!(matches!(verdict, PreDispatchVerdict::Continue));
}

// ---------------------------------------------------------------------------
// DomainFilterPolicy — rejects bad scheme
// ---------------------------------------------------------------------------

#[test]
fn policy_rejects_file_scheme() {
    let policy = DomainFilterPolicy::new(DomainFilter::default());
    let (usage, cost, state) = make_policy_context();
    let ctx = ctx_from(&usage, &cost, &state);
    let mut args = json!({"url": "file:///etc/passwd"});
    let mut tool = ToolPolicyContext {
        tool_name: "web.fetch",
        tool_call_id: "call_5",
        arguments: &mut args,
    };
    let verdict = policy.evaluate(&ctx, &mut tool);
    assert!(
        matches!(verdict, PreDispatchVerdict::Skip(_)),
        "expected Skip for file:// scheme"
    );
}

// ---------------------------------------------------------------------------
// DomainFilterPolicy — rejects invalid URL
// ---------------------------------------------------------------------------

#[test]
fn policy_skips_invalid_url() {
    let policy = DomainFilterPolicy::new(DomainFilter::default());
    let (usage, cost, state) = make_policy_context();
    let ctx = ctx_from(&usage, &cost, &state);
    let mut args = json!({"url": "not a url at all"});
    let mut tool = ToolPolicyContext {
        tool_name: "web.fetch",
        tool_call_id: "call_6",
        arguments: &mut args,
    };
    let verdict = policy.evaluate(&ctx, &mut tool);
    assert!(
        matches!(verdict, PreDispatchVerdict::Skip(_)),
        "expected Skip for unparseable URL"
    );
}
