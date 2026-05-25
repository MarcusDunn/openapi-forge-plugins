// The compile-test crate exists solely to host the integration tests
// under `tests/`. The tests read files from `../out/` (the directory
// `forge generate` populates) and assert structural properties of the
// emitted HTML. No runtime types live here.
