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
}
