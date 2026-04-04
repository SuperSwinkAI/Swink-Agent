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

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::types::Pipeline;

    #[test]
    fn register_and_get() {
        let registry = PipelineRegistry::new();
        let pipeline = Pipeline::sequential("test", vec!["a".into()]);
        let id = pipeline.id().clone();
        registry.register(pipeline);

        let got = registry.get(&id);
        assert!(got.is_some());
        assert_eq!(got.unwrap().name(), "test");
    }

    #[test]
    fn get_unknown_returns_none() {
        let registry = PipelineRegistry::new();
        let id = PipelineId::new("nonexistent");
        assert!(registry.get(&id).is_none());
    }

    #[test]
    fn list_returns_all() {
        let registry = PipelineRegistry::new();
        let p1 = Pipeline::sequential("one", vec!["a".into()]);
        let p2 = Pipeline::sequential("two", vec!["b".into()]);
        let p3 = Pipeline::sequential("three", vec!["c".into()]);
        registry.register(p1);
        registry.register(p2);
        registry.register(p3);

        let list = registry.list();
        assert_eq!(list.len(), 3);

        let names: Vec<&str> = list.iter().map(|(_, n)| n.as_str()).collect();
        assert!(names.contains(&"one"));
        assert!(names.contains(&"two"));
        assert!(names.contains(&"three"));
    }

    #[test]
    fn remove_deletes_entry() {
        let registry = PipelineRegistry::new();
        let pipeline = Pipeline::sequential("doomed", vec!["a".into()]);
        let id = pipeline.id().clone();
        registry.register(pipeline);

        assert!(registry.get(&id).is_some());
        let removed = registry.remove(&id);
        assert!(removed);
        assert!(registry.get(&id).is_none());
    }

    #[test]
    fn re_register_replaces() {
        let registry = PipelineRegistry::new();
        let id = PipelineId::new("fixed-id");
        let p1 = Pipeline::sequential("first", vec!["a".into()]).with_id(id.clone());
        let p2 = Pipeline::sequential("second", vec!["b".into()]).with_id(id.clone());

        registry.register(p1);
        assert_eq!(registry.get(&id).unwrap().name(), "first");

        registry.register(p2);
        assert_eq!(registry.get(&id).unwrap().name(), "second");
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn len_and_is_empty() {
        let registry = PipelineRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);

        registry.register(Pipeline::sequential("one", vec!["a".into()]));
        assert!(!registry.is_empty());
        assert_eq!(registry.len(), 1);
    }
}
