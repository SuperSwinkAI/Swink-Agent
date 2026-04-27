//! US3: mutation strategy tests.

use std::sync::Arc;

use swink_agent_eval::{JudgeVerdict, MockJudge, Score};

use swink_agent_evolve::{
    Ablation, CaseFailure, Candidate, LlmGuided, MutationContext, MutationStrategy,
    TargetComponent, TemplateBased, WeakPoint, deduplicate,
};

fn make_context(max_candidates: usize) -> MutationContext {
    let weak_point = WeakPoint {
        component: TargetComponent::FullPrompt,
        affected_cases: vec![CaseFailure {
            case_id: "c1".to_string(),
            evaluator_name: "response".to_string(),
            score: Score { value: 0.2, threshold: 0.5 },
            details: None,
        }],
        mean_score_gap: 0.3,
        severity: 0.3,
    };
    MutationContext {
        weak_point,
        failing_traces: vec![],
        eval_criteria: "response quality".to_string(),
        seed: None,
        max_candidates,
    }
}

#[test]
fn template_based_produces_candidates() {
    let strategy = TemplateBased::new();
    let context = make_context(10);
    // "Must" → "Should" template fires on this text
    let candidates = strategy.mutate("You Must help users.", &context).unwrap();
    assert!(!candidates.is_empty(), "TemplateBased should produce at least one candidate");
    for c in &candidates {
        assert_ne!(c.mutated_value, "You Must help users.", "candidate should differ from original");
    }
}

#[test]
fn ablation_produces_two_candidates() {
    let strategy = Ablation::new();
    let context = make_context(3);
    let target = "First sentence. Second sentence with more text.";
    let candidates = strategy.mutate(target, &context).unwrap();
    assert_eq!(candidates.len(), 2);
    // One is the full removal (empty)
    assert!(
        candidates.iter().any(|c| c.mutated_value.is_empty()),
        "ablation should produce a full-removal candidate"
    );
    // One is the first-sentence simplification
    assert!(
        candidates.iter().any(|c| c.mutated_value == "First sentence."),
        "ablation should produce a first-sentence candidate"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn llm_guided_uses_judge_client() {
    let rewrite = "improved text from judge";
    let judge = Arc::new(MockJudge::with_verdicts(vec![JudgeVerdict {
        score: 0.9,
        pass: true,
        reason: Some(rewrite.to_string()),
        label: None,
    }]));
    let strategy = LlmGuided::new(judge);
    let context = make_context(3);
    let candidates = strategy.mutate("original text", &context).unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].mutated_value, rewrite);
    assert_eq!(candidates[0].strategy, "llm_guided");
}

#[test]
fn deterministic_seed_produces_identical_results() {
    let strategy = TemplateBased::new();
    let mut ctx = make_context(2);
    ctx.seed = Some(42);
    // Use a target that fires multiple templates so shuffling matters
    let target = "You Must help users and you should utilize tools.";
    let run1 = strategy.mutate(target, &ctx).unwrap();
    let run2 = strategy.mutate(target, &ctx).unwrap();
    assert_eq!(run1.len(), run2.len());
    for (a, b) in run1.iter().zip(run2.iter()) {
        assert_eq!(a.mutated_value, b.mutated_value);
    }
}

#[test]
fn candidates_deduplicated_by_hash() {
    let original = "You are helpful.";
    let same_text = "You are a helper.";
    // Two strategies independently produce the same mutated text
    let c1 = Candidate::new(
        TargetComponent::FullPrompt,
        original.to_string(),
        same_text.to_string(),
        "strategy_a".to_string(),
    );
    let c2 = Candidate::new(
        TargetComponent::FullPrompt,
        original.to_string(),
        same_text.to_string(),
        "strategy_b".to_string(),
    );
    let deduped = deduplicate(vec![c1, c2], original);
    assert_eq!(deduped.len(), 1, "duplicate mutated values should be reduced to one candidate");
}

#[test]
fn max_candidates_per_strategy_enforced() {
    let mut ctx = make_context(1);
    ctx.seed = Some(0);

    let template = TemplateBased::new();
    let target = "You Must help users and you should utilize tools.";
    let candidates = template.mutate(target, &ctx).unwrap();
    assert!(candidates.len() <= 1, "TemplateBased should respect max_candidates cap");

    let ablation = Ablation::new();
    let candidates = ablation.mutate("First sentence. Second sentence.", &ctx).unwrap();
    assert!(candidates.len() <= 1, "Ablation should respect max_candidates cap");
}
