//! Re-exports shared end-of-stream block finalization from the core crate.
//!
//! The canonical definitions now live in [`swink_agent::block_accumulator`].
//! This module re-exports them for backward compatibility within the adapters
//! crate.

pub use swink_agent::block_accumulator::{OpenBlock, StreamFinalize, finalize_blocks};
