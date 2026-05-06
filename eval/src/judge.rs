//! LLM-as-judge client trait and registry for semantic evaluators.
//!
//! `JudgeClient` is an async trait consumed by the semantic evaluators
//! (`SemanticToolSelectionEvaluator`, `SemanticToolParameterEvaluator`).
//!
//! # Scope: trait + test doubles only
//!
//! Spec 023 ships **only** the trait shape, the [`JudgeVerdict`] /
//! [`JudgeError`] types, and a pair of test doubles
//! ([`crate::MockJudge`], [`crate::SlowMockJudge`]). Concrete provider-backed
//! [`JudgeClient`] implementations — Anthropic, `OpenAI`, Gemini, Azure, local
//! models, prompt-template registries, retry / backoff, batching, caching —
//! are explicitly out of scope and will ship in spec 043
//! (`specs/043-evals-adv-features`) in the companion
//! `swink-agent-eval-judges` crate. Until then, downstream consumers wire up
//! their own [`JudgeClient`] impl (often a thin wrapper around an existing
//! `swink-agent` provider handle) and pass it to
//! [`crate::EvaluatorRegistry::with_defaults_and_judge`].
//!
//! # Non-hang guarantee
//!
//! The trait has no deadline parameter; deadline enforcement is evaluator-side
//! via `tokio::time::timeout` (default 5 min, configurable per evaluator).
//! See `FR-010` and `FR-014` in `specs/023-eval-trajectory-matching/spec.md`.
//!
//! # Error mapping
//!
//! All [`JudgeError`] variants map to `Score::fail()` with the variant name
//! and context captured in `EvalMetricResult.details`.

#![forbid(unsafe_code)]

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::fs;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

pub use crate::url_filter::{DefaultUrlFilter, UrlFilter};

/// Maximum retry attempts allowed for judge dispatch.
pub const MAX_RETRY_ATTEMPTS: u32 = 16;

/// Maximum supported judge batch size.
pub const MAX_BATCH_SIZE: usize = 128;

/// Default number of judge verdicts retained in memory.
pub const DEFAULT_JUDGE_CACHE_CAPACITY: usize = 1024;

#[cfg(feature = "judge-core")]
thread_local! {
    static SCOPED_JUDGE_CANCELLATION: std::cell::RefCell<Vec<CancellationToken>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(feature = "judge-core")]
pub(crate) fn with_scoped_judge_cancellation<T>(
    cancellation: Option<&CancellationToken>,
    run: impl FnOnce() -> T,
) -> T {
    let Some(cancellation) = cancellation else {
        return run();
    };

    SCOPED_JUDGE_CANCELLATION.with(|stack| stack.borrow_mut().push(cancellation.clone()));
    let _guard = ScopedJudgeCancellationGuard;
    run()
}

#[cfg(feature = "judge-core")]
pub(crate) fn scoped_judge_cancellation() -> Option<CancellationToken> {
    SCOPED_JUDGE_CANCELLATION.with(|stack| stack.borrow().last().cloned())
}

#[cfg(feature = "judge-core")]
struct ScopedJudgeCancellationGuard;

#[cfg(feature = "judge-core")]
impl Drop for ScopedJudgeCancellationGuard {
    fn drop(&mut self) {
        SCOPED_JUDGE_CANCELLATION.with(|stack| {
            stack.borrow_mut().pop();
        });
    }
}

/// LLM-as-judge client used by semantic evaluators.
///
/// The trait exposes a single async method that accepts a rendered prompt and
/// returns a structured [`JudgeVerdict`]. Concrete implementations (model
/// providers, prompt templating, retry / backoff, batching) are explicitly out
/// of scope for spec 023.
pub trait JudgeClient: Send + Sync {
    /// Judge the given prompt and return a structured verdict.
    ///
    /// Implementations MAY enforce their own inner deadlines and surface
    /// them via [`JudgeError::Timeout`]. Evaluators additionally wrap each
    /// call in an outer `tokio::time::timeout`.
    fn judge<'a>(&'a self, prompt: &'a str) -> JudgeFuture<'a>;
}

/// Object-safe future returned by [`JudgeClient::judge`].
pub type JudgeFuture<'a> =
    Pin<Box<dyn Future<Output = Result<JudgeVerdict, JudgeError>> + Send + 'a>>;

/// Retry configuration shared by judge-backed evaluators.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Maximum number of attempts for one judge request.
    pub max_attempts: u32,
    /// Maximum backoff delay between attempts.
    pub max_delay: Duration,
    /// Whether retry jitter is enabled.
    pub jitter: bool,
}

/// Stable cache key for a judge prompt/model pair.
///
/// The digest is SHA-256 over the model identifier and rendered prompt, with
/// length prefixes to avoid accidental concatenation ambiguity.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct CacheKey([u8; 32]);

impl CacheKey {
    /// Derive a cache key from the explicit judge model identifier and prompt.
    #[must_use]
    pub fn for_prompt(model_id: &str, prompt: &str) -> Self {
        let mut hasher = Sha256::new();
        update_with_len_prefixed_bytes(&mut hasher, model_id.as_bytes());
        update_with_len_prefixed_bytes(&mut hasher, prompt.as_bytes());
        Self(hasher.finalize().into())
    }

    /// Borrow the raw SHA-256 digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    fn to_hex(self) -> String {
        hex_lower(self.as_bytes())
    }
}

impl fmt::Debug for CacheKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("CacheKey")
            .field(&hex_lower(self.as_bytes()))
            .finish()
    }
}

/// In-memory LRU cache for judge verdicts.
///
/// This cache stores structured judge verdicts by [`CacheKey`]. It is small and
/// synchronous by design; callers that share it across tasks should wrap it in
/// their preferred synchronization primitive.
#[derive(Debug)]
pub struct JudgeCache {
    capacity: usize,
    entries: HashMap<CacheKey, JudgeVerdict>,
    recency: VecDeque<CacheKey>,
    disk_path: Option<PathBuf>,
    dirty: bool,
}

impl JudgeCache {
    /// Construct an empty cache with the default capacity.
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_JUDGE_CACHE_CAPACITY)
    }

    /// Construct an empty cache with a bounded capacity.
    ///
    /// A requested capacity of `0` is promoted to `1` so inserts remain useful
    /// and eviction semantics stay well-defined.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: HashMap::new(),
            recency: VecDeque::new(),
            disk_path: None,
            dirty: false,
        }
    }

    /// Construct a disk-backed cache, warm-loading any persisted entries.
    ///
    /// Entries are stored as one JSON file per [`CacheKey`] and flushed on
    /// [`Self::flush_to_disk`] or drop. Invalid cache files are ignored so a
    /// corrupt optional cache never prevents evaluation startup.
    pub fn with_disk_path(capacity: usize, path: impl Into<PathBuf>) -> io::Result<Self> {
        let disk_path = path.into();
        fs::create_dir_all(&disk_path)?;

        let mut cache = Self {
            capacity: capacity.max(1),
            entries: HashMap::new(),
            recency: VecDeque::new(),
            disk_path: Some(disk_path.clone()),
            dirty: false,
        };

        let mut files = fs::read_dir(&disk_path)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
            .collect::<Vec<_>>();
        files.sort();

        for path in files {
            let Some(key) = cache_key_from_path(&path) else {
                continue;
            };
            let Ok(bytes) = fs::read(&path) else {
                continue;
            };
            let Ok(verdict) = serde_json::from_slice::<JudgeVerdict>(&bytes) else {
                continue;
            };
            cache.put_loaded(key, verdict);
        }

        cache.dirty = false;
        Ok(cache)
    }

    /// Number of verdicts currently cached.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache currently stores no verdicts.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Maximum number of verdicts retained.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Return the configured disk cache directory, when disk-backed.
    #[must_use]
    pub fn disk_path(&self) -> Option<&Path> {
        self.disk_path.as_deref()
    }

    /// Retrieve a cached verdict and mark it most recently used.
    pub fn get(&mut self, key: &CacheKey) -> Option<JudgeVerdict> {
        let verdict = self.entries.get(key).cloned();
        if verdict.is_some() {
            self.touch(*key);
        }
        verdict
    }

    /// Insert or replace a cached verdict.
    pub fn put(&mut self, key: CacheKey, verdict: JudgeVerdict) {
        let replacing = self.entries.insert(key, verdict).is_some();
        self.touch(key);
        self.dirty = true;

        if !replacing {
            self.evict_over_capacity();
        }
    }

    /// Flush current entries to disk, removing stale persisted entries.
    pub fn flush_to_disk(&mut self) -> io::Result<()> {
        let Some(path) = self.disk_path.as_ref() else {
            self.dirty = false;
            return Ok(());
        };

        fs::create_dir_all(path)?;
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let remove =
                cache_key_from_path(&path).is_some_and(|key| !self.entries.contains_key(&key));
            if remove {
                fs::remove_file(path)?;
            }
        }

        for (key, verdict) in &self.entries {
            let path = path.join(format!("{}.json", key.to_hex()));
            let bytes = serde_json::to_vec(verdict).map_err(io::Error::other)?;
            fs::write(path, bytes)?;
        }

        self.dirty = false;
        Ok(())
    }

    fn put_loaded(&mut self, key: CacheKey, verdict: JudgeVerdict) {
        let replacing = self.entries.insert(key, verdict).is_some();
        self.touch(key);

        if !replacing {
            self.evict_over_capacity();
        }
    }

    fn touch(&mut self, key: CacheKey) {
        self.recency.retain(|candidate| candidate != &key);
        self.recency.push_back(key);
    }

    fn evict_over_capacity(&mut self) {
        while self.entries.len() > self.capacity {
            if let Some(oldest) = self.recency.pop_front() {
                self.entries.remove(&oldest);
            } else {
                break;
            }
        }
    }
}

impl Default for JudgeCache {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for JudgeCache {
    fn drop(&mut self) {
        if self.dirty {
            let _ = self.flush_to_disk();
        }
    }
}

fn update_with_len_prefixed_bytes(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update(bytes.len().to_le_bytes());
    hasher.update(bytes);
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

fn cache_key_from_path(path: &Path) -> Option<CacheKey> {
    let stem = path.file_stem()?.to_str()?;
    cache_key_from_hex(stem)
}

fn cache_key_from_hex(hex: &str) -> Option<CacheKey> {
    if hex.len() != 64 {
        return None;
    }

    let mut bytes = [0_u8; 32];
    let raw = hex.as_bytes();
    for (idx, byte) in bytes.iter_mut().enumerate() {
        let high = hex_nibble(raw[idx * 2])?;
        let low = hex_nibble(raw[idx * 2 + 1])?;
        *byte = (high << 4) | low;
    }

    Some(CacheKey(bytes))
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

impl RetryPolicy {
    /// Create a retry policy with explicit values.
    #[must_use]
    pub const fn new(max_attempts: u32, max_delay: Duration, jitter: bool) -> Self {
        Self {
            max_attempts,
            max_delay,
            jitter,
        }
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 6,
            max_delay: Duration::from_secs(240),
            jitter: true,
        }
    }
}

/// Configuration binding a judge client to a required model identifier.
///
/// This registry intentionally does not provide a default model. Provider model
/// lineups change over time, so callers must make model identity explicit at
/// construction to preserve score comparability.
pub struct JudgeRegistry {
    client: Arc<dyn JudgeClient>,
    model_id: String,
    retry_policy: RetryPolicy,
    cancellation: Option<CancellationToken>,
    batch_size: usize,
    url_filter: Arc<dyn UrlFilter>,
}

impl std::fmt::Debug for JudgeRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JudgeRegistry")
            .field("model_id", &self.model_id)
            .field("retry_policy", &self.retry_policy)
            .field("cancellation", &self.cancellation.is_some())
            .field("batch_size", &self.batch_size)
            .finish_non_exhaustive()
    }
}

impl JudgeRegistry {
    /// Start building a judge registry for an explicit model identifier.
    #[must_use]
    pub fn builder(
        client: Arc<dyn JudgeClient>,
        model_id: impl Into<String>,
    ) -> JudgeRegistryBuilder {
        JudgeRegistryBuilder {
            client,
            model_id: model_id.into(),
            retry_policy: RetryPolicy::default(),
            cancellation: None,
            batch_size: 1,
            url_filter: Arc::new(DefaultUrlFilter),
        }
    }

    /// Borrow the configured judge client.
    #[must_use]
    pub fn client(&self) -> &Arc<dyn JudgeClient> {
        &self.client
    }

    /// Return the explicit judge model identifier.
    #[must_use]
    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    /// Return the retry policy used by judge-backed evaluators.
    #[must_use]
    pub const fn retry_policy(&self) -> &RetryPolicy {
        &self.retry_policy
    }

    /// Return the optional cancellation token used by judge-backed evaluators.
    #[must_use]
    pub const fn cancellation(&self) -> Option<&CancellationToken> {
        self.cancellation.as_ref()
    }

    /// Return the bounded batch size for judge dispatch.
    #[must_use]
    pub const fn batch_size(&self) -> usize {
        self.batch_size
    }

    /// Borrow the URL filter used when materializing judge attachments.
    #[must_use]
    pub fn url_filter(&self) -> &Arc<dyn UrlFilter> {
        &self.url_filter
    }
}

/// Builder for [`JudgeRegistry`].
pub struct JudgeRegistryBuilder {
    client: Arc<dyn JudgeClient>,
    model_id: String,
    retry_policy: RetryPolicy,
    cancellation: Option<CancellationToken>,
    batch_size: usize,
    url_filter: Arc<dyn UrlFilter>,
}

impl JudgeRegistryBuilder {
    /// Override retry behavior.
    #[must_use]
    pub fn with_retry_policy(mut self, retry_policy: RetryPolicy) -> Self {
        self.retry_policy = retry_policy;
        self
    }

    /// Attach a cancellation token observed by judge-backed evaluator dispatch.
    #[must_use]
    pub fn with_cancellation(mut self, cancellation: CancellationToken) -> Self {
        self.cancellation = Some(cancellation);
        self
    }

    /// Override the bounded judge batch size.
    #[must_use]
    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size;
        self
    }

    /// Override the URL filter used by attachment materialization.
    #[must_use]
    pub fn with_url_filter(mut self, url_filter: Arc<dyn UrlFilter>) -> Self {
        self.url_filter = url_filter;
        self
    }

    /// Validate and construct the registry.
    pub fn build(self) -> Result<JudgeRegistry, JudgeRegistryError> {
        let model_id = self.model_id.trim().to_string();
        if model_id.is_empty() {
            return Err(JudgeRegistryError::MissingModelId);
        }
        if !(1..=MAX_BATCH_SIZE).contains(&self.batch_size) {
            return Err(JudgeRegistryError::InvalidBatchSize {
                batch_size: self.batch_size,
            });
        }
        if self.retry_policy.max_attempts > MAX_RETRY_ATTEMPTS {
            return Err(JudgeRegistryError::InvalidRetryPolicy {
                reason: format!(
                    "max_attempts must be <= {MAX_RETRY_ATTEMPTS}, got {}",
                    self.retry_policy.max_attempts
                ),
            });
        }
        if self.retry_policy.max_attempts == 0 {
            return Err(JudgeRegistryError::InvalidRetryPolicy {
                reason: "max_attempts must be greater than 0".to_string(),
            });
        }

        Ok(JudgeRegistry {
            client: self.client,
            model_id,
            retry_policy: self.retry_policy,
            cancellation: self.cancellation,
            batch_size: self.batch_size,
            url_filter: self.url_filter,
        })
    }
}

/// Errors returned while constructing a [`JudgeRegistry`].
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum JudgeRegistryError {
    /// No explicit model identifier was provided.
    #[error("judge registry requires an explicit model_id")]
    MissingModelId,
    /// Batch size was outside the supported `[1, 128]` range.
    #[error("judge batch_size must be in 1..={MAX_BATCH_SIZE}, got {batch_size}")]
    InvalidBatchSize { batch_size: usize },
    /// Retry policy is outside the supported bounds.
    #[error("invalid judge retry policy: {reason}")]
    InvalidRetryPolicy { reason: String },
}

/// Structured verdict returned by a [`JudgeClient`].
///
/// Mirrors `strands-evals::EvaluationOutput` so future provider bindings can
/// map cleanly without provider-specific types leaking into this crate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JudgeVerdict {
    /// Numeric score in `[0.0, 1.0]`. Callers SHOULD clamp before constructing.
    pub score: f64,
    /// Judge's own pass/fail determination; evaluators surface this directly
    /// in the `EvalMetricResult`.
    pub pass: bool,
    /// Human-readable justification, surfaced in `EvalMetricResult.details`.
    pub reason: Option<String>,
    /// Optional category label (e.g., "equivalent", "unrelated").
    pub label: Option<String>,
}

/// Error type returned by [`JudgeClient::judge`].
///
/// Semantic evaluators map every variant to `Score::fail()` with the variant
/// name and context captured in `EvalMetricResult.details` (FR-014).
#[derive(Debug, thiserror::Error)]
pub enum JudgeError {
    /// Network or transport failure (connection refused, DNS error, etc.).
    #[error("transport: {0}")]
    Transport(String),
    /// An inner deadline fired inside the concrete [`JudgeClient`] impl.
    #[error("timeout")]
    Timeout,
    /// Response parsed successfully but violates the verdict schema.
    #[error("malformed response: {0}")]
    MalformedResponse(String),
    /// Catch-all with a diagnostic string for anything outside the above.
    #[error("other: {0}")]
    Other(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn judge_error_display_variants() {
        assert_eq!(
            JudgeError::Transport("boom".into()).to_string(),
            "transport: boom"
        );
        assert_eq!(JudgeError::Timeout.to_string(), "timeout");
        assert_eq!(
            JudgeError::MalformedResponse("bad".into()).to_string(),
            "malformed response: bad"
        );
        assert_eq!(
            JudgeError::Other("thing".into()).to_string(),
            "other: thing"
        );
    }

    #[test]
    fn verdict_fields_are_public() {
        let v = JudgeVerdict {
            score: 0.75,
            pass: true,
            reason: Some("looks right".into()),
            label: Some("equivalent".into()),
        };
        assert!((v.score - 0.75).abs() < f64::EPSILON);
        assert!(v.pass);
        assert_eq!(v.reason.as_deref(), Some("looks right"));
        assert_eq!(v.label.as_deref(), Some("equivalent"));
    }
}
