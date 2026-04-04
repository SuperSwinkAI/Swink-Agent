//! Plugin system for composing reusable bundles of policies, tools, and event observers.
//!
//! A [`Plugin`] is a single extension point that contributes policies to any of the four
//! policy slots, tools (automatically namespaced), and an event observer. Plugins are
//! registered on [`AgentOptions`](crate::AgentOptions) and merged into the agent during
//! construction.
//!
//! [`PluginRegistry`] manages a collection of plugins with deduplication and priority
//! ordering. [`NamespacedTool`] wraps a plugin-contributed tool, prefixing the plugin
//! name to avoid collisions.

use std::sync::Arc;

use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::loop_::AgentEvent;
use crate::policy::{PostLoopPolicy, PostTurnPolicy, PreDispatchPolicy, PreTurnPolicy};
use crate::tool::{AgentTool, AgentToolResult, ToolFuture, ToolMetadata};

// ─── Plugin Trait ──────────────────────────────────────────────────────────

/// A reusable extension that bundles policies, tools, and an event observer.
///
/// Only [`name()`](Plugin::name) is required; all other methods have default
/// no-op implementations. Plugins are `Send + Sync` so they can be shared
/// across the agent's async tasks.
pub trait Plugin: Send + Sync {
    /// Unique identifier for this plugin (used for registry lookup and tool namespacing).
    fn name(&self) -> &str;

    /// Execution priority — higher values run first. Default: `0`.
    ///
    /// When multiple plugins contribute policies, higher-priority plugins'
    /// policies are evaluated before lower-priority ones. Ties are broken by
    /// insertion order (first registered wins).
    fn priority(&self) -> i32 {
        0
    }

    /// Called once during [`Agent::new()`](crate::Agent::new) after the agent is fully configured.
    ///
    /// Default: no-op.
    fn on_init(&self, _agent: &crate::Agent) {
        // no-op default
    }

    /// Pre-turn policies contributed by this plugin.
    fn pre_turn_policies(&self) -> Vec<Arc<dyn PreTurnPolicy>> {
        vec![]
    }

    /// Pre-dispatch policies contributed by this plugin.
    fn pre_dispatch_policies(&self) -> Vec<Arc<dyn PreDispatchPolicy>> {
        vec![]
    }

    /// Post-turn policies contributed by this plugin.
    fn post_turn_policies(&self) -> Vec<Arc<dyn PostTurnPolicy>> {
        vec![]
    }

    /// Post-loop policies contributed by this plugin.
    fn post_loop_policies(&self) -> Vec<Arc<dyn PostLoopPolicy>> {
        vec![]
    }

    /// Event observer called for every [`AgentEvent`] dispatched by the agent.
    ///
    /// Default: no-op.
    fn on_event(&self, _event: &AgentEvent) {
        // no-op default
    }

    /// Tools contributed by this plugin.
    ///
    /// Each tool is automatically wrapped in a [`NamespacedTool`] with the
    /// plugin's name as prefix (e.g., `"myplugin.mytool"`).
    fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        vec![]
    }
}

// ─── PluginRegistry ────────────────────────────────────────────────────────

/// A collection of plugins with deduplication and priority-based ordering.
///
/// Plugins are stored in insertion order internally. The [`list()`](Self::list)
/// method returns them sorted by priority (highest first, stable sort).
pub struct PluginRegistry {
    plugins: Vec<Arc<dyn Plugin>>,
}

impl PluginRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    /// Register a plugin. If a plugin with the same name already exists,
    /// it is replaced and a warning is logged.
    pub fn register(&mut self, plugin: Arc<dyn Plugin>) {
        let name = plugin.name().to_owned();
        if let Some(pos) = self.plugins.iter().position(|p| p.name() == name) {
            warn!(plugin = %name, "replacing duplicate plugin");
            self.plugins[pos] = plugin;
        } else {
            self.plugins.push(plugin);
        }
    }

    /// Remove a plugin by name. No-op if not found (idempotent).
    pub fn unregister(&mut self, name: &str) {
        self.plugins.retain(|p| p.name() != name);
    }

    /// Look up a plugin by name.
    pub fn get(&self, name: &str) -> Option<&Arc<dyn Plugin>> {
        self.plugins.iter().find(|p| p.name() == name)
    }

    /// All plugins sorted by priority (highest first, stable sort).
    pub fn list(&self) -> Vec<&Arc<dyn Plugin>> {
        let mut sorted: Vec<_> = self.plugins.iter().collect();
        sorted.sort_by_key(|p| std::cmp::Reverse(p.priority()));
        sorted
    }

    /// True if no plugins are registered.
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Number of registered plugins.
    pub fn len(&self) -> usize {
        self.plugins.len()
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ─── NamespacedTool ────────────────────────────────────────────────────────

/// Wraps a plugin-contributed tool, prefixing the plugin name onto the tool name.
///
/// This prevents name collisions when multiple plugins contribute tools with
/// the same name. The prefixed name format is `"{plugin_name}.{tool_name}"`.
///
/// All other trait methods delegate unchanged to the inner tool.
pub struct NamespacedTool {
    prefixed_name: String,
    plugin_name: String,
    inner: Arc<dyn AgentTool>,
}

impl NamespacedTool {
    /// Create a new namespaced tool wrapper.
    pub fn new(plugin_name: impl Into<String>, inner: Arc<dyn AgentTool>) -> Self {
        let plugin_name = plugin_name.into();
        let prefixed_name = format!("{}.{}", plugin_name, inner.name());
        Self {
            prefixed_name,
            plugin_name,
            inner,
        }
    }
}

impl AgentTool for NamespacedTool {
    fn name(&self) -> &str {
        &self.prefixed_name
    }

    fn label(&self) -> &str {
        self.inner.label()
    }

    fn description(&self) -> &str {
        self.inner.description()
    }

    fn parameters_schema(&self) -> &Value {
        self.inner.parameters_schema()
    }

    fn requires_approval(&self) -> bool {
        self.inner.requires_approval()
    }

    fn metadata(&self) -> Option<ToolMetadata> {
        let mut meta = self.inner.metadata().unwrap_or_default();
        meta.namespace = Some(self.plugin_name.clone());
        Some(meta)
    }

    fn approval_context(&self, params: &Value) -> Option<Value> {
        self.inner.approval_context(params)
    }

    fn auth_config(&self) -> Option<crate::credential::AuthConfig> {
        self.inner.auth_config()
    }

    fn execute(
        &self,
        tool_call_id: &str,
        params: Value,
        cancellation_token: CancellationToken,
        on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        state: Arc<std::sync::RwLock<crate::SessionState>>,
        credential: Option<crate::credential::ResolvedCredential>,
    ) -> ToolFuture<'_> {
        self.inner.execute(
            tool_call_id,
            params,
            cancellation_token,
            on_update,
            state,
            credential,
        )
    }
}

impl std::fmt::Debug for NamespacedTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NamespacedTool")
            .field("prefixed_name", &self.prefixed_name)
            .field("plugin_name", &self.plugin_name)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::testing::MockPlugin;

    // ─── PluginRegistry tests ───────────────────────────────────────────

    #[test]
    fn registry_register_and_get() {
        let mut reg = PluginRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);

        reg.register(Arc::new(MockPlugin::new("alpha")));
        assert!(!reg.is_empty());
        assert_eq!(reg.len(), 1);
        assert!(reg.get("alpha").is_some());
        assert!(reg.get("beta").is_none());
    }

    #[test]
    fn registry_duplicate_replaces() {
        let mut reg = PluginRegistry::new();
        reg.register(Arc::new(MockPlugin::new("alpha").with_priority(1)));
        reg.register(Arc::new(MockPlugin::new("alpha").with_priority(5)));
        assert_eq!(reg.len(), 1);
        assert_eq!(reg.get("alpha").unwrap().priority(), 5);
    }

    #[test]
    fn registry_unregister() {
        let mut reg = PluginRegistry::new();
        reg.register(Arc::new(MockPlugin::new("alpha")));
        reg.register(Arc::new(MockPlugin::new("beta")));
        assert_eq!(reg.len(), 2);

        reg.unregister("alpha");
        assert_eq!(reg.len(), 1);
        assert!(reg.get("alpha").is_none());
        assert!(reg.get("beta").is_some());
    }

    #[test]
    fn registry_unregister_nonexistent_is_noop() {
        let mut reg = PluginRegistry::new();
        reg.register(Arc::new(MockPlugin::new("alpha")));
        reg.unregister("nonexistent");
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn registry_list_sorted_by_priority_desc() {
        let mut reg = PluginRegistry::new();
        reg.register(Arc::new(MockPlugin::new("low").with_priority(1)));
        reg.register(Arc::new(MockPlugin::new("high").with_priority(10)));
        reg.register(Arc::new(MockPlugin::new("mid").with_priority(5)));

        let list = reg.list();
        let names: Vec<&str> = list.iter().map(|p| p.name()).collect();
        assert_eq!(names, vec!["high", "mid", "low"]);
    }

    #[test]
    fn registry_list_stable_sort_for_equal_priority() {
        let mut reg = PluginRegistry::new();
        reg.register(Arc::new(MockPlugin::new("first").with_priority(0)));
        reg.register(Arc::new(MockPlugin::new("second").with_priority(0)));
        reg.register(Arc::new(MockPlugin::new("third").with_priority(0)));

        let list = reg.list();
        let names: Vec<&str> = list.iter().map(|p| p.name()).collect();
        assert_eq!(names, vec!["first", "second", "third"]);
    }
}
