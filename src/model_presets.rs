use std::sync::Arc;

use crate::{ModelSpec, StreamFn};

type ExtraModelConnections = Vec<(ModelSpec, Arc<dyn StreamFn>)>;

#[derive(Clone)]
pub struct ModelConnection {
    model: ModelSpec,
    stream_fn: Arc<dyn StreamFn>,
}

impl ModelConnection {
    #[must_use]
    pub fn new(model: ModelSpec, stream_fn: Arc<dyn StreamFn>) -> Self {
        Self { model, stream_fn }
    }

    #[must_use]
    pub const fn model_spec(&self) -> &ModelSpec {
        &self.model
    }

    #[must_use]
    pub fn stream_fn(&self) -> Arc<dyn StreamFn> {
        Arc::clone(&self.stream_fn)
    }
}

pub struct ModelConnections {
    primary_model: ModelSpec,
    primary_stream_fn: Arc<dyn StreamFn>,
    extra_models: ExtraModelConnections,
}

impl ModelConnections {
    #[must_use]
    pub fn new(primary: ModelConnection, extras: Vec<ModelConnection>) -> Self {
        let ModelConnection {
            model: primary_model,
            stream_fn: primary_stream_fn,
        } = primary;
        let mut extra_models = Vec::new();

        for connection in extras {
            let model = connection.model.clone();
            if model == primary_model || extra_models.iter().any(|(existing, _)| *existing == model)
            {
                continue;
            }
            extra_models.push((model, connection.stream_fn()));
        }

        Self {
            primary_model,
            primary_stream_fn,
            extra_models,
        }
    }

    #[must_use]
    pub const fn primary_model(&self) -> &ModelSpec {
        &self.primary_model
    }

    #[must_use]
    pub fn primary_stream_fn(&self) -> Arc<dyn StreamFn> {
        Arc::clone(&self.primary_stream_fn)
    }

    #[must_use]
    pub fn extra_models(&self) -> &[(ModelSpec, Arc<dyn StreamFn>)] {
        &self.extra_models
    }

    #[must_use]
    pub fn into_parts(self) -> (ModelSpec, Arc<dyn StreamFn>, ExtraModelConnections) {
        (
            self.primary_model,
            self.primary_stream_fn,
            self.extra_models,
        )
    }

    /// Create a builder for constructing `ModelConnections` incrementally.
    #[must_use]
    pub const fn builder() -> ModelConnectionsBuilder {
        ModelConnectionsBuilder::new()
    }
}

/// Incrementally builds a [`ModelConnections`] value.
///
/// # Panics
///
/// [`build`](Self::build) panics if no primary connection has been set.
pub struct ModelConnectionsBuilder {
    primary: Option<ModelConnection>,
    fallbacks: Vec<ModelConnection>,
}

impl Default for ModelConnectionsBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelConnectionsBuilder {
    /// Create a new empty builder.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            primary: None,
            fallbacks: Vec::new(),
        }
    }

    /// Set the primary model connection.
    #[must_use]
    pub fn primary(mut self, connection: ModelConnection) -> Self {
        self.primary = Some(connection);
        self
    }

    /// Add a fallback model connection.
    #[must_use]
    pub fn fallback(mut self, connection: ModelConnection) -> Self {
        self.fallbacks.push(connection);
        self
    }

    /// Build the final [`ModelConnections`].
    ///
    /// # Panics
    ///
    /// Panics if no primary connection was set via [`primary`](Self::primary).
    #[must_use]
    pub fn build(self) -> ModelConnections {
        let primary = self
            .primary
            .expect("ModelConnectionsBuilder: primary connection is required");
        ModelConnections::new(primary, self.fallbacks)
    }
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;

    use futures::Stream;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::{AgentContext, AssistantMessageEvent, StreamOptions};

    struct DummyStreamFn;

    impl StreamFn for DummyStreamFn {
        fn stream<'a>(
            &'a self,
            _model: &'a ModelSpec,
            _context: &'a AgentContext,
            _options: &'a StreamOptions,
            _cancellation_token: CancellationToken,
        ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
            Box::pin(futures::stream::empty())
        }
    }

    fn dummy_stream() -> Arc<dyn StreamFn> {
        Arc::new(DummyStreamFn)
    }

    #[test]
    fn into_parts_returns_correct_values() {
        let primary_model = ModelSpec::new("anthropic", "claude-sonnet-4-6");
        let extra_model = ModelSpec::new("openai", "gpt-5.2");

        let connections = ModelConnections::new(
            ModelConnection::new(primary_model.clone(), dummy_stream()),
            vec![ModelConnection::new(extra_model.clone(), dummy_stream())],
        );

        let (model, _stream_fn, extras) = connections.into_parts();
        assert_eq!(model, primary_model);
        assert_eq!(extras.len(), 1);
        assert_eq!(extras[0].0, extra_model);
    }

    #[test]
    fn model_connection_getters() {
        let model = ModelSpec::new("test", "test-model");
        let stream = dummy_stream();
        let conn = ModelConnection::new(model.clone(), Arc::clone(&stream));

        assert_eq!(conn.model_spec(), &model);
        // stream_fn() returns a clone of the Arc
        let sf = conn.stream_fn();
        assert!(Arc::ptr_eq(&sf, &stream));
    }

    #[test]
    fn empty_extras() {
        let connections = ModelConnections::new(
            ModelConnection::new(
                ModelSpec::new("anthropic", "claude-sonnet-4-6"),
                dummy_stream(),
            ),
            vec![],
        );

        assert_eq!(connections.extra_models().len(), 0);
        assert_eq!(
            connections.primary_model(),
            &ModelSpec::new("anthropic", "claude-sonnet-4-6")
        );
    }

    #[test]
    fn all_extras_are_duplicates_of_primary() {
        let primary = ModelSpec::new("anthropic", "claude-sonnet-4-6");
        let connections = ModelConnections::new(
            ModelConnection::new(primary.clone(), dummy_stream()),
            vec![
                ModelConnection::new(primary.clone(), dummy_stream()),
                ModelConnection::new(primary, dummy_stream()),
            ],
        );

        // All extras match primary, so they should be filtered out
        assert_eq!(connections.extra_models().len(), 0);
    }

    #[test]
    fn model_connections_keep_primary_first_and_deduplicate_extras() {
        let connections = ModelConnections::new(
            ModelConnection::new(
                ModelSpec::new("anthropic", "claude-sonnet-4-6"),
                dummy_stream(),
            ),
            vec![
                ModelConnection::new(
                    ModelSpec::new("anthropic", "claude-sonnet-4-6"),
                    dummy_stream(),
                ),
                ModelConnection::new(ModelSpec::new("openai", "gpt-5.2"), dummy_stream()),
                ModelConnection::new(ModelSpec::new("openai", "gpt-5.2"), dummy_stream()),
                ModelConnection::new(ModelSpec::new("local", "SmolLM3-3B-Q4_K_M"), dummy_stream()),
            ],
        );

        assert_eq!(
            connections.primary_model(),
            &ModelSpec::new("anthropic", "claude-sonnet-4-6")
        );
        assert_eq!(connections.extra_models().len(), 2);
        assert_eq!(
            connections.extra_models()[0].0,
            ModelSpec::new("openai", "gpt-5.2")
        );
        assert_eq!(
            connections.extra_models()[1].0,
            ModelSpec::new("local", "SmolLM3-3B-Q4_K_M")
        );
    }

    #[test]
    fn builder_primary_only() {
        let connections = ModelConnections::builder()
            .primary(ModelConnection::new(
                ModelSpec::new("anthropic", "claude-sonnet-4-6"),
                dummy_stream(),
            ))
            .build();

        assert_eq!(
            connections.primary_model(),
            &ModelSpec::new("anthropic", "claude-sonnet-4-6")
        );
        assert_eq!(connections.extra_models().len(), 0);
    }

    #[test]
    fn builder_with_fallbacks() {
        let connections = ModelConnections::builder()
            .primary(ModelConnection::new(
                ModelSpec::new("anthropic", "claude-sonnet-4-6"),
                dummy_stream(),
            ))
            .fallback(ModelConnection::new(
                ModelSpec::new("openai", "gpt-5.2"),
                dummy_stream(),
            ))
            .fallback(ModelConnection::new(
                ModelSpec::new("local", "SmolLM3-3B-Q4_K_M"),
                dummy_stream(),
            ))
            .build();

        assert_eq!(connections.extra_models().len(), 2);
        assert_eq!(
            connections.extra_models()[0].0,
            ModelSpec::new("openai", "gpt-5.2")
        );
    }

    #[test]
    #[should_panic(expected = "primary connection is required")]
    fn builder_panics_without_primary() {
        let _ = ModelConnections::builder().build();
    }
}
