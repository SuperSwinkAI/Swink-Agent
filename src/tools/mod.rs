//! Built-in tools for common file and shell operations.

#[cfg(feature = "builtin-tools")]
mod bash;
#[cfg(feature = "builtin-tools")]
mod edit_file;
#[cfg(feature = "builtin-tools")]
mod path;
#[cfg(feature = "builtin-tools")]
mod read_file;
#[cfg(feature = "builtin-tools")]
mod write_file;

#[cfg(feature = "builtin-tools")]
pub use bash::BashTool;
#[cfg(feature = "builtin-tools")]
pub use edit_file::EditFileTool;
#[cfg(feature = "builtin-tools")]
pub use read_file::ReadFileTool;
#[cfg(feature = "builtin-tools")]
pub use write_file::WriteFileTool;

/// Maximum output size in bytes before truncation, shared across tools.
#[cfg(feature = "builtin-tools")]
pub(crate) const MAX_OUTPUT_BYTES: usize = 100 * 1024;

#[cfg(feature = "builtin-tools")]
const TRUNCATED_MARKER: &str = "\n[truncated]";

#[cfg(feature = "builtin-tools")]
pub(crate) fn truncate_utf8_to_boundary(text: &mut String, max_bytes: usize) {
    let mut end = max_bytes.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }

    text.truncate(end);
    text.push_str(TRUNCATED_MARKER);
}

/// Returns all built-in tools (`BashTool`, `ReadFileTool`, `WriteFileTool`)
/// wrapped in `Arc`, ready to pass to an [`Agent`](crate::Agent).
#[cfg(feature = "builtin-tools")]
pub fn builtin_tools() -> Vec<std::sync::Arc<dyn crate::tool::AgentTool>> {
    vec![
        std::sync::Arc::new(BashTool::new()),
        std::sync::Arc::new(EditFileTool::new()),
        std::sync::Arc::new(ReadFileTool::new()),
        std::sync::Arc::new(WriteFileTool::new()),
    ]
}

#[cfg(feature = "artifact-tools")]
mod list_artifacts;
#[cfg(feature = "artifact-tools")]
mod load_artifact;
#[cfg(feature = "artifact-tools")]
mod save_artifact;

#[cfg(feature = "artifact-tools")]
pub use list_artifacts::ListArtifactsTool;
#[cfg(feature = "artifact-tools")]
pub use load_artifact::LoadArtifactTool;
#[cfg(feature = "artifact-tools")]
pub use save_artifact::SaveArtifactTool;

/// Create all built-in artifact tools (save, load, list) backed by the given store.
#[cfg(feature = "artifact-tools")]
pub fn artifact_tools<S: crate::artifact::ArtifactStore + 'static>(
    store: std::sync::Arc<S>,
) -> Vec<std::sync::Arc<dyn crate::tool::AgentTool>> {
    vec![
        std::sync::Arc::new(SaveArtifactTool::new(store.clone())),
        std::sync::Arc::new(LoadArtifactTool::new(store.clone())),
        std::sync::Arc::new(ListArtifactsTool::new(store)),
    ]
}

#[cfg(test)]
mod tests;
