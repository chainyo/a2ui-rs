# Contributing

## Setup

Install Git. Install Rust through rustup. The exact Rust toolchain is in `rust-toolchain.toml`.
Initialize the pinned upstream references:

```sh
git submodule update --init
```

For a new checkout, `git clone --recurse-submodules` initializes them during the clone.
Install cargo-deny `0.20.2`:

```sh
cargo install cargo-deny --version 0.20.2 --locked
```

The Hawk `0.1.9` installer is optional:

```sh
./scripts/install-hawk.sh
```

Keep the toolchain and Hawk versions compatible when updating either.

Read the [crate map](crates/README.md), [architecture](docs/architecture.md), and [conformance policy](docs/conformance.md) before structural changes.
Use the [writing guide](docs/writing.md) for documentation and comments.

## Validation

Run these checks before a pull request:

```sh
cargo fmt --all -- --check
cargo check-all
cargo clippy-all
cargo test-all
cargo test --workspace --doc --locked
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --locked
cargo deny --locked check
bash -n scripts/install-hawk.sh
shellcheck scripts/install-hawk.sh
actionlint
```

CI initializes the pinned submodule and runs the Rust tests on Linux, macOS, and Windows.
CI runs the full quality checks on Linux.
Hawk remains available through the pinned installer.
CI does not run Hawk because this workspace contains only an external library.
Do not add an artificial binary to make its closed-world analysis applicable.

## Fixtures

Add small, realistic examples under `fixtures/local/`.
Include rejection cases and incremental message sequences when relevant.
Use fictional data and public protocol identifiers.
Tests must exercise production APIs instead of a test-only protocol implementation.

The bootstrap version tests inspect message versions only.
They do not validate the complete local or upstream message payloads.

Upstream references live in a Git submodule. Do not edit its files.
Update the pin only through the process in `docs/conformance.md`.
Do not replace negative expectations with successful examples.

## Dependencies

The public Core SDK depends on Serde and `thiserror` 2.
Keep YAML parsing in dev-dependencies for integration tests.
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
