//! Regression tests for issue #19 — `generator-rust-clap` used to
//! inline every operation's handler into a single `async fn main`
//! body, so the borrow checker had to chew through one MIR unit with
//! one local-set / one CFG / one await-graph per op. On
//! medium-to-large specs that single MIR ballooned past what
//! `ubuntu-latest` can borrow-check before the runner SIGTERMs.
//!
//! Fix: emit one free-standing `async fn handle_<op_id>` per
//! operation, and shrink each `Cmd::<Variant>` arm to a single
//! dispatch call. These tests pin the *shape* — main is small and
//! contains only dispatch calls, every operation gets its own
//! top-level handler fn, and the bodies live there. We can't
//! reproduce a 464-op OOM in CI, but if the inlining regression
//! returns the assertions below all fail at once and a future
//! reviewer sees exactly which property broke.

const MAIN_RS: &str = include_str!("../../out/src/main.rs");

/// The 9 operationIds in the petstore spec. Hard-coded so the test
/// fails loudly if a spec change drops or renames one (rather than
/// silently asserting "whatever's there now").
const OPERATIONS: &[&str] = &[
    "list_pets",
    "create_pet",
    "find_pets_by_tag",
    "show_pet_by_id",
    "replace_pet",
    "list_paginated_pets",
    "get_pet_nullability_demo",
    "download_pet_photo",
    "get_pet_problem",
];

fn count_lines_in_main_body() -> usize {
    // Walk from `#[tokio::main` (the attribute that prefixes `async fn
    // main()`) until the matching closing brace at column 1.
    let start = MAIN_RS
        .find("#[tokio::main")
        .expect("expected `#[tokio::main]` attribute in generated main.rs");
    let body_start = MAIN_RS[start..]
        .find('{')
        .expect("expected `async fn main()` to have a body")
        + start;
    // Naive brace match — works because the generator's output is
    // prettyplease-formatted and balanced.
    let mut depth = 0usize;
    let mut end = body_start;
    for (i, ch) in MAIN_RS[body_start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = body_start + i;
                    break;
                }
            }
            _ => {}
        }
    }
    MAIN_RS[start..=end].lines().count()
}

/// Every operation must have its own free-standing handler fn at file
/// scope. Pre-fix the bodies all lived inside `match cli.cmd { ... }`
/// and these top-level definitions didn't exist.
#[test]
fn every_operation_has_a_top_level_handler_fn() {
    for op in OPERATIONS {
        let needle = format!("\nasync fn handle_{op}(");
        assert!(
            MAIN_RS.contains(&needle),
            "expected top-level `async fn handle_{op}(...)` in main.rs"
        );
    }
}

/// `async fn main` must shrink to pure dispatch — request building,
/// `gen::execute(...).await`, response decoding, and pagination loops
/// all live in the per-op handlers now. Lenient bound (300 lines) so
/// unrelated additions (a new builtin, OAuth pre-match plumbing) don't
/// spuriously trip; the regression we're guarding against is the
/// O(operations) growth that was 27,755 lines on the issue-reporter's
/// 464-op spec.
#[test]
fn main_body_is_O_1_in_operation_count() {
    let n = count_lines_in_main_body();
    assert!(
        n < 300,
        "async fn main body is {n} lines; expected <300. The per-op handlers \
         should be free-standing functions, not inlined into main."
    );
}

/// Each operation's match arm contains exactly one `handle_<op>(...)`
/// dispatch call. If the inlining regression came back, this would
/// stay true *for the arm body shape* but `main`'s line count would
/// blow up — paired with `main_body_is_O_1_in_operation_count` they
/// pin the structure from both sides.
#[test]
fn every_match_arm_dispatches_to_its_handler() {
    for op in OPERATIONS {
        let call = format!("handle_{op}(");
        let occurrences = MAIN_RS.matches(&call).count();
        assert!(
            occurrences >= 2,
            "expected `handle_{op}(` to appear at least twice in main.rs \
             (once as the fn definition, once as the dispatch call); \
             found {occurrences}."
        );
    }
}

/// Anti-regression on the cli-partial-move bug. The dispatch path can
/// move `cli.cmd` (via `Cmd::Group(__g) => match __g.cmd { ... }`), so
/// handlers must NOT take `&cli`. Pre-match the generator hoists the
/// fields handlers actually need (`cli.token`, `cli.output`,
/// `cli.profile` for OAuth) into owned `__token` / `__output` /
/// `__profile` locals and passes them to handlers individually. Asserts
/// the hoist and a no-references-to-`&cli`-in-dispatch invariant.
#[test]
fn handlers_receive_pre_match_clones_not_cli_borrow() {
    assert!(
        MAIN_RS.contains("let __token: Option<String> = cli.token.clone();"),
        "expected pre-match `__token` hoist in main.rs"
    );
    assert!(
        MAIN_RS.contains("let __output: runtime::OutputMode = cli.output;"),
        "expected pre-match `__output` hoist in main.rs"
    );

    // Each `handle_<op>(` definition takes `__token` / `__output` /
    // `__base_url` / `__http_client` — never `cli: &Cli`. Searching for
    // `cli: &Cli` in handler signatures would prove a regression of
    // the partial-move bug.
    for op in OPERATIONS {
        let needle = format!("async fn handle_{op}(");
        let idx = MAIN_RS
            .find(&needle)
            .unwrap_or_else(|| panic!("missing fn handle_{op}"));
        // Bounded window: 1 KiB is plenty to span any handler's params.
        let window = &MAIN_RS[idx..(idx + 1024).min(MAIN_RS.len())];
        assert!(
            !window.contains("cli: &Cli"),
            "handle_{op} signature must not take `cli: &Cli` — `match cli.cmd` \
             partially moves cli, so the dispatch arm can't pass `&cli`."
        );
    }
}
