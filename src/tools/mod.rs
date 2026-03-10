//! Built-in tools for common file and shell operations.

mod bash;
mod read_file;
mod write_file;

pub use bash::BashTool;
pub use read_file::ReadFileTool;
pub use write_file::WriteFileTool;

/// Maximum output size in bytes before truncation, shared across tools.
pub(crate) const MAX_OUTPUT_BYTES: usize = 100 * 1024;
