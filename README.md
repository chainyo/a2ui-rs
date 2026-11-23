# a2ui-rust

A framework-independent Rust SDK for [A2UI](https://a2ui.org/), starting with protocol v1.0.

Our goal is a complete, conformant Rust implementation that can earn upstream adoption.
This is a community project. Google and the A2UI project do not endorse or maintain this repository.

## Status

This repository contains the workspace foundation. It is not a usable A2UI runtime yet.
No crate is published. Package names are provisional until registry availability is confirmed.
The crate version `0.1.0` is an SDK version, not an A2UI protocol version.

Implemented:

- A typed protocol version boundary with Serde support.
- Tests that accept `1.0` and reject legacy, unknown, and malformed versions.
- Version checks against realistic upstream contact and dashboard examples.
- Pinned upstream blueprints, v1.0 schemas, and Core conformance references.
- A reference integrity check and an explicit inventory of pending conformance cases.

Not implemented:

- Typed message payloads and catalog validation.
- Surface, component, and data model state.
- Message processing, JSON Pointer resolution, reactive bindings, and expressions.
- Function validation, RPC routing, cancellation, and lifecycle management.
- Agent helpers and UI adapters.

**Upstream behavioral conformance cases executed: zero.**
The inventory reports pending work. Passing CI does not establish protocol conformance.
See [conformance](docs/conformance.md) for the coverage contract.

## Protocol scope

Support starts at A2UI v1.0. We will add later versions through explicit, tested implementations.
A2UI v0.8, v0.9, and v0.9.1 are outside scope.
Unknown future versions are rejected until implemented.
There is no legacy compatibility layer.

## Architecture

The Core SDK owns protocol types, validation, state, data binding, and message processing.
It must work without a UI framework, network client, model provider, or async runtime.

Agent helpers will depend on the Core SDK.
A GPUI renderer can also depend on the Core SDK through a separate adapter crate.
Neither belongs in the Core SDK. No UI adapter is implemented in this bootstrap.

Read the [architecture](docs/architecture.md) and [crate map](crates/README.md).

## Development

Install [rustup](https://rustup.rs/). The repository pins Rust `1.97.1`, Clippy, and rustfmt.
Use the toolchain file as the build requirement. An older MSRV is not promised.

```sh
rustup show
cargo fmt --all -- --check
cargo xtask-check
cargo xtask-clippy
cargo xtask-test
cargo test --workspace --doc --locked
cargo xtask upstream
```

Install the development checks:

```sh
cargo install cargo-deny --version 0.20.2 --locked
./scripts/install-hawk.sh
cargo deny --locked check
cargo hawk check --manifest-path Cargo.toml --target-dir target/hawk --exclude-crate a2ui_core -D warnings -D hawk::unnecessary_crate_visibility
```

The installer verifies Hawk `0.1.9` against pinned archive checksums.
That Hawk release requires the pinned Rust toolchain.
Hawk checks repository tooling. It excludes the Core SDK because its public callers live outside this workspace.
Clippy, Rustdoc, and integration tests check the public SDK boundary.

See [CONTRIBUTING.md](CONTRIBUTING.md) for all checks and fixture rules.
The workspace follows the crate layout and tooling approach of [rudolint](https://github.com/kubeply/rudolint).

## Upstream foundation

The imported references are pinned in [manifest.json](fixtures/upstream/manifest.json).
They include these upstream contracts:

- [Core SDK blueprint](https://github.com/a2ui-project/a2ui/blob/main/blueprints/modules/a2ui_core.blueprint.md).
- [Agent SDK blueprint](https://github.com/a2ui-project/a2ui/blob/main/blueprints/modules/a2ui_agent.blueprint.md).
- [Framework adapter blueprint](https://github.com/a2ui-project/a2ui/blob/main/blueprints/modules/a2ui_framework_adapter.blueprint.md).
- [Core conformance suites](https://github.com/a2ui-project/a2ui/tree/main/conformance/core).

The manifest revision is the review baseline. Links to `main` show current upstream documents.
See the [implementation plan](docs/implementation-plan.md) before starting a feature.

## License

[Apache License 2.0](LICENSE). See [NOTICE](NOTICE) for upstream reference attribution.
