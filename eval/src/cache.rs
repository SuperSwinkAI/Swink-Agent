//! Eval runner cache abstractions.
//!
//! Spec 043-US2 / FR-038 / research §R-020. The runner caches agent
//! [`Invocation`]s keyed by SHA-256 of a canonical serialisation of
//! [`CaseFingerprint`] + [`FingerprintContext`]. `LocalFileTaskResultStore`
//! lays files out as `<root>/<eval_set_id>/<case_id>/<fingerprint_hex>.json`.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::types::{CaseFingerprint, Invocation};

/// Agent-side inputs that bind the cache key beyond the static case body.
#[derive(Debug, Clone, Default, Serialize)]
pub struct FingerprintContext {
    /// Initial `SessionState` JSON (`None` when no `initial_session_file`).
    pub initial_session: Option<serde_json::Value>,
    /// SHA-256 of agent tool names + schemas (lowercase hex).
    pub tool_set_hash: Option<String>,
    /// Model identifier, e.g. `"anthropic/claude-3-5-sonnet"`.
    pub agent_model: Option<String>,
}

#[derive(Debug, Serialize)]
struct CanonicalCacheInput<'a> {
    fingerprint: &'a CaseFingerprint,
    context: &'a FingerprintContext,
}

/// Stable SHA-256-backed cache key for one `(case, session, tools, model)` tuple.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CacheKey(String);

impl CacheKey {
    /// Construct from the canonical bytes of a `(fingerprint, context)` pair.
    #[must_use]
    pub fn from_fingerprint(fingerprint: &CaseFingerprint, context: &FingerprintContext) -> Self {
        Self::from_bytes(&canonicalize_fingerprint(fingerprint, context))
    }

    /// Construct from arbitrary canonical bytes.
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(hex_lower(&Sha256::digest(bytes)))
    }

    /// Lowercase hex encoding of the underlying SHA-256 digest.
    #[must_use]
    pub fn as_hex(&self) -> &str {
        &self.0
    }

    /// Consume the key and return its hex encoding.
    #[must_use]
    pub fn into_hex(self) -> String {
        self.0
    }
}

/// Canonical byte sequence hashed to form a [`CacheKey`]. `CaseFingerprint`
/// already canonicalises via `BTreeMap` + `CanonicalJsonValue`, so `serde_json`
/// output is stable across key-order permutations.
#[must_use]
pub fn canonicalize_fingerprint(
    fingerprint: &CaseFingerprint,
    context: &FingerprintContext,
) -> Vec<u8> {
    serde_json::to_vec(&CanonicalCacheInput {
        fingerprint,
        context,
    })
    .expect("CaseFingerprint + FingerprintContext always serialize")
}

/// SHA-256 of a tool-set (sorted by name, length-prefixed) producing the
/// `tool_set_hash` field of [`FingerprintContext`].
#[must_use]
pub fn tool_set_hash<'a, I>(tools: I) -> String
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let mut hasher = Sha256::new();
    let mut sorted: Vec<(&str, &str)> = tools.into_iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    for (name, schema) in sorted {
        hasher.update((name.len() as u64).to_le_bytes());
        hasher.update(name.as_bytes());
        hasher.update((schema.len() as u64).to_le_bytes());
        hasher.update(schema.as_bytes());
    }
    hex_lower(&hasher.finalize())
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

/// Structured errors returned by [`EvaluationDataStore`] implementations.
#[derive(Debug, Error)]
pub enum StoreError {
    /// Filesystem or IO error during persistence.
    #[error("store io error: {0}")]
    Io(String),
    /// Serialization or deserialization failure.
    #[error("store serde error: {0}")]
    Serde(String),
    /// An identifier (eval-set id, case id) contained illegal characters.
    #[error("invalid identifier: {0}")]
    InvalidIdentifier(String),
}

impl From<io::Error> for StoreError {
    fn from(err: io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(err: serde_json::Error) -> Self {
        Self::Serde(err.to_string())
    }
}

/// Pluggable persistence for agent invocations.
pub trait EvaluationDataStore: Send + Sync {
    /// Retrieve a cached [`Invocation`] or `Ok(None)` if absent.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if storage is unreachable or payload malformed.
    fn get(
        &self,
        eval_set_id: &str,
        case_id: &str,
        key: &CacheKey,
    ) -> Result<Option<Invocation>, StoreError>;

    /// Persist the given [`Invocation`], overwriting any prior value.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if storage is unreachable or payload
    /// unserialisable.
    fn put(
        &self,
        eval_set_id: &str,
        case_id: &str,
        key: &CacheKey,
        invocation: &Invocation,
    ) -> Result<(), StoreError>;
}

/// Filesystem-backed store. Layout: `<root>/<eval_set_id>/<case_id>/<hex>.json`.
pub struct LocalFileTaskResultStore {
    root: PathBuf,
}

impl LocalFileTaskResultStore {
    /// Create a new store rooted at the given directory.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Root directory of this store.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn case_dir(&self, eval_set_id: &str, case_id: &str) -> Result<PathBuf, StoreError> {
        validate_identifier(eval_set_id)?;
        validate_identifier(case_id)?;
        Ok(self.root.join(eval_set_id).join(case_id))
    }
}

impl EvaluationDataStore for LocalFileTaskResultStore {
    fn get(
        &self,
        eval_set_id: &str,
        case_id: &str,
        key: &CacheKey,
    ) -> Result<Option<Invocation>, StoreError> {
        let path = self
            .case_dir(eval_set_id, case_id)?
            .join(format!("{}.json", key.as_hex()));
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    fn put(
        &self,
        eval_set_id: &str,
        case_id: &str,
        key: &CacheKey,
        invocation: &Invocation,
    ) -> Result<(), StoreError> {
        let dir = self.case_dir(eval_set_id, case_id)?;
        fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.json", key.as_hex()));
        let bytes = serde_json::to_vec_pretty(invocation)?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, &bytes)?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }
}

fn validate_identifier(id: &str) -> Result<(), StoreError> {
    if id.is_empty() {
        return Err(StoreError::InvalidIdentifier(
            "identifier must not be empty".into(),
        ));
    }
    let path = Path::new(id);
    if path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        || id.contains(['/', '\\'])
    {
        return Err(StoreError::InvalidIdentifier(id.to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CanonicalJsonValue;

    fn fp(id: &str) -> CaseFingerprint {
        CaseFingerprint {
            id: id.into(),
            name: id.into(),
            description: None,
            system_prompt: "sp".into(),
            user_messages: vec!["hi".into()],
            expected_trajectory: None,
            expected_response: None,
            expected_assertion: None,
            expected_interactions: None,
            few_shot_examples: vec![],
            budget: None,
            evaluators: vec![],
            metadata: CanonicalJsonValue::Null,
            attachments: vec![],
            expected_environment_state: None,
            expected_tool_intent: None,
            semantic_tool_selection: false,
        }
    }

    #[test]
    fn cache_key_deterministic_and_context_sensitive() {
        let f = fp("c1");
        let empty = FingerprintContext::default();
        let a = CacheKey::from_fingerprint(&f, &empty);
        assert_eq!(a, CacheKey::from_fingerprint(&f, &empty));
        assert_eq!(a.as_hex().len(), 64);
        let b = CacheKey::from_fingerprint(
            &f,
            &FingerprintContext {
                initial_session: Some(serde_json::json!({"k": 1})),
                ..Default::default()
            },
        );
        assert_ne!(a, b);
    }

    #[test]
    fn tool_set_hash_is_order_independent() {
        assert_eq!(
            tool_set_hash([("a", "{}"), ("b", "{}")]),
            tool_set_hash([("b", "{}"), ("a", "{}")])
        );
        assert_ne!(
            tool_set_hash([("a", "{}")]),
            tool_set_hash([("a", "{}"), ("b", "{}")])
        );
    }

    #[test]
    fn validate_identifier_rejects_path_traversal() {
        assert!(validate_identifier("../evil").is_err());
        assert!(validate_identifier("a/b").is_err());
        assert!(validate_identifier("").is_err());
        assert!(validate_identifier("ok-id_1.0").is_ok());
    }
}
