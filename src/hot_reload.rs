//! Feature-gated tool hot-reloading from definition files.
//!
//! When the `hot-reload` feature is enabled, [`ToolWatcher`] monitors a
//! directory for TOML/JSON tool definition files and reloads them at runtime.
//!
//! # Definition file format (TOML)
//!
//! ```toml
//! name = "my_tool"
//! description = "Does something useful"
//! command = "echo {message}"
//!
//! [parameters_schema]
//! type = "object"
//! [parameters_schema.properties.message]
//! type = "string"
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use notify::{RecommendedWatcher, RecursiveMode, Watcher, Event, EventKind};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::tool::{AgentTool, AgentToolResult, ToolFuture, permissive_object_schema};
use crate::tool_filter::ToolFilter;

// ─── ScriptTool ────────────────────────────────────────────────────────────

/// A tool definition parsed from a file (TOML or JSON).
#[derive(Debug, Clone, Deserialize)]
pub struct ScriptToolDef {
    /// Unique tool name.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Shell command template. Parameters are interpolated as `{param_name}`.
    pub command: String,
    /// Optional JSON Schema for parameters.
    #[serde(default = "default_schema")]
    pub parameters_schema: Value,
}

fn default_schema() -> Value {
    permissive_object_schema()
}

/// A tool loaded from a definition file that executes a shell command.
///
/// Parameter values are shell-escaped before interpolation to prevent
/// command injection.
#[derive(Debug, Clone)]
pub struct ScriptTool {
    def: ScriptToolDef,
    schema: Value,
}

impl ScriptTool {
    /// Parse a tool definition from TOML content.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the TOML is invalid or missing required fields.
    pub fn from_toml(content: &str) -> Result<Self, String> {
        let def: ScriptToolDef = toml::from_str(content).map_err(|e| e.to_string())?;
        Ok(Self {
            schema: def.parameters_schema.clone(),
            def,
        })
    }

    /// Parse a tool definition from JSON content.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the JSON is invalid or missing required fields.
    pub fn from_json(content: &str) -> Result<Self, String> {
        let def: ScriptToolDef = serde_json::from_str(content).map_err(|e| e.to_string())?;
        Ok(Self {
            schema: def.parameters_schema.clone(),
            def,
        })
    }

    /// Load from a file, auto-detecting format by extension.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the file cannot be read or parsed.
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        match path.extension().and_then(|e| e.to_str()) {
            Some("toml") => Self::from_toml(&content),
            Some("json") => Self::from_json(&content),
            other => Err(format!("unsupported file extension: {other:?}")),
        }
    }

    /// Shell-escape a string value to prevent command injection.
    fn shell_escape(value: &str) -> String {
        // Single-quote wrapping with internal single-quote escaping
        format!("'{}'", value.replace('\'', "'\\''"))
    }

    /// Interpolate parameters into the command template.
    fn interpolate_command(&self, params: &Value) -> String {
        let mut cmd = self.def.command.clone();
        if let Value::Object(map) = params {
            for (key, val) in map {
                let val_str = match val {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                let escaped = Self::shell_escape(&val_str);
                cmd = cmd.replace(&format!("{{{key}}}"), &escaped);
            }
        }
        cmd
    }
}

impl AgentTool for ScriptTool {
    fn name(&self) -> &str {
        &self.def.name
    }

    fn label(&self) -> &str {
        &self.def.name
    }

    fn description(&self) -> &str {
        &self.def.description
    }

    fn parameters_schema(&self) -> &Value {
        &self.schema
    }

    fn requires_approval(&self) -> bool {
        true // Script tools always require approval
    }

    fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        cancellation_token: CancellationToken,
        _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        _state: Arc<std::sync::RwLock<crate::SessionState>>,
        _credential: Option<crate::credential::ResolvedCredential>,
    ) -> ToolFuture<'_> {
        let command = self.interpolate_command(&params);
        Box::pin(async move {
            if cancellation_token.is_cancelled() {
                return AgentToolResult::error("cancelled before execution");
            }

            match tokio::process::Command::new("sh")
                .arg("-c")
                .arg(&command)
                .output()
                .await
            {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    if output.status.success() {
                        AgentToolResult::text(stdout.to_string())
                    } else {
                        AgentToolResult::error(format!("exit {}: {stderr}", output.status))
                    }
                }
                Err(e) => AgentToolResult::error(format!("command failed: {e}")),
            }
        })
    }
}

// ─── ToolWatcher ────────────────────────────────────────────────────────────

/// Watches a directory for tool definition files and updates the agent's tools.
pub struct ToolWatcher {
    watch_dir: PathBuf,
    filter: Option<ToolFilter>,
    _watcher: RecommendedWatcher,
    event_rx: mpsc::Receiver<Result<Event, notify::Error>>,
}

impl ToolWatcher {
    /// Create a new watcher for the given directory.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the watcher cannot be created or the directory cannot
    /// be watched.
    pub fn new(watch_dir: impl Into<PathBuf>) -> Result<Self, String> {
        Self::with_filter(watch_dir, None)
    }

    /// Create a new watcher with an optional [`ToolFilter`].
    ///
    /// # Errors
    ///
    /// Returns `Err` if the watcher cannot be created or the directory cannot
    /// be watched.
    pub fn with_filter(
        watch_dir: impl Into<PathBuf>,
        filter: Option<ToolFilter>,
    ) -> Result<Self, String> {
        let watch_dir = watch_dir.into();
        let (tx, rx) = mpsc::channel(100);

        let watcher_tx = tx.clone();
        let mut watcher = notify::recommended_watcher(move |res| {
            let _ = watcher_tx.blocking_send(res);
        })
        .map_err(|e| e.to_string())?;

        watcher
            .watch(&watch_dir, RecursiveMode::NonRecursive)
            .map_err(|e| e.to_string())?;

        Ok(Self {
            watch_dir,
            filter,
            _watcher: watcher,
            event_rx: rx,
        })
    }

    /// Start the watcher loop. Returns a stream of tool list updates.
    ///
    /// The returned closure should be called to get the current tool list
    /// whenever an update occurs.
    pub async fn start(
        mut self,
        cancellation_token: CancellationToken,
    ) -> mpsc::Receiver<Vec<Arc<dyn AgentTool>>> {
        let (update_tx, update_rx) = mpsc::channel(10);

        tokio::spawn(async move {
            let mut tools: HashMap<PathBuf, ScriptTool> = HashMap::new();
            let debounce = std::time::Duration::from_millis(500);

            // Initial scan
            if let Ok(entries) = std::fs::read_dir(&self.watch_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if is_tool_file(&path) {
                        if let Ok(tool) = ScriptTool::from_file(&path) {
                            // Check for duplicate names
                            if tools.values().any(|t| t.def.name == tool.def.name) {
                                warn!(
                                    name = %tool.def.name,
                                    path = %path.display(),
                                    "duplicate tool name — last write wins"
                                );
                                // Remove old entry with same name
                                tools.retain(|_, t| t.def.name != tool.def.name);
                            }
                            tools.insert(path, tool);
                        }
                    }
                }
                let _ = update_tx.send(self.build_tool_list(&tools)).await;
            }

            loop {
                tokio::select! {
                    () = cancellation_token.cancelled() => break,
                    event = self.event_rx.recv() => {
                        let Some(Ok(event)) = event else { break };

                        // Debounce: sleep briefly, drain queued events
                        tokio::time::sleep(debounce).await;
                        while self.event_rx.try_recv().is_ok() {}

                        let mut changed = false;
                        for path in &event.paths {
                            if !is_tool_file(path) {
                                continue;
                            }
                            match &event.kind {
                                EventKind::Create(_) | EventKind::Modify(_) => {
                                    match ScriptTool::from_file(path) {
                                        Ok(tool) => {
                                            // Check for duplicate names from other files
                                            let name = tool.def.name.clone();
                                            if tools.iter().any(|(p, t)| p != path && t.def.name == name) {
                                                warn!(
                                                    name = %name,
                                                    path = %path.display(),
                                                    "duplicate tool name — last write wins"
                                                );
                                                tools.retain(|p2, t| p2 == path || t.def.name != name);
                                            }
                                            info!(tool = %tool.def.name, "loaded tool definition");
                                            tools.insert(path.clone(), tool);
                                            changed = true;
                                        }
                                        Err(e) => {
                                            warn!(
                                                path = %path.display(),
                                                error = %e,
                                                "invalid tool definition — skipping"
                                            );
                                        }
                                    }
                                }
                                EventKind::Remove(_) => {
                                    if let Some(removed) = tools.remove(path) {
                                        info!(tool = %removed.def.name, "removed tool definition");
                                        changed = true;
                                    }
                                }
                                _ => {}
                            }
                        }

                        if changed {
                            let _ = update_tx.send(self.build_tool_list(&tools)).await;
                        }
                    }
                }
            }
        });

        update_rx
    }

    fn build_tool_list(&self, tools: &HashMap<PathBuf, ScriptTool>) -> Vec<Arc<dyn AgentTool>> {
        let all: Vec<Arc<dyn AgentTool>> = tools
            .values()
            .map(|t| Arc::new(t.clone()) as Arc<dyn AgentTool>)
            .collect();
        if let Some(filter) = &self.filter {
            filter.filter_tools(all)
        } else {
            all
        }
    }
}

fn is_tool_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| matches!(ext, "toml" | "json"))
}

// ─── Compile-time Send + Sync assertions ────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ScriptTool>();
};

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn script_tool_from_toml() {
        let toml = r#"
name = "greet"
description = "Greet someone"
command = "echo Hello {name}"
"#;
        let tool = ScriptTool::from_toml(toml).unwrap();
        assert_eq!(tool.name(), "greet");
        assert_eq!(tool.description(), "Greet someone");
        assert!(tool.requires_approval());
    }

    #[test]
    fn script_tool_from_json_definition() {
        let json_str = r#"{"name": "test", "description": "A test", "command": "echo test"}"#;
        let tool = ScriptTool::from_json(json_str).unwrap();
        assert_eq!(tool.name(), "test");
    }

    #[test]
    fn script_tool_invalid_definition() {
        let result = ScriptTool::from_toml("invalid toml {{{}}}");
        assert!(result.is_err());
    }

    #[test]
    fn script_tool_escapes_parameters() {
        let toml = r#"
name = "run"
description = "Run command"
command = "echo {input}"
"#;
        let tool = ScriptTool::from_toml(toml).unwrap();
        let cmd = tool.interpolate_command(&json!({"input": "; rm -rf /"}));
        assert!(cmd.contains("'\\''"));
        assert!(!cmd.contains("; rm -rf /\""));
        // The dangerous command should be wrapped in single quotes
        assert!(cmd.contains("'; rm -rf /'"));
    }

    #[tokio::test]
    async fn script_tool_executes_command() {
        let toml = r#"
name = "echo_test"
description = "Echo test"
command = "echo hello"
"#;
        let tool = ScriptTool::from_toml(toml).unwrap();
        let result = tool
            .execute(
                "call_1",
                json!({}),
                CancellationToken::new(),
                None,
                Arc::new(std::sync::RwLock::new(crate::SessionState::new())),
                None,
            )
            .await;
        assert!(!result.is_error);
    }

    #[test]
    fn duplicate_tool_names_last_write_wins() {
        // Simulate two ScriptTools with the same name
        let tool1 = ScriptTool::from_toml(
            r#"name = "dup" description = "First" command = "echo 1""#,
        )
        .unwrap();
        let tool2 = ScriptTool::from_toml(
            r#"name = "dup" description = "Second" command = "echo 2""#,
        )
        .unwrap();

        let mut map: HashMap<PathBuf, ScriptTool> = HashMap::new();
        map.insert(PathBuf::from("/a.toml"), tool1);

        // Simulate last-write-wins
        let name = tool2.def.name.clone();
        map.retain(|_, t| t.def.name != name);
        map.insert(PathBuf::from("/b.toml"), tool2);

        assert_eq!(map.len(), 1);
        assert_eq!(map.values().next().unwrap().def.description, "Second");
    }
}
