# Contributing

## Setup

Install Git and [mise](https://mise.jdx.dev/).
Use mise `2026.11.6` or later. CI uses `2026.11.6`.

```sh
mise trust
mise install --locked
git submodule update --init
```

For a new checkout, `git clone --recurse-submodules` initializes the upstream references during the clone.

`mise.toml` pins cargo-deny, actionlint, ShellCheck, Kingfisher, and Hawk.
`mise.lock` records their download URLs and checksums for Linux, macOS, and Windows.
Mise reads the Rust version and components directly from `rust-toolchain.toml`.
There is no second Rust version declaration in `mise.toml`.
Hawk is available on Linux and macOS. Keep its version compatible with the Rust toolchain.
ShellCheck checks workflow shell commands through actionlint.

Read the [crate map](crates/README.md), [architecture](docs/architecture.md), and [conformance policy](docs/conformance.md) before structural changes.
Use the [writing guide](docs/writing.md) for documentation and comments.

## Validation

Run the same quality task as CI:

```sh
mise run check
```

The task runs formatting, all-target checks, Clippy, integration tests, documentation tests, Rustdoc, actionlint, and cargo-deny.
For focused test runs, use:

```sh
mise run test
mise exec -- cargo test --package a2ui-core --test upstream --locked
```

Use `mise exec -- COMMAND` to run a pinned tool without shell activation.
Mise also provides pinned Kingfisher for the required pre-commit scan:

```sh
mise exec -- kingfisher scan . --staged --quiet --redact --no-validate --no-update-check
```

CI initializes the submodule and runs the Rust tests on Linux, macOS, and Windows.
CI runs the full quality task on Linux.
CI does not run Hawk because this workspace contains only an external library.
Do not add an artificial binary to make its closed-world analysis applicable.

## Tool updates

Update exact tool pins in `mise.toml`, then refresh the lockfile with `mise lock`.
Update Rust in `rust-toolchain.toml`. Review Hawk compatibility when Rust or Hawk changes.
Keep supported platforms in the lockfile and review checksum changes.
Run `mise install --locked` and `mise run check` before submitting an update.
Dependabot maintains Cargo dependencies, CI actions, and the upstream submodule.
Mise tool pins require a reviewed manual update.

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
mise exec -- cargo update --package PACKAGE --precise VERSION
```

## Pull requests and releases

Use Conventional Commits for commit and pull request titles.
State the changed behavior, its reason, and the checks performed.
Keep publication disabled during bootstrap.
Do not publish packages, tags, or releases without a separate request.
