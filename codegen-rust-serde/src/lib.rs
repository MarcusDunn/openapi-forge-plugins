//! Shared IR → Rust codegen building blocks.
//!
//! Consumed by openapi-forge generator plugins that emit Rust. The
//! pieces here are deliberately scope-bound to "produce serde-derived
//! type / model definitions and the helpers needed to do so":
//!
//! - [`models`] — render `models.rs` from `Ir::types`: one
//!   `#[derive(Serialize, Deserialize)]` definition per named type.
//! - [`types`] — IR `TypeRef` / `TypeDef` → Rust type expression as a
//!   `TokenStream`. Strict by default; unmodellable shapes go through
//!   [`diagnostics::report_fatal`].
//! - [`naming`] — identifier sanitization (`snake_case`, `pascal_case`,
//!   `rust_type_name`), keyword/raw-ident escaping.
//! - [`diagnostics`] — thread-local sink for codegen diagnostics so
//!   deeply-nested rendering paths don't have to thread a `&mut Vec`.
//!
//! Tower- or CLI-specific concerns (per-op `impl` blocks, runtime
//! templates, output orchestration via `prettyplease`) belong in
//! consumer crates.

#![forbid(unsafe_code)]

pub mod diagnostics;
pub mod models;
pub mod naming;
pub mod types;
