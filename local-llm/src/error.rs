//! Error types for local model inference.
//!
//! [`LocalModelError`] covers the lifecycle from model download through
//! inference. Transient download/load failures are distinct from runtime
//! inference errors.

/// Errors that can occur during local model operations.
#[derive(Debug, thiserror::Error)]
pub enum LocalModelError {
    /// Failed to download model weights from `HuggingFace`.
    #[error("model download failed: {source}")]
    Download {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// Failed to load model into memory (e.g. GGUF parse error, OOM).
    #[error("model loading failed: {source}")]
    Loading {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// Inference-time error (generation failure, malformed output).
    #[error("inference error: {message}")]
    Inference { message: String },

    /// Model has not been loaded yet — call `ensure_ready()` first.
    #[error("model not ready — call ensure_ready() first")]
    NotReady,
}

impl LocalModelError {
    /// Convenience constructor for [`LocalModelError::Download`].
    pub fn download(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Download {
            source: Box::new(err),
        }
    }

    /// Convenience constructor for [`LocalModelError::Loading`].
    pub fn loading(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Loading {
            source: Box::new(err),
        }
    }

    /// Convenience constructor for [`LocalModelError::Inference`].
    pub fn inference(message: impl Into<String>) -> Self {
        Self::Inference {
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_download_error() {
        let err = LocalModelError::download(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "repo not found",
        ));
        let msg = err.to_string();
        assert!(msg.contains("download failed"), "got: {msg}");
        assert!(msg.contains("repo not found"), "got: {msg}");
    }

    #[test]
    fn display_loading_error() {
        let err = LocalModelError::loading(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "corrupt GGUF",
        ));
        let msg = err.to_string();
        assert!(msg.contains("loading failed"), "got: {msg}");
    }

    #[test]
    fn display_inference_error() {
        let err = LocalModelError::inference("token limit exceeded");
        assert_eq!(err.to_string(), "inference error: token limit exceeded");
    }

    #[test]
    fn display_not_ready() {
        let err = LocalModelError::NotReady;
        assert!(err.to_string().contains("not ready"));
    }

    #[test]
    fn source_chaining() {
        use std::error::Error as _;

        let inner = std::io::Error::other("inner");
        let err = LocalModelError::download(inner);
        assert!(err.source().is_some());

        let err = LocalModelError::NotReady;
        assert!(err.source().is_none());
    }
}
