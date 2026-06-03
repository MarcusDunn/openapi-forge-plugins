# generator-rust-clap

OpenAPI Forge generator that emits a Rust CLI crate (clap derive + reqwest) for an OpenAPI spec.

**Status: shipping.** The plugin emits a buildable Rust CLI crate with:

- One clap subcommand per OpenAPI operation, grouped by tag (OAS 3.2 `parent`-aware nesting).
- OAuth 2.0 PKCE login/logout when the spec declares an `oauth2.authorizationCode` flow + plugin config supplies `clientId`. Optional `client_secret_basic` on the token endpoint via `oauth.clientSecretEnv` (env-var-supplied).
- RFC 8693 standard token exchange driven by a generic `x-token-exchange` extension on the spec's `oauth2` security scheme — operations whose path includes the placeholder use a separately-audienced JWT.
- Shell completions for bash / zsh / fish / powershell / elvish via `clap_complete`: `<bin> completion <shell>`. Dynamic value completion is also wired up unconditionally (`eval "$(COMPLETE=bash <bin>)"`), so **enum-typed query params complete their allowed values** — `<bin> pets list --status <TAB>` offers `available` / `inProgress` / `sold` straight from the spec's `enum`.
- **Schema `default`s become clap `default_value`s.** A query/header/path param whose schema declares a scalar `default` (string / number / boolean) fills that value when the flag is omitted, shows it in `--help` as `[default: …]`, and sends it on the request — matching the server-side default the spec documents. Compound (array/object) defaults are left out (no single-token CLI form).
- Runtime env-var overrides for `<PREFIX>_AUTH_URL` / `<PREFIX>_TOKEN_URL` / `<PREFIX>_BASE_URL` / `<PREFIX>_TOKEN` so a single binary moves between dev / staging / prod.
- Schema discovery on each operation: `--body-schema` prints the JSON Schema for the request body; `--response-schema` prints a status→schema map for response bodies. `--help` stays lean — schemas only appear when asked.
- `curl`-style body input on every body-having operation: `--body '{"k":"v"}'` (inline JSON), `--body @file.json` (read from file), or `--body -` (read from stdin).

This is an *external* plugin — it depends on the published
[`forge-plugin-sdk`](https://crates.io/crates/forge-plugin-sdk) crate, not on
the in-tree workspace. Its purpose is partly to surface rough edges in the
SDK from a downstream-consumer perspective.

## Use

```toml
# forge.toml
[generator]
oci = "ghcr.io/marcusdunn/generator-rust-clap:latest"
```

### Discovering body shape

The generated CLI keeps `--help` short and exposes the spec's schemas
behind two flags. Use them to find out what to send and what comes back
without leaving the terminal:

```sh
$ <bin> users create --body-schema
{ "$schema": "...", "type": "object", "required": ["name"], ... }

$ <bin> users create --response-schema
{ "201": { ... }, "400": { ... }, "$defs": { ... } }

# Pipe to jq to drill into a single status:
$ <bin> users create --response-schema | jq '."201"'
```

Bodies accept inline JSON, an `@file` path, or `-` for stdin — the same
convention as `curl --data`:

```sh
$ <bin> users create --body '{"name":"Marcus"}'
$ <bin> users create --body @user.json
$ cat user.json | <bin> users create --body -
```

Pin by digest for reproducibility:

```toml
[generator]
oci = "ghcr.io/marcusdunn/generator-rust-clap@sha256:…"
```

## Build and release

See the [workspace README](../README.md) — this crate builds and ships
alongside the other plugins via the workspace's `ci.yml` and
`release.yml`.

## License

Apache-2.0 OR MIT
