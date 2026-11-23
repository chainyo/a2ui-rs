# a2ui-rust

A framework-independent Rust SDK for [A2UI](https://a2ui.org/), targeting protocol v1.0 and later.

> [!NOTE]
> Our goal is a complete, conformant Rust implementation that can earn upstream adoption.
> This is a community project. Google and the A2UI project do not endorse or maintain this repository.

## Status

Early development. Only the protocol version type is implemented.
The runtime and upstream behavioral conformance tests are pending. No crates are published.

## Development

Install [rustup](https://rustup.rs/). The toolchain is pinned in `rust-toolchain.toml`.

```sh
cargo fmt --all -- --check
cargo clippy-all
cargo test-all
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for setup and all validation commands.

## Documentation

- [Architecture](docs/architecture.md)
- [Conformance](docs/conformance.md)
- [Implementation plan](docs/implementation-plan.md)

## License

[Apache License 2.0](LICENSE). See [NOTICE](NOTICE) for upstream attribution.
