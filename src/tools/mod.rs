//! Built-in tools for common file and shell operations.

#[cfg(feature = "builtin-tools")]
mod bash;
#[cfg(feature = "builtin-tools")]
mod read_file;
#[cfg(feature = "builtin-tools")]
mod write_file;

#[cfg(feature = "builtin-tools")]
pub use bash::BashTool;
#[cfg(feature = "builtin-tools")]
pub use read_file::ReadFileTool;
#[cfg(feature = "builtin-tools")]
pub use write_file::WriteFileTool;

/// Maximum output size in bytes before truncation, shared across tools.
#[cfg(feature = "builtin-tools")]
pub(crate) const MAX_OUTPUT_BYTES: usize = 100 * 1024;

/// Returns all built-in tools (`BashTool`, `ReadFileTool`, `WriteFileTool`)
/// wrapped in `Arc`, ready to pass to an [`Agent`](crate::Agent).
#[cfg(feature = "builtin-tools")]
pub fn builtin_tools() -> Vec<std::sync::Arc<dyn crate::tool::AgentTool>> {
    vec![
        std::sync::Arc::new(BashTool::new()),
        std::sync::Arc::new(ReadFileTool::new()),
        std::sync::Arc::new(WriteFileTool::new()),
    ]
}

#[cfg(feature = "artifact-tools")]
mod save_artifact;
#[cfg(feature = "artifact-tools")]
mod load_artifact;
#[cfg(feature = "artifact-tools")]
mod list_artifacts;

#[cfg(feature = "artifact-tools")]
pub use save_artifact::SaveArtifactTool;
#[cfg(feature = "artifact-tools")]
pub use load_artifact::LoadArtifactTool;
#[cfg(feature = "artifact-tools")]
pub use list_artifacts::ListArtifactsTool;

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
