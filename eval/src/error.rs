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

    /// The requested eval result was not found.
    #[error("eval result not found: {eval_set_id}/{timestamp}")]
    ResultNotFound { eval_set_id: String, timestamp: u64 },

    /// An eval case definition is invalid.
    #[error("invalid eval case: {reason}")]
    InvalidCase { reason: String },

    /// An evaluator name was registered more than once in the same registry.
    #[error("duplicate evaluator registration: {name}")]
    DuplicateEvaluator { name: String },

    /// A filesystem-facing identifier is invalid.
    #[error("invalid {kind} identifier: {id}")]
    InvalidIdentifier { kind: &'static str, id: String },

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
        source: serde_yaml::Error,
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

    /// Convenience constructor for [`EvalError::DuplicateEvaluator`].
    pub fn duplicate_evaluator(name: impl Into<String>) -> Self {
        Self::DuplicateEvaluator { name: name.into() }
    }

    /// Convenience constructor for [`EvalError::InvalidIdentifier`].
    pub fn invalid_identifier(kind: &'static str, id: impl Into<String>) -> Self {
        Self::InvalidIdentifier {
            kind,
            id: id.into(),
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
impl From<serde_yaml::Error> for EvalError {
    fn from(source: serde_yaml::Error) -> Self {
        Self::Yaml { source }
    }
}
