//! Error types for the evaluation framework.

/// The top-level error type for eval operations.
#[derive(Debug, thiserror::Error)]
pub enum EvalError {
    /// An error from the underlying agent during evaluation.
    #[error("agent error during evaluation")]
    Agent {
        #[source]
        source: swink_agent::AgentError,
    },

    /// The requested eval case was not found.
    #[error("eval case not found: {id}")]
    CaseNotFound { id: String },

    /// The requested eval set was not found.
    #[error("eval set not found: {id}")]
    SetNotFound { id: String },

    /// An eval case definition is invalid.
    #[error("invalid eval case: {reason}")]
    InvalidCase { reason: String },

    /// Filesystem or IO error during persistence.
    #[error("io error")]
    Io {
        #[source]
        source: std::io::Error,
    },

    /// Serialization or deserialization failure.
    #[error("serialization error")]
    Serde {
        #[source]
        source: serde_json::Error,
    },

    /// YAML deserialization failure.
    #[cfg(feature = "yaml")]
    #[error("yaml error")]
    Yaml {
        #[source]
        source: serde_yml::Error,
    },
}

impl EvalError {
    /// Convenience constructor for [`EvalError::Agent`].
    pub const fn agent(source: swink_agent::AgentError) -> Self {
        Self::Agent { source }
    }

    /// Convenience constructor for [`EvalError::InvalidCase`].
    pub fn invalid_case(reason: impl Into<String>) -> Self {
        Self::InvalidCase {
            reason: reason.into(),
        }
    }
}

impl From<std::io::Error> for EvalError {
    fn from(source: std::io::Error) -> Self {
        Self::Io { source }
    }
}

impl From<serde_json::Error> for EvalError {
    fn from(source: serde_json::Error) -> Self {
        Self::Serde { source }
    }
}

impl From<swink_agent::AgentError> for EvalError {
    fn from(source: swink_agent::AgentError) -> Self {
        Self::Agent { source }
    }
}

#[cfg(feature = "yaml")]
impl From<serde_yml::Error> for EvalError {
    fn from(source: serde_yml::Error) -> Self {
        Self::Yaml { source }
    }
}
