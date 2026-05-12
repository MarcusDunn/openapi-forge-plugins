//! Thread-local collector for codegen diagnostics.
//!
//! Threading a `&mut Vec<Diagnostic>` through every `type_ref_to_rust` /
//! `render_*` call would be invasive (deeply nested, used in `models`,
//! `operation_impl`, and `types`). The plugin runs single-threaded in a
//! fresh wasm instance per invocation, so a thread-local collector is
//! safe — `emit::all` `drain`s it at the end and ships the diagnostics
//! in `GenerationOutput`.

use std::cell::RefCell;

use forge_plugin_sdk::ir::Diagnostic;

thread_local! {
    static SINK: RefCell<Vec<Diagnostic>> = const { RefCell::new(Vec::new()) };
}

/// Append a diagnostic to the per-invocation sink.
pub fn report(d: Diagnostic) {
    SINK.with(|cell| cell.borrow_mut().push(d));
}

/// Take and clear the per-invocation sink. Call once at the end of
/// `emit::all` and forward the result through `GenerationOutput`.
pub fn drain() -> Vec<Diagnostic> {
    SINK.with(|cell| std::mem::take(&mut *cell.borrow_mut()))
}
