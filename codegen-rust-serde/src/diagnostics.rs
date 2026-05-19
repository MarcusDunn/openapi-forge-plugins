//! Thread-local collector for codegen diagnostics.
//!
//! Threading a `&mut Vec<Diagnostic>` through every `type_ref_to_rust` /
//! `render_*` call would be invasive (deeply nested, used in `models`,
//! `operation_impl`, and `types`). The plugin runs single-threaded in a
//! fresh wasm instance per invocation, so a thread-local collector is
//! safe — `emit::all` `drain`s it at the end and ships the diagnostics
//! in `GenerationOutput` or a `StageError::Rejected`.

use std::cell::{Cell, RefCell};

use forge_plugin_sdk::ir::Diagnostic;

thread_local! {
    static SINK: RefCell<Vec<Diagnostic>> = const { RefCell::new(Vec::new()) };
    static FATAL: Cell<usize> = const { Cell::new(0) };
}

/// Append a diagnostic that should cause `emit::all` to fail the
/// generation. The token stream emitted at the call site still needs to
/// be valid Rust so `syn::parse2` doesn't panic before we reach the
/// fatal-count check — callers conventionally fall back to
/// `serde_json::Value` for that purpose.
pub fn report_fatal(d: Diagnostic) {
    FATAL.with(|c| c.set(c.get() + 1));
    SINK.with(|cell| cell.borrow_mut().push(d));
}

/// Append a non-fatal diagnostic (warning / info / hint). Surfaces to
/// the consumer via `GenerationOutput.diagnostics` but does not flip
/// the `Outcome::Rejected` switch.
pub fn report(d: Diagnostic) {
    SINK.with(|cell| cell.borrow_mut().push(d));
}

/// Take and clear the per-invocation sink. Call once at the end of
/// `emit::all`.
pub fn drain() -> Vec<Diagnostic> {
    SINK.with(|cell| std::mem::take(&mut *cell.borrow_mut()))
}

/// Take and clear the per-invocation fatal counter.
pub fn take_fatal_count() -> usize {
    FATAL.with(|c| c.replace(0))
}
