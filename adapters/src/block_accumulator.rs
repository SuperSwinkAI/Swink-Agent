//! Re-exports the shared incremental block accumulator from the core crate.
//!
//! The canonical definition now lives in `swink_agent` (re-exported at crate root).
//! This module re-exports it for backward compatibility within the adapters
//! crate.

pub use swink_agent::BlockAccumulator;
