//! Narrow public module for shared stream-assembly helpers.
//!
//! These types support adapter implementations and streaming backends, but
//! they are not part of the crate root facade.

pub use crate::block_accumulator::{BlockAccumulator, OpenBlock, StreamFinalize, finalize_blocks};
