mod common;

use common::mock_invocation;
use swink_agent_eval::AuditedInvocation;

#[test]
fn audited_invocation_roundtrip_via_json() {
    let inv = mock_invocation(&["read", "write"], Some("done"), 0.01, 500);
    let audited = AuditedInvocation::from_invocation(inv);

    let json = serde_json::to_string(&audited).expect("serialize");
    let restored: AuditedInvocation = serde_json::from_str(&json).expect("deserialize");

    assert!(restored.verify());
}

#[test]
fn tampered_invocation_data_fails_verify() {
    let inv = mock_invocation(&["read"], Some("ok"), 0.0, 100);
    let mut audited = AuditedInvocation::from_invocation(inv);

    audited.invocation.turns[0].turn_index = 999;

    assert!(!audited.verify());
}

#[test]
fn tampered_chain_hash_fails_verify() {
    let inv = mock_invocation(&["read"], Some("ok"), 0.0, 100);
    let mut audited = AuditedInvocation::from_invocation(inv);

    audited.chain_hash = format!("{}{}", "bad".repeat(21), "b");

    assert!(!audited.verify());
}
