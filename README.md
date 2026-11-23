# a2ui-rs

A framework-independent Rust SDK for [A2UI](https://a2ui.org/), targeting protocol v1.0 and later.

> [!NOTE]
> Our goal is a complete, conformant Rust implementation that can earn upstream adoption.
> This is a community project. Google and the A2UI project do not endorse or maintain this repository.

## Development

Install [mise](https://mise.jdx.dev/). Use the commands below to install the pinned tools and run the checks.

```sh
mise trust
mise install --locked
git submodule update --init
mise run check
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for setup and validation commands.

## Documentation

- [Protocol types](docs/protocol.md)
- [Architecture](docs/architecture.md)
- [Conformance](docs/conformance.md)
- [Implementation plan](docs/implementation-plan.md)

## License

[Apache License 2.0](LICENSE).
