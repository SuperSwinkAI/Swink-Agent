//! Error types for local model inference.
//!
//! [`LocalModelError`] covers the lifecycle from model download through
//! inference. Transient download/load failures are distinct from runtime
//! inference errors.

/// Errors that can occur during local model operations.
#[non_exhaustive]
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

    /// Embedding-time error (input too long, model error).
    #[error("embedding error: {message}")]
    Embedding { message: String },

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

    /// Convenience constructor for [`LocalModelError::Loading`] from a message.
    pub fn loading_message(message: impl Into<String>) -> Self {
        Self::Loading {
            source: message.into().into(),
        }
    }

    /// Convenience constructor for [`LocalModelError::Inference`].
    pub fn inference(message: impl Into<String>) -> Self {
        Self::Inference {
            message: message.into(),
        }
    }

    /// Convenience constructor for [`LocalModelError::Embedding`].
    pub fn embedding(message: impl Into<String>) -> Self {
        Self::Embedding {
            message: message.into(),
        }
    }
}

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
