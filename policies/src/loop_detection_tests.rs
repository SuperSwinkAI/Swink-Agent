//! Tests for `loop_detection`.
#![cfg(test)]

use super::*;
use swink_agent::{AssistantMessage, ContentBlock, Cost, StopReason, Usage};

fn make_ctx<'a>(
    usage: &'a Usage,
    cost: &'a Cost,
    state: &'a swink_agent::SessionState,
) -> PolicyContext<'a> {
    PolicyContext::new(0, usage, cost, 0, false, &[], state)
}

fn make_turn_ctx<'a>(
    msg: &'a AssistantMessage,
    results: &'a [swink_agent::ToolResultMessage],
) -> TurnPolicyContext<'a> {
    static MODEL: std::sync::LazyLock<swink_agent::ModelSpec> =
        std::sync::LazyLock::new(|| swink_agent::ModelSpec::new("test", "test-model"));
    TurnPolicyContext::new(msg, results, StopReason::Stop, "", &MODEL, &[])
}

fn msg_with_tool_calls(calls: &[(&str, &str, serde_json::Value)]) -> AssistantMessage {
    let content = calls
        .iter()
        .map(|(id, name, args)| ContentBlock::ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments: args.clone(),
            partial_json: None,
        })
        .collect();
    AssistantMessage::new(content, String::new(), String::new()).with_timestamp(0)
}

fn tool_result(id: &str, text: &str) -> swink_agent::ToolResultMessage {
    swink_agent::ToolResultMessage::new(
        id.to_string(),
        vec![ContentBlock::Text {
            text: text.to_string(),
        }],
    )
    .with_timestamp(0)
}

#[test]
fn no_repeat_returns_continue() {
    let policy = LoopDetectionPolicy::new(3);
    let usage = Usage::default();
    let cost = Cost::default();
    let state = swink_agent::SessionState::new();
    let ctx = make_ctx(&usage, &cost, &state);

    let msg1 = msg_with_tool_calls(&[("id1", "bash", serde_json::json!({"cmd": "ls"}))]);
    let results1 = vec![tool_result("id1", "output1")];
    let turn1 = make_turn_ctx(&msg1, &results1);
    assert!(matches!(
        policy.evaluate(&ctx, &turn1),
        PolicyVerdict::Continue
    ));

    let msg2 = msg_with_tool_calls(&[("id2", "bash", serde_json::json!({"cmd": "pwd"}))]);
    let results2 = vec![tool_result("id2", "output2")];
    let turn2 = make_turn_ctx(&msg2, &results2);
    assert!(matches!(
        policy.evaluate(&ctx, &turn2),
        PolicyVerdict::Continue
    ));

    let msg3 = msg_with_tool_calls(&[("id3", "bash", serde_json::json!({"cmd": "whoami"}))]);
    let results3 = vec![tool_result("id3", "output3")];
    let turn3 = make_turn_ctx(&msg3, &results3);
    assert!(matches!(
        policy.evaluate(&ctx, &turn3),
        PolicyVerdict::Continue
    ));
}

#[test]
fn repeat_within_lookback_returns_stop() {
    let policy = LoopDetectionPolicy::new(3);
    let usage = Usage::default();
    let cost = Cost::default();
    let state = swink_agent::SessionState::new();
    let ctx = make_ctx(&usage, &cost, &state);

    // Same tool name + args each turn (but tool_call_id could differ)
    let msg = msg_with_tool_calls(&[("id1", "bash", serde_json::json!({"cmd": "ls"}))]);
    let results = vec![tool_result("id1", "same_output")];

    let turn = make_turn_ctx(&msg, &results);
    let r1 = policy.evaluate(&ctx, &turn);
    assert!(matches!(r1, PolicyVerdict::Continue)); // 1 entry, need 3

    let turn = make_turn_ctx(&msg, &results);
    let r2 = policy.evaluate(&ctx, &turn);
    assert!(matches!(r2, PolicyVerdict::Continue)); // 2 entries, need 3

    let turn = make_turn_ctx(&msg, &results);
    let r3 = policy.evaluate(&ctx, &turn);
    assert!(matches!(r3, PolicyVerdict::Stop(_))); // 3 identical entries -> stuck
}

#[test]
fn repeat_with_steering_returns_inject() {
    let policy = LoopDetectionPolicy::new(2).with_steering("Try something different");
    let usage = Usage::default();
    let cost = Cost::default();
    let state = swink_agent::SessionState::new();
    let ctx = make_ctx(&usage, &cost, &state);

    let msg = msg_with_tool_calls(&[("id1", "bash", serde_json::json!({"cmd": "ls"}))]);
    let results = vec![tool_result("id1", "same")];

    let turn = make_turn_ctx(&msg, &results);
    let _ = policy.evaluate(&ctx, &turn); // 1st

    let turn = make_turn_ctx(&msg, &results);
    let r = policy.evaluate(&ctx, &turn); // 2nd identical -> inject
    assert!(matches!(r, PolicyVerdict::Inject(_)));
}

#[test]
fn different_args_not_detected() {
    let policy = LoopDetectionPolicy::new(2);
    let usage = Usage::default();
    let cost = Cost::default();
    let state = swink_agent::SessionState::new();
    let ctx = make_ctx(&usage, &cost, &state);

    let msg1 = msg_with_tool_calls(&[("id1", "bash", serde_json::json!({"cmd": "ls"}))]);
    let results1 = vec![tool_result("id1", "output_a")];
    let turn1 = make_turn_ctx(&msg1, &results1);
    let _ = policy.evaluate(&ctx, &turn1);

    let msg2 = msg_with_tool_calls(&[("id2", "bash", serde_json::json!({"cmd": "pwd"}))]);
    let results2 = vec![tool_result("id2", "output_b")];
    let turn2 = make_turn_ctx(&msg2, &results2);
    let r = policy.evaluate(&ctx, &turn2);
    assert!(matches!(r, PolicyVerdict::Continue));
}

#[test]
fn lookback_window_respected() {
    let policy = LoopDetectionPolicy::new(3);
    let usage = Usage::default();
    let cost = Cost::default();
    let state = swink_agent::SessionState::new();
    let ctx = make_ctx(&usage, &cost, &state);

    let same_args = serde_json::json!({"cmd": "ls"});
    let diff_args = serde_json::json!({"cmd": "pwd"});

    // Push 2 identical, then 1 different, then 2 identical again
    // Should not trigger because the different one breaks the streak
    let same_msg = msg_with_tool_calls(&[("id1", "bash", same_args)]);
    let same_res = vec![tool_result("id1", "same")];
    let diff_msg = msg_with_tool_calls(&[("id2", "bash", diff_args)]);
    let diff_res = vec![tool_result("id2", "different")];

    let t = make_turn_ctx(&same_msg, &same_res);
    let _ = policy.evaluate(&ctx, &t);
    let t = make_turn_ctx(&same_msg, &same_res);
    let _ = policy.evaluate(&ctx, &t);
    let t = make_turn_ctx(&diff_msg, &diff_res);
    let _ = policy.evaluate(&ctx, &t);
    let t = make_turn_ctx(&same_msg, &same_res);
    let _ = policy.evaluate(&ctx, &t);
    let t = make_turn_ctx(&same_msg, &same_res);
    let r = policy.evaluate(&ctx, &t);
    // Last 3: different, same, same — not all identical
    assert!(matches!(r, PolicyVerdict::Continue));
}

/// Regression test for #276: identical tool calls with different `tool_call_ids`
/// must still be detected as a loop.
#[test]
fn fresh_tool_call_ids_still_detected_as_loop() {
    let policy = LoopDetectionPolicy::new(3);
    let usage = Usage::default();
    let cost = Cost::default();
    let state = swink_agent::SessionState::new();
    let ctx = make_ctx(&usage, &cost, &state);

    let args = serde_json::json!({"command": "ls -la"});

    // Each turn has the same tool name + args but a different tool_call_id
    let msg1 = msg_with_tool_calls(&[("call_abc", "bash", args.clone())]);
    let res1 = vec![tool_result("call_abc", "file1.txt")];
    let t1 = make_turn_ctx(&msg1, &res1);
    assert!(matches!(
        policy.evaluate(&ctx, &t1),
        PolicyVerdict::Continue
    ));

    let msg2 = msg_with_tool_calls(&[("call_def", "bash", args.clone())]);
    let res2 = vec![tool_result("call_def", "file1.txt")];
    let t2 = make_turn_ctx(&msg2, &res2);
    assert!(matches!(
        policy.evaluate(&ctx, &t2),
        PolicyVerdict::Continue
    ));

    let msg3 = msg_with_tool_calls(&[("call_ghi", "bash", args)]);
    let res3 = vec![tool_result("call_ghi", "file1.txt")];
    let t3 = make_turn_ctx(&msg3, &res3);
    // Despite fresh IDs each time, the tool name + args are identical → loop detected
    assert!(matches!(policy.evaluate(&ctx, &t3), PolicyVerdict::Stop(_)));
}
