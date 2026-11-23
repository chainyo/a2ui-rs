# Contributing

## Setup

Install Rust through rustup. The exact build toolchain is in `rust-toolchain.toml`.
Install cargo-deny `0.20.2` and Hawk `0.1.9` as described in the README.
Keep the toolchain and Hawk versions compatible when updating either.

Read the [crate map](crates/README.md), [architecture](docs/architecture.md), and [conformance policy](docs/conformance.md) before structural changes.
Use the [writing guide](docs/writing.md) for documentation and comments.

## Validation

Run these checks before a pull request:

```sh
cargo fmt --all -- --check
cargo xtask-check
cargo xtask-clippy
cargo xtask-test
cargo test --workspace --doc --locked
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --locked
cargo xtask upstream
cargo deny --locked check
cargo hawk check --manifest-path Cargo.toml --target-dir target/hawk --exclude-crate a2ui_core -D warnings -D hawk::unnecessary_crate_visibility
bash -n scripts/install-hawk.sh
shellcheck scripts/install-hawk.sh
actionlint
```

CI runs the Rust tests on Linux, macOS, and Windows.
CI runs the full quality checks on Linux.
The Core SDK is an external library boundary for Hawk.
Do not reduce its public API because a workspace binary does not call it.

## Fixtures

Add small, realistic examples under `fixtures/local/`.
Include rejection cases and incremental message sequences when relevant.
Use fictional data and public protocol identifiers.
Tests must exercise production APIs instead of a test-only protocol implementation.

The bootstrap version tests inspect message versions only.
They do not validate the complete local or upstream message payloads.

Imported upstream references retain their original bytes and license notices.
Update them only through the process in `docs/conformance.md`.
Do not replace negative expectations with successful examples.

## Dependencies

The public Core SDK depends on Serde and `thiserror` 2.
Keep extra parsing, hashing, and audit dependencies in repository tooling.
Prefer maintained stable releases. Disable unnecessary default features.
Commit `Cargo.lock` and review license, source, advisory, and duplicate-version checks.
Use a scoped update when changing a dependency:

```sh
cargo update --package PACKAGE --precise VERSION
```

## Pull requests and releases

Use Conventional Commits for commit and pull request titles.
State the changed behavior, its reason, and the checks performed.
Keep publication disabled during bootstrap.
Do not publish packages, tags, or releases without a separate request.
