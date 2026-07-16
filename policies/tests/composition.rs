#![cfg(all(
    feature = "audit",
    feature = "content-filter",
    feature = "pii",
    feature = "prompt-guard"
))]
//! Integration tests for policy composition.

use swink_agent::{
    AgentMessage, AssistantMessage, ContentBlock, Cost, LlmMessage, PolicyContext, PolicyVerdict,
    PostTurnPolicy, PreTurnPolicy, StopReason, ToolResultMessage, TurnPolicyContext, Usage,
    UserMessage,
};
use swink_agent_policies::{
    AuditLogger, ContentFilter, JsonlAuditSink, PiiRedactor, PromptInjectionGuard,
};

fn make_assistant_msg(text: &str) -> AssistantMessage {
    AssistantMessage::new(
        vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        "test",
        "test-model",
    )
    .with_timestamp(0)
}

/// T030: All policies can be instantiated and composed together.
#[test]
fn all_policies_compose() {
    let guard = PromptInjectionGuard::new();
    let redactor = PiiRedactor::new();
    let filter = ContentFilter::new().with_keyword("blocked-term");
    let logger = AuditLogger::new(JsonlAuditSink::new("/dev/null"));

    // Verify names are distinct
    let pre_name = PreTurnPolicy::name(&guard);
    let post_guard_name = PostTurnPolicy::name(&guard);
    let redactor_name = PostTurnPolicy::name(&redactor);
    let filter_name = PostTurnPolicy::name(&filter);
    let logger_name = PostTurnPolicy::name(&logger);

    assert_eq!(pre_name, post_guard_name); // same struct, same name
    assert_ne!(redactor_name, filter_name);
    assert_ne!(filter_name, logger_name);

    // All can evaluate without interfering
    let usage = Usage::default();
    let cost = Cost::default();
    let messages: Vec<AgentMessage> = vec![AgentMessage::Llm(LlmMessage::User(
        UserMessage::new(vec![ContentBlock::Text {
            text: "Hello, normal message".into(),
        }])
        .with_timestamp(0),
    ))];
    let state = swink_agent::SessionState::new();
    let ctx = PolicyContext::new(0, &usage, &cost, 1, false, &messages, &state);

    // PreTurn: guard allows benign message
    assert!(matches!(
        PreTurnPolicy::evaluate(&guard, &ctx),
        PolicyVerdict::Continue
    ));

    // PostTurn: all policies evaluate without interference
    let msg = make_assistant_msg("This is a clean response with no PII or blocked terms.");
    let tool_results: Vec<ToolResultMessage> = vec![];
    let model = swink_agent::ModelSpec::new("test", "test-model");
    let turn_ctx = TurnPolicyContext::new(&msg, &tool_results, StopReason::Stop, "", &model, &[]);

    assert!(matches!(
        PostTurnPolicy::evaluate(&guard, &ctx, &turn_ctx),
        PolicyVerdict::Continue
    ));
    assert!(matches!(
        PostTurnPolicy::evaluate(&redactor, &ctx, &turn_ctx),
        PolicyVerdict::Continue
    ));
    assert!(matches!(
        PostTurnPolicy::evaluate(&filter, &ctx, &turn_ctx),
        PolicyVerdict::Continue
    ));
    assert!(matches!(
        PostTurnPolicy::evaluate(&logger, &ctx, &turn_ctx),
        PolicyVerdict::Continue
    ));
}
