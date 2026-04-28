//! US5 generation regression tests (T115).

#![cfg(feature = "generation")]

use std::sync::Arc;

use swink_agent_eval::JudgeVerdict;
use swink_agent_eval::generation::{ExperimentGenerator, GenerationRequest, ToolDef, TopicPlanner};
use swink_agent_eval::testing::MockJudge;

fn verdict_json(body: &str) -> JudgeVerdict {
    JudgeVerdict {
        score: 1.0,
        pass: true,
        reason: Some(body.to_string()),
        label: None,
    }
}

fn topic_list_verdict(topics: &[&str]) -> JudgeVerdict {
    let body = serde_json::to_string(&topics.to_vec()).unwrap();
    verdict_json(&body)
}

fn case_body(name: &str, tool: Option<&str>) -> String {
    let trajectory = tool
        .map(|t| format!(r#","expected_trajectory":[{{"tool_name":"{t}"}}]"#))
        .unwrap_or_default();
    format!(
        r#"{{"name":"{name}","system_prompt":"you are helpful","user_messages":["hi"],"expected_response":"okay","expected_assertion":"goal met"{trajectory}}}"#
    )
}

#[tokio::test]
async fn twenty_cases_across_five_topics_have_even_distribution() {
    // Judge: 1 planner call + 20 case calls (5 topics × 4 cases each).
    let mut verdicts = vec![topic_list_verdict(&[
        "alpha", "beta", "gamma", "delta", "epsilon",
    ])];
    for i in 0..20 {
        verdicts.push(verdict_json(&case_body(&format!("case-{i}"), None)));
    }
    let judge = Arc::new(MockJudge::with_verdicts(verdicts));
    let planner = Arc::new(TopicPlanner::new(judge.clone(), "planner-model"));
    let generator = ExperimentGenerator::new(judge, "gen-model", planner);

    let request = GenerationRequest {
        context: "multi-tool agent".into(),
        task: "help the user".into(),
        desired_count: 20,
        num_topics: 5,
        include_expected_output: true,
        include_expected_trajectory: false,
        include_expected_interactions: false,
        include_metadata: false,
        agent_tools: None,
    };
    let set = generator.generate(request).await.unwrap();
    assert_eq!(
        set.cases.len(),
        20,
        "expected 20 cases, got {}",
        set.cases.len()
    );

    // Five buckets of 4 cases each (20/5 = 4). Inspect case_id prefix derived
    // from slugged topic.
    let mut counts = std::collections::HashMap::<String, u32>::new();
    for case in &set.cases {
        let topic = case
            .id
            .split("::")
            .next()
            .map(String::from)
            .unwrap_or_default();
        *counts.entry(topic).or_default() += 1;
    }
    assert_eq!(counts.len(), 5);
    for (topic, count) in counts {
        assert_eq!(count, 4, "topic {topic} had {count} cases");
    }
}

#[tokio::test]
async fn trajectories_are_scoped_to_provided_tools() {
    let mut verdicts = vec![topic_list_verdict(&["only"])];
    // One valid tool call + one not-in-catalogue tool call — must be filtered.
    let body = r#"{"name":"scoped","system_prompt":"p","user_messages":["m"],"expected_trajectory":[{"tool_name":"allowed_tool"},{"tool_name":"forbidden_tool"}]}"#;
    verdicts.push(verdict_json(body));
    let judge = Arc::new(MockJudge::with_verdicts(verdicts));
    let planner = Arc::new(TopicPlanner::new(judge.clone(), "planner-model"));
    let generator = ExperimentGenerator::new(judge, "gen-model", planner);

    let request = GenerationRequest {
        context: "ctx".into(),
        task: "t".into(),
        desired_count: 1,
        num_topics: 1,
        include_expected_output: false,
        include_expected_trajectory: true,
        include_expected_interactions: false,
        include_metadata: false,
        agent_tools: Some(vec![ToolDef::new("allowed_tool", "the allowed tool")]),
    };
    let set = generator.generate(request).await.unwrap();
    assert_eq!(set.cases.len(), 1);
    let trajectory = set.cases[0]
        .expected_trajectory
        .as_ref()
        .expect("trajectory emitted");
    assert_eq!(trajectory.len(), 1);
    assert_eq!(trajectory[0].tool_name, "allowed_tool");
}

#[tokio::test]
async fn generator_retries_past_malformed_judge_output() {
    let mut verdicts = vec![topic_list_verdict(&["only"])];
    // First 2 attempts are malformed JSON; 3rd succeeds.
    verdicts.push(verdict_json("not json"));
    verdicts.push(verdict_json("{incomplete"));
    verdicts.push(verdict_json(&case_body("recovered", None)));
    let judge = Arc::new(MockJudge::with_verdicts(verdicts));
    let planner = Arc::new(TopicPlanner::new(judge.clone(), "planner-model"));
    let generator = ExperimentGenerator::new(judge, "gen-model", planner).with_retry_cap(3);

    let request = GenerationRequest {
        desired_count: 1,
        num_topics: 1,
        ..GenerationRequest::default()
    };
    let set = generator.generate(request).await.unwrap();
    assert_eq!(set.cases.len(), 1);
    assert!(set.cases[0].name.contains("recovered"));
}

#[tokio::test]
async fn generator_validates_every_emitted_case() {
    // A draft missing user_messages must NOT land in the returned set even
    // when validation is on. The generator drops it and the final set has 0
    // entries (exhausted retries all malformed).
    let mut verdicts = vec![topic_list_verdict(&["only"])];
    for _ in 0..5 {
        verdicts.push(verdict_json(
            r#"{"name":"no-messages","system_prompt":"p"}"#,
        ));
    }
    let judge = Arc::new(MockJudge::with_verdicts(verdicts));
    let planner = Arc::new(TopicPlanner::new(judge.clone(), "planner-model"));
    let generator = ExperimentGenerator::new(judge, "gen-model", planner).with_retry_cap(3);

    let request = GenerationRequest {
        desired_count: 1,
        num_topics: 1,
        ..GenerationRequest::default()
    };
    let set = generator.generate(request).await.unwrap();
    assert_eq!(set.cases.len(), 0, "malformed draft must not appear");
}
