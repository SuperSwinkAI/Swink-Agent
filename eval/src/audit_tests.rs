//! Tests for `audit`.
#![cfg(test)]

use std::time::Duration;

use swink_agent::{AssistantMessage, Cost, ModelSpec, StopReason, Usage};

use super::*;
use crate::types::TurnRecord;

fn minimal_invocation(num_turns: usize) -> Invocation {
    let turns = (0..num_turns)
        .map(|i| TurnRecord {
            turn_index: i,
            assistant_message: AssistantMessage::new(vec![], "test", "test-model")
                .with_timestamp(0),
            tool_calls: vec![],
            tool_results: vec![],
            duration: Duration::from_millis(10),
        })
        .collect();

    Invocation {
        turns,
        total_usage: Usage::default(),
        total_cost: Cost::default(),
        total_duration: Duration::from_millis(10 * num_turns as u64),
        final_response: None,
        stop_reason: StopReason::Stop,
        model: ModelSpec::new("test", "test-model"),
    }
}

#[test]
fn roundtrip_verify() {
    let inv = minimal_invocation(3);
    let audited = AuditedInvocation::from_invocation(inv);

    assert!(audited.verify());
    assert_eq!(audited.turn_hashes.len(), 3);
    for hash in &audited.turn_hashes {
        assert_eq!(hash.len(), 64);
    }
    assert_eq!(audited.chain_hash.len(), 64);
}

#[test]
fn tampered_turn_fails_verify() {
    let inv = minimal_invocation(2);
    let mut audited = AuditedInvocation::from_invocation(inv);

    audited.turn_hashes[0] = "0".repeat(64);

    assert!(!audited.verify());
}

#[test]
fn empty_invocation() {
    let inv = minimal_invocation(0);
    let audited = AuditedInvocation::from_invocation(inv);

    assert!(audited.verify());
    assert!(audited.turn_hashes.is_empty());
    assert_eq!(audited.chain_hash.len(), 64);
}
