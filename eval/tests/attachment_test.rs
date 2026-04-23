use std::path::PathBuf;

use swink_agent_eval::{
    Attachment, AttachmentError, DefaultUrlFilter, EvalCase, ResponseCriteria, UrlFilter,
};
use tempfile::tempdir;
use url::Url;

struct AllowAllUrls;

impl UrlFilter for AllowAllUrls {
    fn allows(&self, _url: &Url) -> bool {
        true
    }
}

fn base_case(id: &str) -> EvalCase {
    EvalCase {
        id: id.to_string(),
        name: id.to_string(),
        description: None,
        system_prompt: String::new(),
        user_messages: vec!["look at the image".to_string()],
        expected_trajectory: None,
        expected_response: Some(ResponseCriteria::Contains {
            substring: "safe".to_string(),
        }),
        expected_assertion: None,
        expected_interactions: None,
        few_shot_examples: vec![],
        budget: None,
        evaluators: vec![],
        metadata: serde_json::Value::Null,
        attachments: vec![],
        session_id: None,
        expected_environment_state: None,
        expected_tool_intent: None,
        semantic_tool_selection: false,
        state_capture: None,
    }
}

#[tokio::test]
async fn materializes_path_attachment_relative_to_eval_root() {
    let root = tempdir().unwrap();
    let image_path = root.path().join("fixtures").join("image.png");
    std::fs::create_dir_all(image_path.parent().unwrap()).unwrap();
    std::fs::write(&image_path, b"png bytes").unwrap();

    let attachment = Attachment::Path(PathBuf::from("fixtures/image.png"));
    let materialized = attachment
        .materialize(root.path(), &DefaultUrlFilter)
        .await
        .unwrap();

    assert_eq!(materialized.mime, "image/png");
    assert_eq!(materialized.bytes, b"png bytes");
}

#[tokio::test]
async fn materializes_base64_attachment_with_supported_mime() {
    let root = tempdir().unwrap();
    let attachment = Attachment::Base64 {
        mime: "IMAGE/JPEG; charset=binary".to_string(),
        bytes: b"jpeg bytes".to_vec(),
    };

    let materialized = attachment
        .materialize(root.path(), &DefaultUrlFilter)
        .await
        .unwrap();

    assert_eq!(materialized.mime, "image/jpeg");
    assert_eq!(materialized.bytes, b"jpeg bytes");
}

#[tokio::test]
async fn url_attachment_requires_https_before_fetch() {
    let root = tempdir().unwrap();
    let attachment = Attachment::Url("http://example.com/image.png".to_string());

    let err = attachment
        .materialize(root.path(), &AllowAllUrls)
        .await
        .expect_err("http URLs should be blocked");

    assert!(matches!(err, AttachmentError::UrlBlocked { .. }));
}

#[tokio::test]
async fn url_attachment_applies_ssrf_filter() {
    let root = tempdir().unwrap();
    let attachment = Attachment::Url("https://127.0.0.1/image.png".to_string());

    let err = attachment
        .materialize(root.path(), &DefaultUrlFilter)
        .await
        .expect_err("loopback URL should be blocked");

    assert!(matches!(err, AttachmentError::UrlBlocked { .. }));
}

#[tokio::test]
async fn unsupported_mime_is_structured_error() {
    let root = tempdir().unwrap();
    let attachment = Attachment::Base64 {
        mime: "application/pdf".to_string(),
        bytes: b"pdf bytes".to_vec(),
    };

    let err = attachment
        .materialize(root.path(), &DefaultUrlFilter)
        .await
        .expect_err("pdf is not a supported multimodal image MIME");

    assert!(matches!(
        err,
        AttachmentError::UnsupportedMime { mime } if mime == "application/pdf"
    ));
}

#[test]
fn eval_case_serde_defaults_attachments_for_existing_cases() {
    let json = r#"{
        "id": "legacy",
        "name": "Legacy",
        "system_prompt": "",
        "user_messages": ["hi"]
    }"#;

    let case: EvalCase = serde_json::from_str(json).unwrap();

    assert!(case.attachments.is_empty());
}

#[test]
fn eval_case_serde_round_trips_attachments() {
    let mut case = base_case("with-attachments");
    case.attachments = vec![
        Attachment::Path(PathBuf::from("image.png")),
        Attachment::Base64 {
            mime: "image/webp".to_string(),
            bytes: vec![1, 2, 3],
        },
        Attachment::Url("https://example.com/image.gif".to_string()),
    ];

    let json = serde_json::to_string(&case).unwrap();
    let back: EvalCase = serde_json::from_str(&json).unwrap();

    assert_eq!(back.attachments, case.attachments);
}
