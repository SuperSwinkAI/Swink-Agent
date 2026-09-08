//! Tests for `deny_list`.
#![cfg(test)]

use super::*;

fn make_dispatch_ctx<'a>(
    tool_name: &'a str,
    args: &'a mut serde_json::Value,
    state: &'a swink_agent::SessionState,
) -> ToolDispatchContext<'a> {
    ToolDispatchContext::new(tool_name, "id1", args, None, state)
}

#[test]
fn denies_listed_tool() {
    let policy = ToolDenyListPolicy::new(["bash", "write_file"]);
    let state = swink_agent::SessionState::new();
    let mut args = serde_json::json!({"command": "ls"});
    let mut ctx = make_dispatch_ctx("bash", &mut args, &state);
    let result = policy.evaluate(&mut ctx);
    assert!(matches!(result, PreDispatchVerdict::Skip(ref e) if e.contains("denied")));
}

#[test]
fn allows_unlisted_tool() {
    let policy = ToolDenyListPolicy::new(["bash"]);
    let state = swink_agent::SessionState::new();
    let mut args = serde_json::json!({"path": "/tmp/file"});
    let mut ctx = make_dispatch_ctx("read_file", &mut args, &state);
    let result = policy.evaluate(&mut ctx);
    assert!(matches!(result, PreDispatchVerdict::Continue));
}

#[test]
fn empty_deny_list_allows_all() {
    let policy = ToolDenyListPolicy::new(Vec::<String>::new());
    let state = swink_agent::SessionState::new();
    let mut args = serde_json::json!({});
    let mut ctx = make_dispatch_ctx("bash", &mut args, &state);
    let result = policy.evaluate(&mut ctx);
    assert!(matches!(result, PreDispatchVerdict::Continue));
}
