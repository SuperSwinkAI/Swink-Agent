use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::Mutex;

use swink_agent::{
    ArtifactData, ArtifactError, ArtifactMeta, ArtifactStore, ArtifactVersion,
    validate_artifact_name,
};

type VersionList = Vec<(ArtifactVersion, ArtifactData)>;
type SessionMap = HashMap<String, HashMap<String, VersionList>>;

/// In-memory artifact store for testing and lightweight use.
///
/// All data lives in heap memory. Not persisted across process restarts.
pub struct InMemoryArtifactStore {
    data: Arc<Mutex<SessionMap>>,
}

impl InMemoryArtifactStore {
    /// Create a new empty in-memory store.
    pub fn new() -> Self {
        Self {
            data: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Default for InMemoryArtifactStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ArtifactStore for InMemoryArtifactStore {
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

        tracing::debug!(
            session_id,
            name,
            version = next_version,
            size = data.content.len(),
            "artifact saved"
        );

        versions.push((version.clone(), data));
        drop(store);
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
        drop(store);
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
        drop(store);
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

        drop(store);
        Ok(metas)
    }

    async fn delete(&self, session_id: &str, name: &str) -> Result<(), ArtifactError> {
        let mut store = self.data.lock().await;
        if let Some(session) = store.get_mut(session_id) {
            session.remove(name);
        }
        drop(store);
        Ok(())
    }
}
