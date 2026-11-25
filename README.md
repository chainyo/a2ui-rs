# a2ui-rs

A headless Rust SDK for [A2UI](https://a2ui.org/) v1.0. Use it to validate messages, manage surface state, resolve bindings, and dispatch actions in your app.

> [!NOTE]
> Our goal is a complete, conformant Rust implementation that can earn upstream adoption.
> This is a community project. Google and the A2UI project do not endorse or maintain this repository.

## Quick start

Add the SDK and the JSON library used in this example.

```sh
cargo add a2ui-rs serde_json
```

Create a surface, apply a data update, and read the updated value.

```rust
use a2ui_rs::validation::MessageProcessor;
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut processor = MessageProcessor::default();
    processor.apply_json(&json!([
        {
            "version": "1.0",
            "createSurface": {
                "surfaceId": "contact",
                "dataModel": { "name": "Sam" }
            }
        },
        {
            "version": "1.0",
            "updateDataModel": {
                "surfaceId": "contact",
                "path": "/name",
                "value": "Riley"
            }
        }
    ]))?;

    let contact = processor.get("contact").ok_or("Missing contact surface")?;
    assert_eq!(contact.data_model().get("/name")?, Some(&json!("Riley")));
    Ok(())
}
```

## Basic Catalog functions

Enable the `basic-catalog` feature to use local validators, formatting functions, and the `openUrl` host callback.

```sh
cargo add a2ui-rs --features basic-catalog
```

Load the Basic Catalog schema and register the functions your app needs. [Basic Catalog setup](https://github.com/chainyo/a2ui-rs/blob/main/docs/architecture.md#basic-catalog-functions)

## Headless example

Run the contact form example from a repository checkout.

```sh
mise exec -- cargo run --locked --package a2ui-rs --example headless_form
```

The example mounts a tree, writes through a field binding, and prints the action with the updated value. [Example source](https://github.com/chainyo/a2ui-rs/tree/main/crates/a2ui-rs/examples/headless_form)

## Documentation

- [Rust API](https://docs.rs/a2ui-rs/latest/a2ui_rs/)
- [Protocol types](https://github.com/chainyo/a2ui-rs/blob/main/docs/protocol.md)
- [Architecture](https://github.com/chainyo/a2ui-rs/blob/main/docs/architecture.md)
- [Conformance](https://github.com/chainyo/a2ui-rs/blob/main/docs/conformance.md)
- [Contributing](https://github.com/chainyo/a2ui-rs/blob/main/CONTRIBUTING.md)

## License

[Apache License 2.0](https://github.com/chainyo/a2ui-rs/blob/main/LICENSE).
