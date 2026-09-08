//! Sandbox policy — restricts file paths to an allowed root directory.
#![forbid(unsafe_code)]

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use swink_agent::{PreDispatchPolicy, PreDispatchVerdict, ToolDispatchContext};

/// Rejects tool calls that reference file paths outside an allowed root directory.
///
/// Inspects string values in configured argument field names (default: `["path", "file_path", "file"]`).
/// Skips with a descriptive error on violation — does not silently rewrite paths.
///
/// # Example
/// ```rust,ignore
/// use swink_agent_policies::SandboxPolicy;
/// use swink_agent::AgentOptions;
///
/// let opts = AgentOptions::new(...)
///     .with_pre_dispatch_policy(SandboxPolicy::new("/tmp/workspace"));
/// ```
#[derive(Debug, Clone)]
pub struct SandboxPolicy {
    allowed_root: PathBuf,
    path_fields: Vec<String>,
}

impl SandboxPolicy {
    /// Create a new `SandboxPolicy` with the given allowed root.
    ///
    /// Default path fields: `["path", "file_path", "file"]`.
    pub fn new(allowed_root: impl Into<PathBuf>) -> Self {
        Self {
            allowed_root: allowed_root.into(),
            path_fields: vec![
                "path".to_string(),
                "file_path".to_string(),
                "file".to_string(),
            ],
        }
    }

    /// Override the argument field names to check for file paths.
    #[must_use]
    pub fn with_path_fields(mut self, fields: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.path_fields = fields.into_iter().map(Into::into).collect();
        self
    }

    fn validate_path(&self, path_str: &str, execution_root: Option<&Path>) -> Result<(), String> {
        let allowed_root = std::fs::canonicalize(&self.allowed_root).map_err(|err| {
            format!(
                "sandbox allowed root '{}' is unavailable: {err}",
                self.allowed_root.display()
            )
        })?;
        let resolved_path = self.resolve_path(Path::new(path_str), execution_root)?;

        if resolved_path.starts_with(&allowed_root) {
            Ok(())
        } else {
            Err(format!(
                "path '{}' resolves outside allowed root '{}'",
                resolved_path.display(),
                allowed_root.display()
            ))
        }
    }

    fn resolve_path(&self, path: &Path, execution_root: Option<&Path>) -> Result<PathBuf, String> {
        let _ = self; // Future use for per-instance resolution config.
        let base_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            let execution_root = execution_root.ok_or_else(|| {
                format!(
                    "relative path '{}' cannot be validated without an execution root",
                    path.display()
                )
            })?;
            let execution_root = std::fs::canonicalize(execution_root).map_err(|err| {
                format!(
                    "execution root '{}' is unavailable: {err}",
                    execution_root.display()
                )
            })?;
            execution_root.join(path)
        };

        Self::resolve_existing_prefix(&base_path)
    }

    fn resolve_existing_prefix(path: &Path) -> Result<PathBuf, String> {
        let mut unresolved_components: Vec<OsString> = Vec::new();
        let mut probe = path;

        loop {
            match std::fs::symlink_metadata(probe) {
                Ok(_) => {
                    let mut resolved = std::fs::canonicalize(probe).map_err(|err| {
                        format!("failed to canonicalize '{}': {err}", probe.display())
                    })?;
                    for component in unresolved_components.iter().rev() {
                        match Path::new(component).components().next() {
                            Some(Component::CurDir) => {}
                            Some(Component::ParentDir) => {
                                resolved.pop();
                            }
                            Some(Component::Normal(part)) => resolved.push(part),
                            Some(Component::RootDir | Component::Prefix(_)) | None => {
                                return Err(format!(
                                    "path '{}' contains an unsupported component",
                                    path.display()
                                ));
                            }
                        }
                    }
                    return Ok(resolved);
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    let component = probe.file_name().ok_or_else(|| {
                        format!(
                            "failed to resolve path '{}': no existing ancestor",
                            path.display()
                        )
                    })?;
                    unresolved_components.push(component.to_os_string());
                    probe = probe.parent().ok_or_else(|| {
                        format!(
                            "failed to resolve path '{}': no existing ancestor",
                            path.display()
                        )
                    })?;
                }
                Err(err) => {
                    return Err(format!(
                        "failed to inspect path '{}': {err}",
                        probe.display()
                    ));
                }
            }
        }
    }
}

impl PreDispatchPolicy for SandboxPolicy {
    fn name(&self) -> &'static str {
        "sandbox"
    }

    fn evaluate(&self, ctx: &mut ToolDispatchContext<'_>) -> PreDispatchVerdict {
        let Some(obj) = ctx.arguments.as_object() else {
            return PreDispatchVerdict::Continue;
        };

        for field_name in &self.path_fields {
            if let Some(serde_json::Value::String(path_str)) = obj.get(field_name.as_str())
                && let Err(reason) = self.validate_path(path_str, ctx.execution_root)
            {
                return PreDispatchVerdict::Skip(format!(
                    "path '{}' in field '{}' is outside allowed root '{}': {}",
                    path_str,
                    field_name,
                    self.allowed_root.display(),
                    reason
                ));
            }
        }

        PreDispatchVerdict::Continue
    }
}

#[cfg(test)]
#[path = "sandbox_tests.rs"]
mod tests;
