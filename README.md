# openapi-forge-plugins

[openapi-forge](https://github.com/marcusdunn/openapi-forge) plugins
distributed as public OCI WASM components.

## Plugins

| Plugin | OCI ref | Purpose |
|---|---|---|
| [`generator-rust-tower`](generator-rust-tower/) | `ghcr.io/marcusdunn/generator-rust-tower:<tag>` | Emit a Rust client where each operation is one `impl Operation` block against a tiny `Operation` trait. Drives the operations through any `tower::Service<http::Request<…>>` the caller supplies. |
| [`generator-rust-clap`](generator-rust-clap/) | `ghcr.io/marcusdunn/generator-rust-clap:<tag>` | Emit a complete Rust CLI crate (`clap` derive + `reqwest`). One subcommand per operation, grouped by tag; OAuth 2.0 PKCE login when the spec declares an `authorizationCode` flow; shell completions; `--body-schema` / `--response-schema` discovery flags. |
| [`transformer-filter-operations`](transformer-filter-operations/) | `ghcr.io/marcusdunn/transformer-filter-operations:<tag>` | Keep only a configured set of `operationId`s (and the named types they transitively reach). Drops everything else. |

The generator emits the `Operation` trait and an `execute` helper inline
in the generated crate (`src/runtime.rs`). There is no separate runtime
dependency on crates.io — generated crates are self-contained except for
`http`, `http-body-util`, `tower`, `bytes`, `serde`, `serde_json`, and
`thiserror`.

## Using the plugins

```toml
# forge.toml
[input]
spec = "openapi.json"

[[transformers]]
oci = "ghcr.io/marcusdunn/transformer-filter-operations:v0.1.0"
config = { keep = ["createUser", "getUser"] }

[generator]
oci = "ghcr.io/marcusdunn/generator-rust-tower:v0.1.0"
config = { crateName = "my-api-client" }

[output]
dir = "gen"
```

```bash
forge generate
```

## Building locally

```bash
rustup target add wasm32-wasip2
cargo build --release --target wasm32-wasip2
```

The `.wasm` artifacts land under `target/wasm32-wasip2/release/`.

## Releasing

Publish a GitHub Release (via the UI, `gh release create`, or
release-please). The `release.yml` workflow fires on the
`release: { types: [published] }` event, builds the plugins, and
`oras push`es each one to `ghcr.io/marcusdunn/<plugin>:<tag>` with the
media type
`application/vnd.bytecodealliance.wasm.component.layer.v0+wasm`.

GHCR packages must be set to public via the repo settings UI after the
first release.

## License

Dual-licensed under Apache-2.0 or MIT, at your option.
