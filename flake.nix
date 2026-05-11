{
  description = "OpenAPI Forge plugins dev shell";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs { inherit system overlays; };
        rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        # Nightly toolchain for `fuzz/`. cargo-fuzz / libFuzzer require
        # nightly; the rest of the workspace stays on stable. See
        # docs/fuzzing.md. `llvm-tools-preview` provides `llvm-cov` /
        # `llvm-profdata` so `cargo fuzz coverage` + `cargo cov` work.
        rustNightly = pkgs.rust-bin.selectLatestNightlyWith (t:
          t.default.override {
            extensions = [ "llvm-tools-preview" "rust-src" ];
          });
      in {
        devShells.default = pkgs.mkShell {
          packages = [
            rustToolchain
            pkgs.cargo-nextest
            pkgs.cargo-component
            pkgs.cargo-deny
            pkgs.cargo-insta
            pkgs.wasm-tools
            pkgs.wasmtime
            pkgs.pkg-config
            pkgs.openssl
            # Go plugin toolchain (plugins/generator-go-server).
            # TinyGo ≥ 0.34 supports `-target=wasip2` natively; pair with
            # wit-bindgen-go (installed via `go install` in build.sh).
            pkgs.go
            pkgs.tinygo
            # TypeScript plugin toolchain (plugins/generator-typescript-cli).
            # jco + componentize-js + esbuild + typescript installed via
            # plugin-local `npm ci` (see plugins/generator-typescript-cli/build.sh).
            pkgs.nodejs_22
          ];

          RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
        };

        # `nix develop .#fuzz` — nightly Rust + cargo-fuzz for working in
        # `fuzz/`. Kept separate so the default shell stays lean and the
        # main workspace never sees the nightly toolchain on PATH.
        devShells.fuzz = pkgs.mkShell {
          packages = [
            rustNightly
            pkgs.cargo-fuzz
            pkgs.pkg-config
            pkgs.openssl
          ];

          RUST_SRC_PATH = "${rustNightly}/lib/rustlib/src/rust/library";
        };
      });
}
