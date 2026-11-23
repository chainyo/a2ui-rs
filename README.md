# a2ui-rust

A framework-independent Rust SDK for [A2UI](https://a2ui.org/), targeting protocol v1.0 and later.

> [!NOTE]
> Our goal is a complete, conformant Rust implementation that can earn upstream adoption.
> This is a community project. Google and the A2UI project do not endorse or maintain this repository.

## Status

Early development. `a2ui-rust-core` provides typed v1.0 messages, Serde support, and initial upstream conformance tests.
The state engine and catalog validation are pending. No crates are published.

## Development

Install [mise](https://mise.jdx.dev/). Tool versions are pinned in the repository.

```sh
mise trust
mise install --locked
git submodule update --init
mise run check
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for setup and all validation commands.

## Documentation

- [Wire types](docs/wire.md)
- [Architecture](docs/architecture.md)
- [Conformance](docs/conformance.md)
- [Implementation plan](docs/implementation-plan.md)

## License

[Apache License 2.0](LICENSE).
