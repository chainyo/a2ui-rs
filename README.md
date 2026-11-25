# a2ui-rs

A framework-independent Rust SDK for [A2UI](https://a2ui.org/), targeting protocol v1.0 and later.

> [!NOTE]
> Our goal is a complete, conformant Rust implementation that can earn upstream adoption.
> This is a community project. Google and the A2UI project do not endorse or maintain this repository.

Enable the `basic-catalog` feature to register the Basic Catalog functions.
See [Basic Catalog functions](docs/architecture.md#basic-catalog-functions).

## Headless example

Run the contact form example with Cargo.

```sh
mise exec -- cargo run --locked --package a2ui-rs --example headless_form
```

The example mounts a surface and writes through a field binding.
It prints the updated name and the action message that contains it.
See the [example source](crates/a2ui-rs/examples/headless_form/main.rs).

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
