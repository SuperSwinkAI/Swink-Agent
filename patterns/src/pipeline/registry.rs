//! Thread-safe in-memory pipeline registry.

use std::collections::HashMap;
use std::sync::RwLock;

use super::types::{Pipeline, PipelineId};

/// Thread-safe in-memory registry for pipeline definitions.
pub struct PipelineRegistry {
    pipelines: RwLock<HashMap<PipelineId, Pipeline>>,
}

impl PipelineRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            pipelines: RwLock::new(HashMap::new()),
        }
    }

    /// Register a pipeline. Replaces any existing pipeline with the same ID.
    pub fn register(&self, pipeline: Pipeline) {
        let id = pipeline.id().clone();
        self.pipelines
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, pipeline);
    }

    /// Look up a pipeline by ID (returns a clone).
    pub fn get(&self, id: &PipelineId) -> Option<Pipeline> {
        self.pipelines
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
    }

    /// List all registered pipelines as `(id, name)` pairs.
    pub fn list(&self) -> Vec<(PipelineId, String)> {
        self.pipelines
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .map(|p| (p.id().clone(), p.name().to_owned()))
            .collect()
    }

    /// Remove a pipeline by ID. Returns `true` if it was present.
    pub fn remove(&self, id: &PipelineId) -> bool {
        self.pipelines
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id)
            .is_some()
    }

    /// Returns the number of registered pipelines.
    pub fn len(&self) -> usize {
        self.pipelines
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    /// Returns `true` if no pipelines are registered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for PipelineRegistry {
    fn default() -> Self {
        Self::new()
    }
}
