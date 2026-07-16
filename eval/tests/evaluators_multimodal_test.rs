//! Regression tests for the Multimodal family evaluator (T084).

#![cfg(feature = "multimodal")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use swink_agent::{ModelSpec, StopReason};
use swink_agent_eval::{
    Attachment, DefaultUrlFilter, EvalCase, Evaluator, ImageSafetyEvaluator, Invocation,
    JudgeClient, JudgeEvaluatorConfig, JudgeRegistry, JudgeVerdict, materialize_case_attachments,
};

struct FixedVerdict {
    score: f64,
    pass: bool,
    reason: Option<String>,
    last_prompt: Mutex<Option<String>>,
}

impl JudgeClient for FixedVerdict {
    fn judge<'a>(&'a self, prompt: &'a str) -> swink_agent_eval::JudgeFuture<'a> {
        Box::pin(async move {
            *self.last_prompt.lock().unwrap() = Some(prompt.to_string());
            let mut verdict = JudgeVerdict::new(self.score, self.pass);
            verdict.reason = self.reason.clone();
            Ok(verdict)
        })
    }
}

fn make_case_with_attachments(attachments: Vec<Attachment>) -> EvalCase {
    EvalCase::new("case", "Case", "s", vec!["check the image".into()]).with_attachments(attachments)
}

fn make_invocation() -> Invocation {
    Invocation::new(StopReason::Stop, ModelSpec::new("test", "m"))
        .with_total_duration(Duration::from_millis(1))
        .with_final_response("ok")
}

fn make_config(judge: Arc<dyn JudgeClient>) -> JudgeEvaluatorConfig {
    let registry = JudgeRegistry::builder(judge, "mock-model")
        .build()
        .expect("registry builds");
    JudgeEvaluatorConfig::default_with(Arc::new(registry))
}

#[test]
fn image_safety_returns_none_without_attachments() {
    let judge: Arc<dyn JudgeClient> = Arc::new(FixedVerdict {
        score: 1.0,
        pass: true,
        reason: None,
        last_prompt: Mutex::new(None),
    });
    let evaluator = ImageSafetyEvaluator::new(make_config(judge), std::env::temp_dir());
    let case = make_case_with_attachments(vec![]);
    assert!(evaluator.evaluate(&case, &make_invocation()).is_none());
}

// Tiny 1x1 PNG (binary for a valid PNG header).
const TINY_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // signature
    0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
    0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41,
    0x54, 0x78, 0x9C, 0x62, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00,
    0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

#[tokio::test]
async fn materialize_case_attachments_decodes_base64_variant() {
    let attachment = Attachment::Base64 {
        mime: "image/png".into(),
        bytes: TINY_PNG.to_vec(),
    };
    let case = make_case_with_attachments(vec![attachment]);
    let filter = DefaultUrlFilter;
    let materialized = materialize_case_attachments(&case, std::path::Path::new("."), &filter)
        .await
        .expect("materialize succeeds");
    assert_eq!(materialized.len(), 1);
    assert_eq!(materialized[0].mime, "image/png");
    assert_eq!(materialized[0].bytes, TINY_PNG);
}

#[test]
fn image_safety_happy_path_pass_verdict_surfaces_score() {
    let judge: Arc<dyn JudgeClient> = Arc::new(FixedVerdict {
        score: 1.0,
        pass: true,
        reason: Some("safe".into()),
        last_prompt: Mutex::new(None),
    });
    let evaluator = ImageSafetyEvaluator::new(make_config(judge), std::env::temp_dir());
    let case = make_case_with_attachments(vec![Attachment::Base64 {
        mime: "image/png".into(),
        bytes: TINY_PNG.to_vec(),
    }]);
    let result = evaluator.evaluate(&case, &make_invocation()).unwrap();
    assert!(result.score.verdict().is_pass());
}

#[test]
fn image_safety_deny_path_reports_failure() {
    let judge: Arc<dyn JudgeClient> = Arc::new(FixedVerdict {
        score: 0.0,
        pass: false,
        reason: Some("explicit content".into()),
        last_prompt: Mutex::new(None),
    });
    let evaluator = ImageSafetyEvaluator::new(make_config(judge), std::env::temp_dir());
    let case = make_case_with_attachments(vec![Attachment::Base64 {
        mime: "image/png".into(),
        bytes: TINY_PNG.to_vec(),
    }]);
    let result = evaluator.evaluate(&case, &make_invocation()).unwrap();
    assert!(!result.score.verdict().is_pass());
}
