//! Shared tower-client codegen.
//!
//! Produces the same module tree the original `generator-rust-tower`
//! plugin emitted: `mod.rs`, `runtime.rs`, `models.rs`,
//! `operations/mod.rs`, and one `operations/<snake>.rs` per kept
//! operation. Each operation lands as `impl runtime::Operation` against
//! the trait emitted inline in `runtime.rs`, so the generated tree is
//! self-contained — a consumer drops it under any module path (`pub mod
//! gen;`) and drives operations through their own `tower::Service`.
//!
//! Re-uses [`codegen_rust_serde`] for IR → Rust type lowering, model
//! emission, naming, and diagnostics. The tower-specific layer here is
//! per-operation `impl` rendering plus file-emission orchestration.

#![forbid(unsafe_code)]

pub mod emit;
mod operation_impl;

pub use emit::{all, Outcome};
