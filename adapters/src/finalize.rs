//! Re-exports shared end-of-stream block finalization from the core crate.
//!
//! The canonical definitions now live in `swink_agent::stream_assembly`.

pub use swink_agent::stream_assembly::{OpenBlock, StreamFinalize, finalize_blocks};
