//! Plugin system for composing reusable bundles of policies, tools, and event observers.
//!
//! A [`Plugin`] is a single extension point that contributes policies to any of the four
//! policy slots, tools (automatically namespaced), and an event observer. Plugins are
//! registered on [`AgentOptions`](crate::AgentOptions) and merged into the agent during
//! construction.
//!
//! [`PluginRegistry`] manages a collection of plugins with deduplication and priority
//! ordering. [`NamespacedTool`] wraps a plugin-contributed tool, prefixing the plugin
//! name so the composed identifier is unique and safe for every provider's tool-name
//! grammar (see `compose_provider_safe_tool_name`).

use std::sync::Arc;

use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::loop_::AgentEvent;
use crate::policy::{PostLoopPolicy, PostTurnPolicy, PreDispatchPolicy, PreTurnPolicy};
use crate::tool::{AgentTool, AgentToolResult, ToolFuture, ToolMetadata};
use crate::tool_name::compose_provider_safe_tool_name;
#[cfg(test)]
use crate::tool_name::{MAX_TOOL_NAME_LEN, TOOL_NAME_HASH_HEX_LEN};

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
    /// plugin's name as prefix. The prefix and inner name are joined with an
    /// underscore and sanitized to the common subset accepted by every
    /// provider's tool-name grammar (e.g., `"myplugin_mytool"`). See
    /// `sanitize_tool_name_component` for the exact rule.
    fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        vec![]
    }
}

// ─── Shared plugin-collection invariants ───────────────────────────────────
//
// [`PluginRegistry`] and [`AgentOptions`](crate::agent_options::AgentOptions)
// both maintain a `Vec<Arc<dyn Plugin>>` with the same two invariants:
// dedup-by-name (last registration wins) and priority-descending ordering
// (stable, so insertion order breaks ties). These two functions are the
// single source of truth for both call sites so the invariants can't drift.

/// Insert `plugin` into `plugins`, replacing any existing entry with the same
/// name (last-registration-wins) and logging a warning when that happens.
pub(crate) fn dedup_insert_plugin(plugins: &mut Vec<Arc<dyn Plugin>>, plugin: Arc<dyn Plugin>) {
    let name = plugin.name().to_owned();
    if let Some(pos) = plugins.iter().position(|p| p.name() == name) {
        warn!(plugin = %name, "replacing duplicate plugin");
        plugins[pos] = plugin;
    } else {
        plugins.push(plugin);
    }
}

/// Sort key for priority-descending, stable ordering (highest priority first,
/// insertion order preserved for ties).
pub(crate) fn priority_desc(plugin: &Arc<dyn Plugin>) -> std::cmp::Reverse<i32> {
    std::cmp::Reverse(plugin.priority())
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
        dedup_insert_plugin(&mut self.plugins, plugin);
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
        sorted.sort_by_key(|p| priority_desc(p));
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

// ─── Tool name sanitization ────────────────────────────────────────────────

/// Compose a plugin-namespaced tool name that is safe across every provider.
///
/// Joins `plugin_name` and `tool_name` with `_`, sanitizes each half, prepends
/// `t_` if the result would start with a non-letter (Bedrock/Gemini require a
/// leading letter or underscore — we pick letter for maximum safety), and
/// truncates to [`MAX_TOOL_NAME_LEN`]. When truncation is required, a stable
/// hash suffix is appended so long names do not silently collapse onto the same
/// dispatch key.
fn compose_namespaced_name(plugin_name: &str, tool_name: &str) -> String {
    compose_provider_safe_tool_name(Some(plugin_name), tool_name)
}

// ─── NamespacedTool ────────────────────────────────────────────────────────

/// Wraps a plugin-contributed tool, prefixing the plugin name onto the tool name.
///
/// This prevents name collisions when multiple plugins contribute tools with
/// the same name. The composed name format is `"{plugin_name}_{tool_name}"`,
/// with each component sanitized so the result matches the strictest tool-name
/// grammar across supported providers (Anthropic, `OpenAI`, Bedrock, Mistral,
/// Gemini, Ollama, Azure). See `compose_provider_safe_tool_name`.
///
/// The original (unsanitized) plugin name is preserved in
/// [`ToolMetadata::namespace`] for introspection.
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
        let prefixed_name = compose_namespaced_name(&plugin_name, inner.name());
        Self::with_name(plugin_name, inner, prefixed_name)
    }

    /// Create a new namespaced tool wrapper with an explicit provider-safe
    /// final name.
    pub fn with_name(
        plugin_name: impl Into<String>,
        inner: Arc<dyn AgentTool>,
        prefixed_name: impl Into<String>,
    ) -> Self {
        let plugin_name = plugin_name.into();
        Self {
            prefixed_name: prefixed_name.into(),
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

    fn execution_root(&self) -> Option<&std::path::Path> {
        self.inner.execution_root()
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
#[path = "plugin_tests.rs"]
mod tests;
