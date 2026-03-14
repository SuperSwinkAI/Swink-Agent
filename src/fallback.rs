//! Model fallback configuration.
//!
//! [`ModelFallback`] defines an ordered list of fallback models to try when
//! the primary model exhausts its retry budget. Each entry pairs a
//! [`ModelSpec`] with its corresponding [`StreamFn`], allowing fallback
//! across providers.

use std::sync::Arc;

use crate::stream::StreamFn;
use crate::types::ModelSpec;

/// An ordered sequence of fallback models to attempt when the primary model
/// (and its retries) are exhausted.
///
/// The agent tries each model in order, applying the configured
/// [`RetryStrategy`](crate::RetryStrategy) independently for each model.
/// When all fallback models are also exhausted the error propagates normally.
///
/// # Example
///
/// ```rust,no_run
/// use swink_agent::{ModelFallback, ModelSpec};
/// # use std::sync::Arc;
/// # fn make_stream_fn() -> Arc<dyn swink_agent::StreamFn> { todo!() }
///
/// let fallback = ModelFallback::new(vec![
///     (ModelSpec::new("openai", "gpt-4o-mini"), make_stream_fn()),
///     (ModelSpec::new("anthropic", "claude-3-haiku-20240307"), make_stream_fn()),
/// ]);
/// ```
#[derive(Clone)]
pub struct ModelFallback {
    models: Vec<(ModelSpec, Arc<dyn StreamFn>)>,
}

impl ModelFallback {
    /// Create a new fallback chain from an ordered list of model/stream pairs.
    #[must_use]
    pub fn new(models: Vec<(ModelSpec, Arc<dyn StreamFn>)>) -> Self {
        Self { models }
    }

    /// Returns the fallback models in order.
    #[must_use]
    pub fn models(&self) -> &[(ModelSpec, Arc<dyn StreamFn>)] {
        &self.models
    }

    /// Returns `true` if the fallback chain is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    /// Returns the number of fallback models.
    #[must_use]
    pub fn len(&self) -> usize {
        self.models.len()
    }
}

impl std::fmt::Debug for ModelFallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelFallback")
            .field(
                "models",
                &self
                    .models
                    .iter()
                    .map(|(m, _)| format!("{}:{}", m.provider, m.model_id))
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_fallback() {
        let fb = ModelFallback::new(vec![]);
        assert!(fb.is_empty());
        assert_eq!(fb.len(), 0);
        assert!(fb.models().is_empty());
    }

    #[test]
    fn debug_format() {
        let fb = ModelFallback::new(vec![]);
        let dbg = format!("{fb:?}");
        assert!(dbg.contains("ModelFallback"));
    }
}
