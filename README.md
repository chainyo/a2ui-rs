# a2ui-rs

A headless Rust SDK for [A2UI](https://a2ui.org/) v1.0. Use it to validate messages, manage surface state, resolve bindings, and dispatch actions in your app.

> [!NOTE]
> Our goal is a complete, conformant Rust implementation that can earn upstream adoption.
> This is a community project. Google and the A2UI project do not endorse or maintain this repository.

## Quick start

Add the SDK to your Cargo project.

```sh
cargo add a2ui-rs
```

Use typed messages to create a surface and update its data.

```rust
use a2ui_rs::{
    protocol::{AgentToRenderer, AgentToRendererMessage, CreateSurface, UpdateDataModel},
    validation::MessageProcessor,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let surface_id = "contact";
    let mut processor = MessageProcessor::default();
    let create = CreateSurface {
        surface_id: surface_id.into(),
        catalog_id: None,
        send_data_model: None,
        components: None,
        data_model: Some([("name".into(), "Sam".into())].into_iter().collect()),
        metadata: None,
    };
    processor.apply(AgentToRendererMessage::V1(AgentToRenderer::CreateSurface(create)))?;

    let update = UpdateDataModel {
        surface_id: surface_id.into(),
        path: Some("/name".into()),
        value: "Riley".into(),
    };
    processor.apply(AgentToRendererMessage::V1(AgentToRenderer::UpdateDataModel(update)))?;

    let contact = processor.get(surface_id).ok_or("Missing contact surface")?;
    let name = contact.data_model().get("/name")?.and_then(|value| value.as_str());
    assert_eq!(name, Some("Riley"));
    Ok(())
}
```

## Basic Catalog functions

Enable the `basic-catalog` feature to use local validators, formatting functions, and the `openUrl` host callback.

```sh
cargo add a2ui-rs --features basic-catalog
```

Load the Basic Catalog schema and register the functions your app needs. [Basic Catalog setup](https://github.com/chainyo/a2ui-rs/blob/main/docs/architecture.md#basic-catalog-functions)

## Headless examples

Run the contact form example from a repository checkout.

```sh
mise exec -- cargo run --locked --package a2ui-rs --example headless_form
```

The example mounts a tree, writes through a field binding, and prints the action with the updated value. [Example source](https://github.com/chainyo/a2ui-rs/tree/main/crates/a2ui-rs/examples/headless_form)

Run the transport example to see surface updates, an action, and an agent RPC with its reply.
The example forwards renderer errors and advances the clock to an unanswered RPC deadline.

```sh
mise exec -- cargo run --locked --package a2ui-rs --example headless_transport
```

[Transport example source](https://github.com/chainyo/a2ui-rs/tree/main/crates/a2ui-rs/examples/headless_transport)

## Documentation

- [Rust API](https://docs.rs/a2ui-rs/latest/a2ui_rs/)
- [Protocol types](https://github.com/chainyo/a2ui-rs/blob/main/docs/protocol.md)
- [Architecture](https://github.com/chainyo/a2ui-rs/blob/main/docs/architecture.md)
- [Conformance](https://github.com/chainyo/a2ui-rs/blob/main/docs/conformance.md)
- [Contributing](https://github.com/chainyo/a2ui-rs/blob/main/CONTRIBUTING.md)

## License

[Apache License 2.0](https://github.com/chainyo/a2ui-rs/blob/main/LICENSE).
