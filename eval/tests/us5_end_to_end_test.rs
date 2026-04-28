//! US5 end-to-end regression (T118).
//!
//! Verifies that a generator-produced `EvalSet` can feed into `EvalRunner`
//! without further plumbing and that tool-scoped trajectories are honored
//! when `agent_tools` are supplied.

#![cfg(feature = "generation")]

use std::sync::Arc;

use swink_agent_eval::JudgeVerdict;
use swink_agent_eval::generation::{ExperimentGenerator, GenerationRequest, ToolDef, TopicPlanner};
use swink_agent_eval::testing::MockJudge;
use swink_agent_eval::validate_eval_set;

fn verdict(body: &str) -> JudgeVerdict {
    JudgeVerdict {
        score: 1.0,
        pass: true,
        reason: Some(body.to_string()),
        label: None,
    }
}

#[tokio::test]
async fn generated_set_validates_and_is_runner_ready() {
    // 1 planner + 1 case draft.
    let verdicts = vec![
        verdict(r#"["core"]"#),
        verdict(
            r#"{"name":"ok","system_prompt":"p","user_messages":["do"],"expected_response":"ok","expected_assertion":"done"}"#,
        ),
    ];
    let judge = Arc::new(MockJudge::with_verdicts(verdicts));
    let planner = Arc::new(TopicPlanner::new(judge.clone(), "planner-model"));
    let generator = ExperimentGenerator::new(judge, "gen-model", planner);
    let set = generator
        .generate(GenerationRequest {
            desired_count: 1,
            num_topics: 1,
            include_expected_output: true,
            ..GenerationRequest::default()
        })
        .await
        .expect("generate succeeds");

    validate_eval_set(&set).expect("generated set must validate");
    assert!(!set.cases.is_empty());
    // Every case carries a non-empty id + user_messages required by EvalRunner.
    for case in &set.cases {
        assert!(!case.id.is_empty());
        assert!(!case.user_messages.is_empty());
    }
}

#[tokio::test]
async fn tool_scoped_trajectory_is_respected_when_agent_tools_provided() {
    let verdicts = vec![
        verdict(r#"["scoped"]"#),
        verdict(
            r#"{"name":"s","system_prompt":"p","user_messages":["do"],"expected_trajectory":[{"tool_name":"ok_tool"},{"tool_name":"leak_tool"}]}"#,
        ),
    ];
    let judge = Arc::new(MockJudge::with_verdicts(verdicts));
    let planner = Arc::new(TopicPlanner::new(judge.clone(), "planner-model"));
    let generator = ExperimentGenerator::new(judge, "gen-model", planner);
    let set = generator
        .generate(GenerationRequest {
            desired_count: 1,
            num_topics: 1,
            include_expected_trajectory: true,
            agent_tools: Some(vec![ToolDef::new("ok_tool", "scoped tool")]),
            ..GenerationRequest::default()
        })
        .await
        .expect("generate succeeds");

    assert_eq!(set.cases.len(), 1);
    let traj = set.cases[0]
        .expected_trajectory
        .as_ref()
        .expect("trajectory present");
    assert_eq!(traj.len(), 1);
    assert_eq!(traj[0].tool_name, "ok_tool");
}
