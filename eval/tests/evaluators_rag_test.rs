//! Integration tests for the RAG-family evaluators (T065).
//!
//! * Judge-backed evaluators (`RAGGroundednessEvaluator`,
//!   `RAGRetrievalRelevanceEvaluator`, `RAGHelpfulnessEvaluator`) drive
//!   through `MockJudge` to cover happy path, `None`-on-missing-criterion,
//!   `prompt_version` recording, and score clamp.
//! * `EmbeddingSimilarityEvaluator` drives through an in-test `StubEmbedder`
//!   to cover happy path, threshold miss, and `None` on missing criterion.

#![cfg(all(feature = "judge-core", feature = "evaluator-rag"))]

use std::sync::Arc;

use swink_agent_eval::{
    Embedder, EmbedderError, EmbeddingSimilarityEvaluator, Evaluator, FewShotExample, JudgeClient,
    JudgeEvaluatorConfig, JudgeRegistry, JudgeVerdict, MockJudge, RAGGroundednessEvaluator,
    RAGHelpfulnessEvaluator, RAGRetrievalRelevanceEvaluator, Verdict,
};

mod common;

use common::{mock_invocation, mock_invocation_with_response};

fn make_registry(judge: Arc<dyn JudgeClient>) -> Arc<JudgeRegistry> {
    Arc::new(
        JudgeRegistry::builder(judge, "mock-model")
            .build()
            .expect("registry builds"),
    )
}

fn config(judge: Arc<dyn JudgeClient>) -> JudgeEvaluatorConfig {
    JudgeEvaluatorConfig::default_with(make_registry(judge))
}

fn verdict(score: f64, reason: &str) -> JudgeVerdict {
    JudgeVerdict {
        score,
        pass: (0.5..=1.0).contains(&score),
        reason: Some(reason.to_string()),
        label: None,
    }
}

fn retrieved_passages() -> Vec<FewShotExample> {
    vec![
        FewShotExample {
            input: "Paris is the capital of France.".into(),
            expected: "supporting passage".into(),
            reasoning: None,
        },
        FewShotExample {
            input: "France is a country in Europe.".into(),
            expected: "supporting passage".into(),
            reasoning: None,
        },
    ]
}

// ─── RAGGroundednessEvaluator ───────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rag_groundedness_records_prompt_version_against_retrieved_context() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::with_verdicts(vec![verdict(
        1.0,
        "every claim supported",
    )]));
    let evaluator = RAGGroundednessEvaluator::new(config(Arc::clone(&judge)));
    let mut case = common::case_with_trajectory(vec![]);
    case.few_shot_examples = retrieved_passages();
    let invocation = mock_invocation_with_response(&[], "Paris is the capital of France.");

    let result = evaluator
        .evaluate(&case, &invocation)
        .expect("groundedness emits a result when retrieved context is present");
    assert_eq!(result.evaluator_name, "rag_groundedness");
    assert!((result.score.value - 1.0).abs() < f64::EPSILON);
    let details = result.details.expect("details populated");
    assert!(details.contains("rag_groundedness_v0"));
    assert!(details.contains("every claim supported"));
}

#[test]
fn rag_groundedness_returns_none_without_retrieved_context() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
    let evaluator = RAGGroundednessEvaluator::new(config(judge));
    let case = common::case_with_trajectory(vec![]); // no few_shot_examples
    let invocation = mock_invocation_with_response(&[], "the sky is blue");

    assert!(evaluator.evaluate(&case, &invocation).is_none());
}

#[test]
fn rag_groundedness_returns_none_without_final_response() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
    let evaluator = RAGGroundednessEvaluator::new(config(judge));
    let mut case = common::case_with_trajectory(vec![]);
    case.few_shot_examples = retrieved_passages();
    let mut invocation = mock_invocation(&[], None, 0.0, 0);
    invocation.final_response = None;

    assert!(evaluator.evaluate(&case, &invocation).is_none());
}

#[test]
fn rag_groundedness_returns_none_without_user_prompt() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
    let evaluator = RAGGroundednessEvaluator::new(config(judge));
    let mut case = common::case_with_trajectory(vec![]);
    case.few_shot_examples = retrieved_passages();
    case.user_messages.clear();
    let invocation = mock_invocation_with_response(&[], "some answer");

    assert!(evaluator.evaluate(&case, &invocation).is_none());
}

// ─── RAGRetrievalRelevanceEvaluator ─────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rag_retrieval_relevance_scores_context_for_user_prompt() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::with_verdicts(vec![verdict(
        0.9,
        "on-topic retrieval",
    )]));
    let evaluator = RAGRetrievalRelevanceEvaluator::new(config(Arc::clone(&judge)));
    let mut case = common::case_with_trajectory(vec![]);
    case.few_shot_examples = retrieved_passages();
    let invocation = mock_invocation_with_response(&[], "anything at all");

    let result = evaluator
        .evaluate(&case, &invocation)
        .expect("retrieval-relevance emits a result");
    assert_eq!(result.evaluator_name, "rag_retrieval_relevance");
    let details = result.details.expect("details populated");
    assert!(details.contains("rag_retrieval_relevance_v0"));
    assert!(details.contains("on-topic"));
}

#[test]
fn rag_retrieval_relevance_returns_none_without_retrieved_context() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
    let evaluator = RAGRetrievalRelevanceEvaluator::new(config(judge));
    let case = common::case_with_trajectory(vec![]); // no few_shot_examples
    let invocation = mock_invocation_with_response(&[], "answer");

    assert!(evaluator.evaluate(&case, &invocation).is_none());
}

#[test]
fn rag_retrieval_relevance_returns_none_without_user_prompt() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
    let evaluator = RAGRetrievalRelevanceEvaluator::new(config(judge));
    let mut case = common::case_with_trajectory(vec![]);
    case.few_shot_examples = retrieved_passages();
    case.user_messages.clear();
    let invocation = mock_invocation_with_response(&[], "answer");

    assert!(evaluator.evaluate(&case, &invocation).is_none());
}

// ─── RAGHelpfulnessEvaluator ────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rag_helpfulness_emits_result_for_grounded_response() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::with_verdicts(vec![verdict(
        0.75,
        "used retrieved passages",
    )]));
    let evaluator = RAGHelpfulnessEvaluator::new(config(Arc::clone(&judge)));
    let mut case = common::case_with_trajectory(vec![]);
    case.few_shot_examples = retrieved_passages();
    let invocation = mock_invocation_with_response(&[], "Paris is the capital of France.");

    let result = evaluator
        .evaluate(&case, &invocation)
        .expect("helpfulness emits a result");
    assert_eq!(result.evaluator_name, "rag_helpfulness");
    assert!(result.details.unwrap().contains("rag_helpfulness_v0"));
}

#[test]
fn rag_helpfulness_returns_none_without_retrieved_context() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
    let evaluator = RAGHelpfulnessEvaluator::new(config(judge));
    let case = common::case_with_trajectory(vec![]);
    let invocation = mock_invocation_with_response(&[], "answer");

    assert!(evaluator.evaluate(&case, &invocation).is_none());
}

// ─── Score clamp (FR-021) ───────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rag_score_clamps_out_of_range_verdict() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::with_verdicts(vec![verdict(
        1.5,
        "too confident",
    )]));
    let evaluator = RAGGroundednessEvaluator::new(config(Arc::clone(&judge)));
    let mut case = common::case_with_trajectory(vec![]);
    case.few_shot_examples = retrieved_passages();
    let invocation = mock_invocation_with_response(&[], "Paris is the capital of France.");

    let result = evaluator.evaluate(&case, &invocation).expect("result");
    assert!((result.score.value - 1.0).abs() < f64::EPSILON);
    let details = result.details.expect("details");
    assert!(
        details.contains("score_clamped"),
        "expected score_clamped detail, got: {details}"
    );
    assert!(details.contains("1.5"));
}

// ─── EmbeddingSimilarityEvaluator ───────────────────────────────────────────

/// Deterministic test-double that maps a string into a fixed-length vector
/// via a caller-supplied lookup table. Any text not in the table returns
/// `InvalidInput`, making it easy to write negative-path tests.
struct StubEmbedder {
    entries: Vec<(String, Vec<f32>)>,
}

impl StubEmbedder {
    fn new(entries: Vec<(&str, Vec<f32>)>) -> Self {
        Self {
            entries: entries
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        }
    }
}

impl Embedder for StubEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedderError> {
        self.entries
            .iter()
            .find(|(k, _)| k == text)
            .map(|(_, v)| v.clone())
            .ok_or_else(|| EmbedderError::InvalidInput {
                reason: format!("no stub vector for `{text}`"),
            })
    }
}

#[test]
fn embedding_similarity_passes_when_vectors_are_identical() {
    let embedder: Arc<dyn Embedder> = Arc::new(StubEmbedder::new(vec![
        ("actual response", vec![1.0, 0.0, 0.0]),
        ("reference answer", vec![1.0, 0.0, 0.0]),
    ]));
    let evaluator = EmbeddingSimilarityEvaluator::new("reference answer", embedder);
    let case = common::case_with_trajectory(vec![]);
    let invocation = mock_invocation_with_response(&[], "actual response");

    let result = evaluator.evaluate(&case, &invocation).expect("result");
    assert_eq!(result.evaluator_name, "embedding_similarity");
    // cosine similarity 1.0 → remapped 1.0 → pass against default 0.8.
    assert!((result.score.value - 1.0).abs() < f64::EPSILON);
    assert_eq!(result.score.verdict(), Verdict::Pass);
    let details = result.details.expect("details populated");
    assert!(details.contains("cosine_similarity="));
    assert!(details.contains("threshold="));
}

#[test]
fn embedding_similarity_fails_when_below_threshold() {
    // Orthogonal vectors → cosine 0 → remapped 0.5 → below the default 0.8.
    let embedder: Arc<dyn Embedder> = Arc::new(StubEmbedder::new(vec![
        ("actual response", vec![1.0, 0.0]),
        ("reference answer", vec![0.0, 1.0]),
    ]));
    let evaluator = EmbeddingSimilarityEvaluator::new("reference answer", embedder);
    let case = common::case_with_trajectory(vec![]);
    let invocation = mock_invocation_with_response(&[], "actual response");

    let result = evaluator.evaluate(&case, &invocation).expect("result");
    assert!((result.score.value - 0.5).abs() < 1e-9);
    assert_eq!(result.score.verdict(), Verdict::Fail);
}

#[test]
fn embedding_similarity_honours_custom_threshold() {
    // Orthogonal vectors → remapped 0.5 → pass when threshold lowered to 0.4.
    let embedder: Arc<dyn Embedder> = Arc::new(StubEmbedder::new(vec![
        ("actual response", vec![1.0, 0.0]),
        ("reference answer", vec![0.0, 1.0]),
    ]));
    let evaluator =
        EmbeddingSimilarityEvaluator::new("reference answer", embedder).with_threshold(0.4);
    let case = common::case_with_trajectory(vec![]);
    let invocation = mock_invocation_with_response(&[], "actual response");

    let result = evaluator.evaluate(&case, &invocation).expect("result");
    assert_eq!(result.score.verdict(), Verdict::Pass);
    assert!((evaluator.threshold() - 0.4).abs() < f64::EPSILON);
}

#[test]
fn embedding_similarity_returns_none_when_final_response_missing() {
    let embedder: Arc<dyn Embedder> = Arc::new(StubEmbedder::new(vec![(
        "reference answer",
        vec![1.0, 0.0, 0.0],
    )]));
    let evaluator = EmbeddingSimilarityEvaluator::new("reference answer", embedder);
    let case = common::case_with_trajectory(vec![]);
    let mut invocation = mock_invocation(&[], None, 0.0, 0);
    invocation.final_response = None;

    assert!(evaluator.evaluate(&case, &invocation).is_none());
}

#[test]
fn embedding_similarity_returns_none_when_final_response_is_blank() {
    let embedder: Arc<dyn Embedder> = Arc::new(StubEmbedder::new(vec![(
        "reference answer",
        vec![1.0, 0.0, 0.0],
    )]));
    let evaluator = EmbeddingSimilarityEvaluator::new("reference answer", embedder);
    let case = common::case_with_trajectory(vec![]);
    let invocation = mock_invocation_with_response(&[], "   ");

    assert!(evaluator.evaluate(&case, &invocation).is_none());
}

#[test]
fn embedding_similarity_reports_embedder_failure_as_score_fail() {
    // The stub only knows the reference; the response embed call fails.
    let embedder: Arc<dyn Embedder> = Arc::new(StubEmbedder::new(vec![(
        "reference answer",
        vec![1.0, 0.0, 0.0],
    )]));
    let evaluator = EmbeddingSimilarityEvaluator::new("reference answer", embedder);
    let case = common::case_with_trajectory(vec![]);
    let invocation = mock_invocation_with_response(&[], "unknown text");

    let result = evaluator.evaluate(&case, &invocation).expect("result");
    assert_eq!(result.score.verdict(), Verdict::Fail);
    let details = result.details.expect("details");
    assert!(details.contains("embed_response"));
    assert!(details.contains("invalid input"));
}

#[test]
fn embedding_similarity_reports_dimension_mismatch() {
    let embedder: Arc<dyn Embedder> = Arc::new(StubEmbedder::new(vec![
        ("actual response", vec![1.0, 0.0, 0.0]),
        ("reference answer", vec![1.0, 0.0]),
    ]));
    let evaluator = EmbeddingSimilarityEvaluator::new("reference answer", embedder);
    let case = common::case_with_trajectory(vec![]);
    let invocation = mock_invocation_with_response(&[], "actual response");

    let result = evaluator.evaluate(&case, &invocation).expect("result");
    assert_eq!(result.score.verdict(), Verdict::Fail);
    let details = result.details.expect("details");
    assert!(details.contains("dimension mismatch"));
}

#[test]
fn embedding_similarity_name_override_sticks() {
    let embedder: Arc<dyn Embedder> = Arc::new(StubEmbedder::new(vec![]));
    let evaluator =
        EmbeddingSimilarityEvaluator::new("ref", embedder).with_name("custom_embedding_sim");
    assert_eq!(evaluator.name(), "custom_embedding_sim");
}
