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
//! grammar (see `sanitize_tool_name_component`).

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
    /// plugin's name as prefix. The prefix and inner name are joined with an
    /// underscore and sanitized to the common subset accepted by every
    /// provider's tool-name grammar (e.g., `"myplugin_mytool"`). See
    /// `sanitize_tool_name_component` for the exact rule.
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

// ─── Tool name sanitization ────────────────────────────────────────────────

/// Maximum length for a composed tool name (the tightest cap across providers:
/// `OpenAI`, Bedrock, and Gemini all cap at 64; Anthropic allows 128).
const MAX_TOOL_NAME_LEN: usize = 64;
/// Hex characters preserved from the stable hash when truncation is required.
const TOOL_NAME_HASH_HEX_LEN: usize = 16;

/// Sanitize a single component (plugin name or inner tool name) to the common
/// subset of characters accepted by every provider's tool-name grammar.
///
/// The strictest grammar is Bedrock's `^[a-zA-Z][a-zA-Z0-9_]*` (max 64). This
/// is also a subset of what Anthropic (`^[a-zA-Z0-9_-]{1,128}$`), OpenAI-style
/// providers (same pattern, cap 64), and Gemini accept, so names produced here
/// round-trip safely across providers.
///
/// Rules:
/// - Every character outside `[a-zA-Z0-9_]` is replaced with `_`.
/// - The result is not truncated here; truncation happens on the composed name
///   in [`NamespacedTool::new`] so both components survive when possible.
/// - An empty input becomes `"_"` — callers should still prepend a letter
///   prefix if the composed result needs to start with a letter.
fn sanitize_tool_name_component(input: &str) -> String {
    if input.is_empty() {
        return "_".to_owned();
    }
    input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Compose a plugin-namespaced tool name that is safe across every provider.
///
/// Joins `plugin_name` and `tool_name` with `_`, sanitizes each half, prepends
/// `t_` if the result would start with a non-letter (Bedrock/Gemini require a
/// leading letter or underscore — we pick letter for maximum safety), and
/// truncates to [`MAX_TOOL_NAME_LEN`]. When truncation is required, a stable
/// hash suffix is appended so long names do not silently collapse onto the same
/// dispatch key.
fn compose_namespaced_name(plugin_name: &str, tool_name: &str) -> String {
    let plugin = sanitize_tool_name_component(plugin_name);
    let tool = sanitize_tool_name_component(tool_name);
    let joined = format!("{plugin}_{tool}");
    let with_leading_letter = match joined.chars().next() {
        Some(c) if c.is_ascii_alphabetic() => joined,
        _ => format!("t_{joined}"),
    };
    if with_leading_letter.len() <= MAX_TOOL_NAME_LEN {
        with_leading_letter
    } else {
        let hash_suffix = stable_name_hash_hex(&with_leading_letter);
        let prefix_len = MAX_TOOL_NAME_LEN - TOOL_NAME_HASH_HEX_LEN - 1;
        let prefix: String = with_leading_letter.chars().take(prefix_len).collect();
        format!("{prefix}_{hash_suffix}")
    }
}

fn stable_name_hash_hex(input: &str) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in input.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:0TOOL_NAME_HASH_HEX_LEN$x}")
}

// ─── NamespacedTool ────────────────────────────────────────────────────────

/// Wraps a plugin-contributed tool, prefixing the plugin name onto the tool name.
///
/// This prevents name collisions when multiple plugins contribute tools with
/// the same name. The composed name format is `"{plugin_name}_{tool_name}"`,
/// with each component sanitized so the result matches the strictest tool-name
/// grammar across supported providers (Anthropic, `OpenAI`, Bedrock, Mistral,
/// Gemini, Ollama, Azure). See `sanitize_tool_name_component`.
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

    // ─── Tool-name sanitization tests ───────────────────────────────────

    #[test]
    fn compose_namespaced_name_dot_becomes_underscore() {
        assert_eq!(compose_namespaced_name("web", "search"), "web_search");
        assert_eq!(compose_namespaced_name("web", "fetch"), "web_fetch");
    }

    #[test]
    fn compose_namespaced_name_replaces_dashes_and_dots() {
        assert_eq!(compose_namespaced_name("my-web", "search"), "my_web_search");
        assert_eq!(compose_namespaced_name("web", "read.file"), "web_read_file");
        assert_eq!(compose_namespaced_name("my.ns", "x.y.z"), "my_ns_x_y_z");
    }

    #[test]
    fn compose_namespaced_name_prepends_letter_when_leading_non_alpha() {
        // Plugin starting with a digit would otherwise produce "1plugin_foo" —
        // Bedrock requires a leading letter, so we prepend "t_".
        assert_eq!(compose_namespaced_name("1plugin", "foo"), "t_1plugin_foo");
        // Same for a leading underscore (valid for Gemini, rejected by Bedrock).
        assert_eq!(compose_namespaced_name("_plugin", "foo"), "t__plugin_foo");
    }

    #[test]
    fn compose_namespaced_name_replaces_non_ascii() {
        assert_eq!(compose_namespaced_name("plugin", "naïve"), "plugin_na_ve");
    }

    #[test]
    fn compose_namespaced_name_truncates_to_max_length() {
        let long_plugin = "a".repeat(40);
        let long_tool = "b".repeat(40);
        let result = compose_namespaced_name(&long_plugin, &long_tool);
        assert_eq!(result.len(), MAX_TOOL_NAME_LEN);
        // Prefix is preserved (plugin name survives at the front).
        assert!(result.starts_with(&long_plugin));
        assert_eq!(result.chars().filter(|c| *c == '_').count(), 2);
        assert_eq!(
            result.rsplit_once('_').unwrap().1.len(),
            TOOL_NAME_HASH_HEX_LEN
        );
    }

    #[test]
    fn compose_namespaced_name_long_collisions_get_distinct_hash_suffixes() {
        let long_plugin = "a".repeat(40);
        let first_tool = format!("{}x", "b".repeat(40));
        let second_tool = format!("{}y", "b".repeat(40));

        let first = compose_namespaced_name(&long_plugin, &first_tool);
        let second = compose_namespaced_name(&long_plugin, &second_tool);

        assert_eq!(first.len(), MAX_TOOL_NAME_LEN);
        assert_eq!(second.len(), MAX_TOOL_NAME_LEN);
        assert_ne!(first, second);
        assert_eq!(
            first[..MAX_TOOL_NAME_LEN - TOOL_NAME_HASH_HEX_LEN - 1],
            second[..MAX_TOOL_NAME_LEN - TOOL_NAME_HASH_HEX_LEN - 1]
        );
        assert_ne!(
            first.rsplit_once('_').unwrap().1,
            second.rsplit_once('_').unwrap().1
        );
    }

    #[test]
    fn compose_namespaced_name_empty_components() {
        // Empty plugin name collapses to "_", then the leading-letter rule kicks in.
        assert_eq!(compose_namespaced_name("", "foo"), "t___foo");
        assert_eq!(compose_namespaced_name("foo", ""), "foo__");
        // "" → "_", "" → "_", joined "___" (3 underscores), leading non-alpha → prepend "t_".
        assert_eq!(compose_namespaced_name("", ""), "t____");
    }

    #[test]
    fn compose_namespaced_name_satisfies_strictest_grammar() {
        // Regex equivalent to Bedrock's ^[a-zA-Z][a-zA-Z0-9_]*$ (the strictest
        // provider pattern). Every output from this function must match.
        let is_valid = |s: &str| {
            s.len() <= MAX_TOOL_NAME_LEN
                && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        };
        for (plugin, tool) in [
            ("web", "search"),
            ("my-web", "search"),
            ("web", "read.file"),
            ("1plugin", "foo"),
            ("_plugin", "foo"),
            ("plugin", "naïve"),
            ("", ""),
            (&"a".repeat(100), &"b".repeat(100)),
        ] {
            let name = compose_namespaced_name(plugin, tool);
            assert!(
                is_valid(&name),
                "composed name {name:?} (from {plugin:?} + {tool:?}) violates the strictest grammar"
            );
        }
    }

    #[test]
    fn namespaced_tool_preserves_unsanitized_plugin_name_in_metadata() {
        use crate::testing::MockTool;
        let tool: Arc<dyn AgentTool> = Arc::new(MockTool::new("search"));
        let wrapped = NamespacedTool::new("my-web", tool);
        assert_eq!(wrapped.name(), "my_web_search");
        // Metadata namespace keeps the original plugin name for introspection.
        let meta = wrapped.metadata().expect("metadata present");
        assert_eq!(meta.namespace.as_deref(), Some("my-web"));
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
