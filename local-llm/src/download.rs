//! Model weight download from the Hugging Face Hub.
//!
//! Speaks the Hub's `resolve` endpoint directly on the workspace's
//! reqwest + rustls(ring) stack and reads/writes the `huggingface_hub` cache
//! layout, so weights already fetched by other tools (or by earlier releases
//! through `hf-hub`) are reused without a re-download:
//!
//! ```text
//! {cache}/models--{owner}--{name}/
//!   blobs/{etag}                 the bytes
//!   snapshots/{commit}/{file}    pointer (symlink; a copy on Windows)
//!   refs/{revision}              commit hash the revision resolved to
//! ```
//!
//! `hf-hub` 1.0 was dropped because it hard-depends on `hf-xet`, whose default
//! feature hardwires `aws-lc-rs` — the last C build in the graph after the
//! workspace moved to `rustls-no-provider` + ring. Environment conventions
//! (`HF_ENDPOINT`, `HF_HOME`, `HF_HUB_CACHE`, `HF_TOKEN`, `HF_TOKEN_PATH`,
//! `~/.cache/huggingface/token`) are kept so existing setups keep working.

use std::path::{Component, Path, PathBuf};

use futures::StreamExt as _;
use reqwest::header::{
    AUTHORIZATION, CONTENT_LENGTH, ETAG, HeaderMap, HeaderValue, IF_NONE_MATCH, LOCATION,
};
use reqwest::{StatusCode, redirect};
use tokio::io::AsyncWriteExt as _;
use tracing::{debug, warn};

use crate::progress::{ProgressCallbackFn, ProgressEvent};

const DEFAULT_ENDPOINT: &str = "https://huggingface.co";
const REVISION: &str = "main";
const MAX_RELATIVE_REDIRECTS: usize = 10;
const USER_AGENT: &str = concat!("swink-agent-local-llm/", env!("CARGO_PKG_VERSION"));

/// Why a download could not produce a local file.
#[derive(Debug, thiserror::Error)]
pub(crate) enum DownloadError {
    #[error("model repo id {0:?} must be in \"owner/name\" form")]
    InvalidRepoId(String),
    #[error("model filename {0:?} must be a relative path without \"..\" components")]
    InvalidFilename(String),
    #[error("{url} returned HTTP {status}")]
    Status { url: String, status: u16 },
    #[error("{url} response is missing the {header} header")]
    MissingHeader { url: String, header: &'static str },
    #[error("hub unreachable and no cached copy of {repo_id}/{filename}: {source}")]
    Offline {
        repo_id: String,
        filename: String,
        #[source]
        source: reqwest::Error,
    },
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Resolves a model file to a local path, downloading it if needed.
///
/// One conditional `HEAD` per load revalidates a cached copy (a `304` returns
/// the cached path); when the hub is unreachable the cached copy is used
/// as-is, so offline use from a warm cache keeps working.
pub(crate) async fn resolve_model_path(
    repo_id: &str,
    filename: &str,
    progress_cb: Option<ProgressCallbackFn>,
) -> Result<PathBuf, DownloadError> {
    HubClient::from_env()
        .download(repo_id, filename, progress_cb)
        .await
}

/// A Hub endpoint plus the cache directory and token used against it.
pub(crate) struct HubClient {
    endpoint: String,
    cache_dir: PathBuf,
    token: Option<String>,
    /// Follows redirects; used for the byte download.
    http: reqwest::Client,
    /// Never follows redirects: the `HEAD` metadata (`x-repo-commit`, etag)
    /// is on the hub's response, not on the CDN it redirects to.
    head: reqwest::Client,
}

impl HubClient {
    /// Endpoint, cache directory and token from the `huggingface_hub`
    /// environment conventions.
    pub(crate) fn from_env() -> Self {
        let endpoint = std::env::var("HF_ENDPOINT").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_owned());
        Self::new(endpoint, cache_dir_from_env(), token_from_env())
    }

    pub(crate) fn new(
        endpoint: impl Into<String>,
        cache_dir: impl Into<PathBuf>,
        token: Option<String>,
    ) -> Self {
        // Workspace reqwest is `rustls-no-provider`; the process default must
        // be installed before any client is built. Idempotent.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = |policy: redirect::Policy| {
            reqwest::Client::builder()
                .user_agent(USER_AGENT)
                .redirect(policy)
                .build()
                .expect("static reqwest client configuration is valid")
        };
        Self {
            endpoint: endpoint.into().trim_end_matches('/').to_owned(),
            cache_dir: cache_dir.into(),
            token,
            http: client(redirect::Policy::default()),
            head: client(redirect::Policy::none()),
        }
    }

    /// Download `filename` from the model repo `repo_id` (`owner/name`) into
    /// the cache and return the snapshot path.
    pub(crate) async fn download(
        &self,
        repo_id: &str,
        filename: &str,
        progress_cb: Option<ProgressCallbackFn>,
    ) -> Result<PathBuf, DownloadError> {
        let Some((owner, name)) = repo_id
            .split_once('/')
            .filter(|(o, n)| !o.is_empty() && !n.is_empty())
        else {
            return Err(DownloadError::InvalidRepoId(repo_id.to_owned()));
        };
        if !is_safe_relative(filename) {
            return Err(DownloadError::InvalidFilename(filename.to_owned()));
        }
        let repo = RepoCache {
            dir: self.cache_dir.join(format!("models--{owner}--{name}")),
        };
        let url = format!(
            "{}/{owner}/{name}/resolve/{REVISION}/{filename}",
            self.endpoint
        );

        let cached_etag = repo.cached_etag(filename);
        let mut headers = self.auth_headers();
        if let Some(etag) = &cached_etag
            && let Ok(value) = HeaderValue::from_str(&format!("\"{etag}\""))
        {
            headers.insert(IF_NONE_MATCH, value);
        }

        let response = match self.head_following_relative_redirects(&url, headers).await {
            Ok(response) => response,
            Err(source) if source.is_connect() || source.is_timeout() || source.is_request() => {
                // Offline: serve the cached snapshot if there is one.
                if let Some(path) = repo.cached_snapshot(filename) {
                    debug!(%source, path = %path.display(), "hub unreachable; using cached model");
                    return Ok(path);
                }
                return Err(DownloadError::Offline {
                    repo_id: repo_id.to_owned(),
                    filename: filename.to_owned(),
                    source,
                });
            }
            Err(source) => return Err(source.into()),
        };

        let status = response.status();
        if status == StatusCode::NOT_MODIFIED
            && let (Some(etag), Some(commit)) = (cached_etag, repo.read_ref())
        {
            return repo.finalize(&commit, filename, &etag);
        }
        if !status.is_success() && !status.is_redirection() {
            return Err(DownloadError::Status {
                url,
                status: status.as_u16(),
            });
        }
        let header = |name: &'static str| -> Result<String, DownloadError> {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
                .ok_or(DownloadError::MissingHeader {
                    url: url.clone(),
                    header: name,
                })
        };
        let commit = header("x-repo-commit")?;
        // Hub files backed by LFS carry the blob's etag in `x-linked-etag`;
        // `etag` is the pointer file's. Weak validators and quotes stripped.
        let etag = header("x-linked-etag").or_else(|_| header(ETAG.as_str()))?;
        let etag = etag
            .strip_prefix("W/")
            .unwrap_or(&etag)
            .trim_matches('"')
            .to_owned();
        let total_bytes = header("x-linked-size")
            .or_else(|_| header(CONTENT_LENGTH.as_str()))
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|n| *n > 0);

        let blob = repo.dir.join("blobs").join(&etag);
        if !blob.exists() {
            self.fetch_blob(&url, &blob, total_bytes, progress_cb.as_ref())
                .await?;
        }
        repo.finalize(&commit, filename, &etag)
    }

    fn auth_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(token) = &self.token
            && let Ok(value) = HeaderValue::from_str(&format!("Bearer {token}"))
        {
            headers.insert(AUTHORIZATION, value);
        }
        headers
    }

    /// `HEAD` that follows same-origin (relative) redirects but stops at the
    /// first absolute one, which is the CDN hop whose headers we do not want.
    async fn head_following_relative_redirects(
        &self,
        url: &str,
        headers: HeaderMap,
    ) -> Result<reqwest::Response, reqwest::Error> {
        let mut url = reqwest::Url::parse(url).map_err(|_| unreachable_url())?;
        for _ in 0..=MAX_RELATIVE_REDIRECTS {
            let response = self
                .head
                .head(url.clone())
                .headers(headers.clone())
                .send()
                .await?;
            let relative = response
                .headers()
                .get(LOCATION)
                .and_then(|v| v.to_str().ok())
                .filter(|location| response.status().is_redirection() && location.starts_with('/'))
                .and_then(|location| url.join(location).ok());
            match relative {
                Some(next) => url = next,
                None => return Ok(response),
            }
        }
        warn!(%url, "too many relative redirects; using the last response");
        self.head.head(url).headers(headers).send().await
    }

    /// Stream the file into `blob`, writing a per-process temp file first so
    /// a crash never leaves a truncated blob under the final name.
    async fn fetch_blob(
        &self,
        url: &str,
        blob: &Path,
        total_bytes: Option<u64>,
        progress_cb: Option<&ProgressCallbackFn>,
    ) -> Result<(), DownloadError> {
        let response = self
            .http
            .get(url)
            .headers(self.auth_headers())
            .send()
            .await?
            .error_for_status()?;
        let total_bytes = total_bytes.or_else(|| response.content_length().filter(|n| *n > 0));
        let emit = |bytes_downloaded: u64| {
            if let Some(cb) = progress_cb {
                cb(ProgressEvent::DownloadProgress {
                    bytes_downloaded,
                    total_bytes,
                });
            }
        };
        emit(0);

        let parent = blob.parent().expect("blob path has a parent");
        tokio::fs::create_dir_all(parent).await?;
        // ponytail: no resume and no cross-process lock; two processes fetching
        // the same blob both complete and the last rename wins (same bytes).
        let incomplete = parent.join(format!(
            "{}.{}.incomplete",
            blob.file_name().and_then(|n| n.to_str()).unwrap_or("blob"),
            std::process::id()
        ));
        let mut file = tokio::fs::File::create(&incomplete).await?;
        let mut downloaded = 0u64;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) => {
                    let _ = tokio::fs::remove_file(&incomplete).await;
                    return Err(error.into());
                }
            };
            file.write_all(&chunk).await?;
            downloaded += chunk.len() as u64;
            emit(downloaded);
        }
        file.flush().await?;
        drop(file);
        tokio::fs::rename(&incomplete, blob).await?;
        Ok(())
    }
}

/// One `models--{owner}--{name}` directory in the cache.
struct RepoCache {
    dir: PathBuf,
}

impl RepoCache {
    fn read_ref(&self) -> Option<String> {
        std::fs::read_to_string(self.dir.join("refs").join(REVISION))
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
    }

    fn snapshot_path(&self, commit: &str, filename: &str) -> PathBuf {
        self.dir.join("snapshots").join(commit).join(filename)
    }

    /// The snapshot the current ref points at, if it is on disk.
    fn cached_snapshot(&self, filename: &str) -> Option<PathBuf> {
        let path = self.snapshot_path(&self.read_ref()?, filename);
        path.exists().then_some(path)
    }

    /// The blob etag behind the cached pointer (its symlink target's name).
    /// Windows pointers are copies, so revalidation there always re-checks.
    fn cached_etag(&self, filename: &str) -> Option<String> {
        let target = std::fs::read_link(self.cached_snapshot(filename)?).ok()?;
        target.file_name()?.to_str().map(str::to_owned)
    }

    /// Record the ref and point `snapshots/{commit}/{filename}` at the blob.
    fn finalize(&self, commit: &str, filename: &str, etag: &str) -> Result<PathBuf, DownloadError> {
        let refs = self.dir.join("refs");
        std::fs::create_dir_all(&refs)?;
        std::fs::write(refs.join(REVISION), commit)?;

        let pointer = self.snapshot_path(commit, filename);
        if !pointer.exists() {
            let pointer_dir = pointer.parent().expect("snapshot path has a parent");
            std::fs::create_dir_all(pointer_dir)?;
            let blob = self.dir.join("blobs").join(etag);
            let _ = std::fs::remove_file(&pointer); // a dangling link `exists()` as false
            #[cfg(unix)]
            {
                // Relative like huggingface_hub, so the cache dir can move.
                let depth = Path::new(filename).components().count();
                let mut relative = PathBuf::new();
                for _ in 0..=depth {
                    relative.push("..");
                }
                std::os::unix::fs::symlink(relative.join("blobs").join(etag), &pointer)?;
            }
            #[cfg(not(unix))]
            {
                std::fs::copy(&blob, &pointer)?;
            }
            debug!(path = %pointer.display(), blob = %blob.display(), "model cached");
        }
        Ok(pointer)
    }
}

fn is_safe_relative(filename: &str) -> bool {
    !filename.is_empty()
        && Path::new(filename)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

fn unreachable_url() -> reqwest::Error {
    // Only reachable with a malformed HF_ENDPOINT; surface it as the request
    // error a builder-less client would produce.
    reqwest::Client::new()
        .get("http://")
        .build()
        .expect_err("an empty host is not a valid request")
}

fn hf_home() -> PathBuf {
    if let Ok(home) = std::env::var("HF_HOME") {
        return PathBuf::from(home);
    }
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        return PathBuf::from(xdg).join("huggingface");
    }
    std::env::home_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(".cache")
        .join("huggingface")
}

fn cache_dir_from_env() -> PathBuf {
    std::env::var("HF_HUB_CACHE")
        .or_else(|_| std::env::var("HUGGINGFACE_HUB_CACHE"))
        .map_or_else(|_| hf_home().join("hub"), PathBuf::from)
}

fn token_from_env() -> Option<String> {
    let non_empty = |s: String| {
        let s = s.trim().to_owned();
        (!s.is_empty()).then_some(s)
    };
    if let Some(token) = std::env::var("HF_TOKEN").ok().and_then(non_empty) {
        return Some(token);
    }
    let path =
        std::env::var("HF_TOKEN_PATH").map_or_else(|_| hf_home().join("token"), PathBuf::from);
    std::fs::read_to_string(path).ok().and_then(non_empty)
}

#[cfg(test)]
#[path = "download_tests.rs"]
mod tests;
