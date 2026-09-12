//! Single integration-test binary for this crate.
//!
//! Every `tests/*.rs` file is a module here rather than its own executable:
//! each separate test binary statically links the whole dependency graph,
//! and dozens of them across the workspace were most of a 100+ GB `target/`.
//! New test files must be added as a `mod` line below (autotests is off).

mod end_to_end;
mod peer;
