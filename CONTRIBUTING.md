# Contributing

## Setup

Install Git and [mise](https://mise.jdx.dev/).
See `mise.toml` for the minimum mise version.

```sh
mise trust
mise install --locked
git submodule update --init
```

For a new checkout, `git clone --recurse-submodules` initializes the upstream references during the clone.

`mise.toml` pins cargo-deny, actionlint, ShellCheck, Kingfisher, and Hawk.
`mise.lock` records their download URLs and checksums for Linux, macOS, and Windows.
Mise reads the Rust version and components directly from `rust-toolchain.toml`.
Keep the Hawk version compatible with the Rust toolchain.
ShellCheck checks workflow shell commands through actionlint.

Read the [crate map](crates/README.md), [architecture](docs/architecture.md), and [conformance policy](docs/conformance.md) before structural changes.
Use ASD-STE100 for documentation and comments. Read the [writing guide](docs/writing.md).

## Validation

Run the same quality task as CI:

```sh
mise run check
```

The task checks build targets with and without the Basic Catalog feature.
It also runs formatting, Clippy, Rustdoc, actionlint, and cargo-deny.
It compiles test targets without executing tests.
Run tests locally for behavior changes and before a package release:

```sh
mise run test
mise exec -- cargo test --package a2ui-rs --features catalog --test upstream --locked
```

Use `mise exec -- COMMAND` to run a pinned tool without shell activation.
Mise also provides pinned Kingfisher for the required pre-commit scan:

```sh
mise exec -- kingfisher scan . --staged --quiet --redact --no-validate --no-update-check
```

PR and main-branch CI run the quality task on Linux without executing tests or initializing the submodule.
[Package validation](.github/workflows/package-validation.yml) runs integration and documentation tests on Linux, macOS, and Windows when a `v*` tag is pushed.
A publishing workflow can call package validation and require its success before publication.
Use Hawk only for an internal product with a suitable analysis target.

## Tool updates

Update exact tool pins in `mise.toml`, then refresh the lockfile with `mise lock`.
Update Rust in `rust-toolchain.toml`. Review Hawk compatibility when Rust or Hawk changes.
Keep supported platforms in the lockfile and review checksum changes.
Run `mise install --locked` and `mise run check` before submitting an update.
Dependabot maintains Cargo dependencies, CI actions, and the upstream submodule.
Mise tool pins require a reviewed manual update.

## Release validation

Run the release tests locally with the pinned submodule initialized:

```sh
git submodule update --init
mise run test
```

Version tags identify SDK releases. They do not identify the A2UI protocol version.

Verify the package archive with the Basic Catalog feature.

```sh
mise exec -- cargo package --package a2ui-rs --all-features --locked
```

The archive omits integration tests that use fixtures from the repository.
Create a release tag only when a release is explicitly requested.

## Fixtures

Add small, realistic examples under `fixtures/local/`.
Include rejection cases and incremental message sequences when relevant.
Use fictional data and public protocol identifiers.
Tests must exercise production APIs instead of a test-only protocol implementation.

The version tests check protocol selection. Protocol tests check typed envelopes and upstream renderer fixtures.
The common-type runner executes supported upstream cases and reports all remaining cases with reasons.

Upstream references live in a Git submodule. Do not edit its files.
Update the pin only through the process in `docs/conformance.md`.
Do not replace negative expectations with successful examples.

## Dependencies

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
Do not publish packages, tags, or releases without a separate request.
