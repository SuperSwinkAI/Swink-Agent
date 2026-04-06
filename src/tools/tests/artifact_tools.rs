//! Tests for the built-in artifact tools (save, load, list).

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use serde_json::json;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::SessionState;
use crate::artifact::{
    ArtifactData, ArtifactError, ArtifactMeta, ArtifactStore, ArtifactVersion,
    validate_artifact_name,
};
use crate::tool::{AgentTool, AgentToolResult};
use crate::types::ContentBlock;

// ─── Mock Store ────────────────────────────────────────────────────────────

type VersionList = Vec<(ArtifactVersion, ArtifactData)>;
type SessionMap = HashMap<String, HashMap<String, VersionList>>;

struct MockArtifactStore {
    data: Mutex<SessionMap>,
}

impl MockArtifactStore {
    fn new() -> Self {
        Self {
            data: Mutex::new(HashMap::new()),
        }
    }
}

impl ArtifactStore for MockArtifactStore {
    async fn save(
        &self,
        session_id: &str,
        name: &str,
        data: ArtifactData,
    ) -> Result<ArtifactVersion, ArtifactError> {
        validate_artifact_name(name)?;
        let mut store = self.data.lock().await;
        let session = store.entry(session_id.to_string()).or_default();
        let versions = session.entry(name.to_string()).or_default();

        #[allow(clippy::cast_possible_truncation)]
        let next_version = versions.len() as u32 + 1;
        let version = ArtifactVersion {
            name: name.to_string(),
            version: next_version,
            created_at: Utc::now(),
            size: data.content.len(),
            content_type: data.content_type.clone(),
        };
        versions.push((version.clone(), data));
        Ok(version)
    }

    async fn load(
        &self,
        session_id: &str,
        name: &str,
    ) -> Result<Option<(ArtifactData, ArtifactVersion)>, ArtifactError> {
        let store = self.data.lock().await;
        let result = store
            .get(session_id)
            .and_then(|session| session.get(name))
            .and_then(|versions| versions.last())
            .map(|(v, d)| (d.clone(), v.clone()));
        Ok(result)
    }

    async fn load_version(
        &self,
        session_id: &str,
        name: &str,
        version: u32,
    ) -> Result<Option<(ArtifactData, ArtifactVersion)>, ArtifactError> {
        let store = self.data.lock().await;
        let result = store
            .get(session_id)
            .and_then(|session| session.get(name))
            .and_then(|versions| {
                versions
                    .iter()
                    .find(|(v, _)| v.version == version)
                    .map(|(v, d)| (d.clone(), v.clone()))
            });
        Ok(result)
    }

    async fn list(&self, session_id: &str) -> Result<Vec<ArtifactMeta>, ArtifactError> {
        let store = self.data.lock().await;
        let Some(session) = store.get(session_id) else {
            return Ok(Vec::new());
        };
        let mut metas = Vec::with_capacity(session.len());
        for (name, versions) in session {
            if let (Some(first), Some(last)) = (versions.first(), versions.last()) {
                metas.push(ArtifactMeta {
                    name: name.clone(),
                    latest_version: last.0.version,
                    created_at: first.0.created_at,
                    updated_at: last.0.created_at,
                    content_type: last.0.content_type.clone(),
                });
            }
        }
        Ok(metas)
    }

    async fn delete(&self, session_id: &str, name: &str) -> Result<(), ArtifactError> {
        let mut store = self.data.lock().await;
        if let Some(session) = store.get_mut(session_id) {
            session.remove(name);
        }
        Ok(())
    }
}

// ─── Helpers ───────────────────────────────────────────────────────────────

fn make_state(session_id: &str) -> Arc<std::sync::RwLock<SessionState>> {
    let mut state = SessionState::new();
    state.set("session_id", session_id.to_string());
    Arc::new(std::sync::RwLock::new(state))
}

fn cancel_token() -> CancellationToken {
    CancellationToken::new()
}

/// Extract the text from the first content block of an `AgentToolResult`.
fn result_text(result: &AgentToolResult) -> &str {
    match result.content.first() {
        Some(ContentBlock::Text { text }) => text.as_str(),
        _ => panic!("expected Text content block, got: {:?}", result.content),
    }
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn save_artifact_tool_creates_version() {
    use super::super::SaveArtifactTool;

    let store = Arc::new(MockArtifactStore::new());
    let tool = SaveArtifactTool::new(store.clone());
    let state = make_state("sess-1");

    let result = tool
        .execute(
            "call-1",
            json!({"name": "report.md", "content": "# Hello"}),
            cancel_token(),
            None,
            state,
            None,
        )
        .await;

    assert!(!result.is_error);
    let text = result_text(&result);
    assert!(text.contains("Saved"), "expected 'Saved' in: {text}");
    assert!(
        text.contains("'report.md'"),
        "expected artifact name in: {text}"
    );
    assert!(text.contains("version 1"), "expected version 1 in: {text}");
}

#[tokio::test]
async fn load_artifact_tool_returns_text_content() {
    use super::super::{LoadArtifactTool, SaveArtifactTool};

    let store = Arc::new(MockArtifactStore::new());
    let state = make_state("sess-2");

    // Save first.
    let save_tool = SaveArtifactTool::new(store.clone());
    let _ = save_tool
        .execute(
            "c1",
            json!({"name": "notes.txt", "content": "some notes here"}),
            cancel_token(),
            None,
            state.clone(),
            None,
        )
        .await;

    // Load.
    let load_tool = LoadArtifactTool::new(store);
    let result = load_tool
        .execute(
            "c2",
            json!({"name": "notes.txt"}),
            cancel_token(),
            None,
            state,
            None,
        )
        .await;

    assert!(!result.is_error);
    assert_eq!(result_text(&result), "some notes here");
}

#[tokio::test]
async fn load_artifact_tool_returns_binary_summary() {
    use super::super::LoadArtifactTool;

    let store = Arc::new(MockArtifactStore::new());
    let state = make_state("sess-3");

    // Directly save binary content via store.
    let data = ArtifactData {
        content: vec![0x89, 0x50, 0x4E, 0x47], // PNG magic bytes
        content_type: "image/png".to_string(),
        metadata: HashMap::new(),
    };
    store.save("sess-3", "photo.png", data).await.unwrap();

    let load_tool = LoadArtifactTool::new(store);
    let result = load_tool
        .execute(
            "c3",
            json!({"name": "photo.png"}),
            cancel_token(),
            None,
            state,
            None,
        )
        .await;

    assert!(!result.is_error);
    let text = result_text(&result);
    assert!(
        text.contains("[binary:"),
        "expected binary marker in: {text}"
    );
    assert!(text.contains("4 bytes"), "expected size in: {text}");
    assert!(
        text.contains("image/png"),
        "expected content type in: {text}"
    );
}

#[tokio::test]
async fn list_artifacts_tool_returns_formatted_list() {
    use super::super::{ListArtifactsTool, SaveArtifactTool};

    let store = Arc::new(MockArtifactStore::new());
    let state = make_state("sess-4");

    let save_tool = SaveArtifactTool::new(store.clone());

    // Save two artifacts.
    let _ = save_tool
        .execute(
            "c1",
            json!({"name": "report.md", "content": "# Report"}),
            cancel_token(),
            None,
            state.clone(),
            None,
        )
        .await;
    let _ = save_tool
        .execute(
            "c2",
            json!({"name": "data.csv", "content": "a,b,c", "content_type": "text/csv"}),
            cancel_token(),
            None,
            state.clone(),
            None,
        )
        .await;

    let list_tool = ListArtifactsTool::new(store);
    let result = list_tool
        .execute("c3", json!({}), cancel_token(), None, state, None)
        .await;

    assert!(!result.is_error);
    let text = result_text(&result);
    assert!(text.starts_with("Artifacts:"), "expected header in: {text}");
    assert!(text.contains("report.md"), "expected report.md in: {text}");
    assert!(text.contains("data.csv"), "expected data.csv in: {text}");
}

#[tokio::test]
async fn list_artifacts_tool_empty_session() {
    use super::super::ListArtifactsTool;

    let store = Arc::new(MockArtifactStore::new());
    let state = make_state("sess-5");

    let list_tool = ListArtifactsTool::new(store);
    let result = list_tool
        .execute("c1", json!({}), cancel_token(), None, state, None)
        .await;

    assert!(!result.is_error);
    assert_eq!(result_text(&result), "No artifacts in this session.");
}

#[test]
fn artifact_tools_convenience_constructor() {
    use super::super::artifact_tools;

    let store = Arc::new(MockArtifactStore::new());
    let tools = artifact_tools(store);

    assert_eq!(tools.len(), 3);
    let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
    assert!(names.contains(&"save_artifact"));
    assert!(names.contains(&"load_artifact"));
    assert!(names.contains(&"list_artifacts"));
}
