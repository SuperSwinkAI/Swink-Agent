//! Integration tests for budget evaluator and budget guard.

mod common;

use std::time::Duration;

use std::sync::Arc;

use swink_agent::{
    AgentEvent, AssistantMessage, ContentBlock, Cost, StopReason, TurnEndReason, TurnSnapshot,
    Usage,
};
use swink_agent_eval::{
    BudgetConstraints, BudgetEvaluator, BudgetGuard, Evaluator, TrajectoryCollector, Verdict,
};
use tokio_util::sync::CancellationToken;

use common::{case_with_budget, mock_invocation};

#[test]
fn passes_within_budget() {
    let case = case_with_budget(BudgetConstraints {
        max_cost: Some(1.0),
        max_tokens: Some(1000),
        max_turns: Some(5),
        max_duration: Some(Duration::from_secs(10)),
    });
    let invocation = mock_invocation(&["read"], Some("done"), 0.5, 500);
    let result = BudgetEvaluator.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Pass);
}

#[test]
fn fails_on_cost_exceeded() {
    let case = case_with_budget(BudgetConstraints {
        max_cost: Some(0.001),
        max_tokens: None,
        max_turns: None,
        max_duration: None,
    });
    let invocation = mock_invocation(&["read"], Some("done"), 0.01, 100);
    let result = BudgetEvaluator.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Fail);
    assert!(result.details.unwrap().contains("cost"));
}

#[test]
fn fails_on_token_exceeded() {
    let case = case_with_budget(BudgetConstraints {
        max_cost: None,
        max_tokens: Some(50),
        max_turns: None,
        max_duration: None,
    });
    let invocation = mock_invocation(&["read"], Some("done"), 0.01, 100);
    let result = BudgetEvaluator.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Fail);
    assert!(result.details.unwrap().contains("tokens"));
}

// ─── BudgetGuard integration tests ──────────────────────────────────────────

fn turn_events(cost: f64, tokens: u64) -> Vec<AgentEvent> {
    vec![
        AgentEvent::TurnStart,
        AgentEvent::TurnEnd {
            assistant_message: AssistantMessage {
                content: vec![ContentBlock::Text {
                    text: "ok".to_string(),
                }],
                provider: "test".to_string(),
                model_id: "test-model".to_string(),
                usage: Usage {
                    total: tokens,
                    ..Default::default()
                },
                cost: Cost {
                    total: cost,
                    ..Default::default()
                },
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 0,
                cache_hint: None,
            },
            tool_results: vec![],
            reason: TurnEndReason::Complete,
            snapshot: TurnSnapshot {
                turn_index: 0,
                messages: Arc::new(vec![]),
                usage: Usage {
                    total: tokens,
                    ..Default::default()
                },
                cost: Cost {
                    total: cost,
                    ..Default::default()
                },
                stop_reason: StopReason::Stop,
                state_delta: None,
            },
        },
    ]
}

#[tokio::test]
async fn guard_cancels_on_cost_exceeded() {
    let cancel = CancellationToken::new();
    let guard = BudgetGuard::new(cancel.clone()).with_max_cost(1.0);

    // 3 turns at $0.5 each → $1.5 total, exceeds $1.0 max
    let mut events = Vec::new();
    events.push(AgentEvent::AgentStart);
    for _ in 0..3 {
        events.extend(turn_events(0.5, 100));
    }
    events.push(AgentEvent::AgentEnd {
        messages: Arc::new(vec![]),
    });

    let stream = futures::stream::iter(events);
    let _invocation = TrajectoryCollector::collect_with_guard(stream, Some(guard)).await;

    assert!(
        cancel.is_cancelled(),
        "guard should have cancelled the token"
    );
}

#[tokio::test]
async fn guard_does_not_cancel_within_budget() {
    let cancel = CancellationToken::new();
    let guard = BudgetGuard::new(cancel.clone())
        .with_max_cost(10.0)
        .with_max_tokens(10000)
        .with_max_turns(10);

    let mut events = Vec::new();
    events.push(AgentEvent::AgentStart);
    events.extend(turn_events(0.1, 50));
    events.push(AgentEvent::AgentEnd {
        messages: Arc::new(vec![]),
    });

    let stream = futures::stream::iter(events);
    let _invocation = TrajectoryCollector::collect_with_guard(stream, Some(guard)).await;

    assert!(
        !cancel.is_cancelled(),
        "guard should not cancel within budget"
    );
}
