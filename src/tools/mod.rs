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
