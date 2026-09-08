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
#[path = "model_presets_tests.rs"]
mod tests;
