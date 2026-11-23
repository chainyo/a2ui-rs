//! Tests the public v1.0 wire boundary without a renderer or state engine.

use std::error::Error;
use std::fs;
use std::path::Path;

use a2ui_rs::wire::{
    Action, AgentToRenderer, AgentToRendererMessage, GenericError, MessageBatch,
    RendererToAgentMessage,
};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn round_trips_all_agent_operations() -> TestResult {
    let operations = [
        json!({"createSurface":{"surfaceId":"contact","sendDataModel":false,"dataModel":{},"metadata":{"extensions":{}}}}),
        json!({"updateComponents":{"surfaceId":"contact","components":[{"id":"root","component":"Text","text":{"@path":"/name"},"@catalogId":"example","@metadata":{"extensions":{"audit":1}}}]}}),
        json!({"updateDataModel":{"surfaceId":"contact","path":"/name","value":null}}),
        json!({"deleteSurface":{"surfaceId":"contact"}}),
        json!({"callRendererFunction":{"functionCallId":"call","callFunction":{"@call":"openUrl","catalogId":"example","args":{"url":"https://example.com"}}}}),
        json!({"agentFunctionResponse":{"functionCallId":"call","value":null}}),
    ];
    for mut value in operations {
        value["version"] = json!("1.0");
        let message: AgentToRendererMessage = serde_json::from_value(value.clone())?;
        assert_eq!(serde_json::to_value(message)?, value);
    }
    let message: AgentToRendererMessage =
        serde_json::from_value(json!({"version":"1.0","deleteSurface":{"surfaceId":"contact"}}))?;
    let AgentToRendererMessage::V1(AgentToRenderer::DeleteSurface(payload)) = message else {
        return Err("wrong native operation variant".into());
    };
    assert_eq!(payload.surface_id, "contact");
    Ok(())
}

#[test]
fn rejects_invalid_agent_envelopes_and_presence() {
    let invalid = [
        json!({"createSurface":{"surfaceId":"s"}}),
        json!({"version":"0.9","deleteSurface":{"surfaceId":"s"}}),
        json!({"version":"1.1","deleteSurface":{"surfaceId":"s"}}),
        json!({"version":"1.0"}),
        json!({"version":"1.0","deleteSurface":{"surfaceId":"s"},"extra":true}),
        json!({"version":"1.0","deleteSurface":{"surfaceId":"s"},"createSurface":{"surfaceId":"s"}}),
        json!({"version":"1.0","createSurface":{"surfaceId":3}}),
        json!({"version":"1.0","createSurface":{"surfaceId":"s","theme":{}}}),
        json!({"version":"1.0","createSurface":{"surfaceId":"s","catalogId":null}}),
        json!({"version":"1.0","createSurface":{"surfaceId":"s","components":[]}}),
        json!({"version":"1.0","createSurface":{"surfaceId":"s","dataModel":[]}}),
        json!({"version":"1.0","updateComponents":{"surfaceId":"s","components":[]}}),
        json!({"version":"1.0","updateDataModel":{"surfaceId":"s"}}),
        json!({"version":"1.0","updateDataModel":{"surfaceId":"s","path":null,"value":1}}),
        json!({"version":"1.0","callRendererFunction":{"functionCallId":"call","callFunction":{"@call":"ping"}}}),
    ];
    for value in invalid {
        assert!(
            serde_json::from_value::<AgentToRendererMessage>(value.clone()).is_err(),
            "accepted {value}"
        );
    }
    for source in [
        r#"{"version":"1.0","version":"1.0","deleteSurface":{"surfaceId":"s"}}"#,
        r#"{"version":"1.0","deleteSurface":{"surfaceId":"s","surfaceId":"other"}}"#,
    ] {
        assert!(serde_json::from_str::<AgentToRendererMessage>(source).is_err());
    }
}

#[test]
fn round_trips_renderer_operations_and_extra_properties() -> TestResult {
    for value in [
        json!({"version":"1.0","action":{"name":"save","surfaceId":"s","sourceComponentId":"save","timestamp":"2026-11-23T12:00:00Z","context":{},"catalogId":"custom"}}),
        json!({"version":"1.0","callAgentFunction":{"surfaceId":"s","functionCallId":"call","callFunction":{"@call":"save","args":{}}}}),
        json!({"version":"1.0","rendererFunctionResponse":{"functionCallId":"call","error":{"code":"FAILED","message":"failure"}}}),
        json!({"version":"1.0","error":{"code":"EXPRESSION_ERROR","message":"failed","functionCallId":"call","detail":{"retry":false}}}),
    ] {
        let message: RendererToAgentMessage = serde_json::from_value(value.clone())?;
        assert_eq!(serde_json::to_value(message)?, value);
    }
    Ok(())
}

#[test]
fn runs_upstream_renderer_schema_fixtures() -> TestResult {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/upstream/specification/v1_0/test/cases");
    for file in ["renderer_messages.json", "function_response.json"] {
        let fixture: Value = serde_json::from_slice(&fs::read(root.join(file))?)?;
        let tests = fixture["tests"].as_array().ok_or("fixture without tests")?;
        assert!(!tests.is_empty(), "empty upstream fixture: {file}");
        for test in tests {
            let value = test["data"].clone();
            let result = serde_json::from_value::<RendererToAgentMessage>(value.clone());
            assert_eq!(
                result.is_ok(),
                test["valid"]
                    .as_bool()
                    .ok_or("fixture without expected validity")?,
                "{file}: {}: {result:?}",
                test["description"]
            );
            if let Ok(message) = result {
                assert_eq!(
                    serde_json::to_value(message)?,
                    value,
                    "{file}: {}",
                    test["description"]
                );
            }
        }
    }
    Ok(())
}

#[test]
fn round_trips_realistic_upstream_surfaces() -> TestResult {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/upstream/catalogs/basic/v1/examples");
    for file in ["contact-card.json", "incremental-dashboard.json"] {
        let fixture: Value = serde_json::from_slice(&fs::read(root.join(file))?)?;
        let messages = fixture["messages"]
            .as_array()
            .ok_or("fixture without messages")?;
        assert!(!messages.is_empty(), "empty upstream example: {file}");
        for value in messages {
            let message: AgentToRendererMessage = serde_json::from_value(value.clone())?;
            assert_eq!(serde_json::to_value(message)?, *value);
        }
    }
    Ok(())
}

#[test]
fn rejects_renderer_errors_with_invalid_targets_or_reserved_codes() {
    for error in [
        json!({"code":"FAILED","message":"failure"}),
        json!({"code":"FAILED","message":"failure","surfaceId":"s","functionCallId":"call"}),
        json!({"code":"FAILED","message":"failure","surfaceId":null}),
        json!({"code":"VALIDATION_FAILED","message":"failure","surfaceId":"s"}),
        json!({"code":"UNALLOWED_CHILD","message":"failure","functionCallId":"call"}),
        json!({"code":"VALIDATION_FAILED","message":"failure","surfaceId":"s","path":"/name","extra":true}),
    ] {
        assert!(
            serde_json::from_value::<RendererToAgentMessage>(
                json!({"version":"1.0","error":error})
            )
            .is_err()
        );
    }
    assert!(serde_json::from_str::<RendererToAgentMessage>(r#"{"version":"1.0","error":{"code":"FAILED","message":"failure","surfaceId":"s","surfaceId":"other"}}"#).is_err());
}

#[test]
fn rejects_metadata_nulls_and_reserved_surface_components() {
    for payload in [
        json!({"surfaceId":"s","metadata":null}),
        json!({"surfaceId":"s","metadata":{"extensions":null}}),
        json!({"surfaceId":"s","sendDataModel":null}),
        json!({"surfaceId":"s","components":[{"id":"root","component":"Surface"}]}),
        json!({"surfaceId":"s","components":[{"id":"root","component":"Text","@catalogId":null}]}),
    ] {
        assert!(
            serde_json::from_value::<AgentToRendererMessage>(
                json!({"version":"1.0","createSurface":payload})
            )
            .is_err()
        );
    }
}

#[test]
fn outgoing_additional_properties_cannot_override_typed_fields() -> TestResult {
    let AgentToRendererMessage::V1(AgentToRenderer::UpdateComponents(mut update)) =
        serde_json::from_value(
            json!({"version":"1.0","updateComponents":{"surfaceId":"s","components":[{"id":"root","component":"Text"}]}}),
        )?
    else {
        return Err("wrong operation variant".into());
    };
    let mut component = update.components.as_slice()[0].clone();
    component
        .properties
        .insert("id".to_owned(), json!("override"));
    assert!(serde_json::to_value(&component).is_err());
    component.properties.clear();
    component.component = "Surface".to_owned();
    assert!(serde_json::to_value(&component).is_err());
    update.components = vec![component].try_into()?;
    assert!(
        serde_json::to_value(AgentToRendererMessage::V1(
            AgentToRenderer::UpdateComponents(update)
        ))
        .is_err()
    );
    Ok(())
}

#[test]
fn typed_batches_preserve_order_and_reject_extra_fields() -> TestResult {
    let value = json!({"messages":[{"version":"1.0","deleteSurface":{"surfaceId":"first"}},{"version":"1.0","deleteSurface":{"surfaceId":"second"}}]});
    let batch: MessageBatch<AgentToRendererMessage> = serde_json::from_value(value.clone())?;
    assert_eq!(serde_json::to_value(batch)?, value);
    assert!(
        serde_json::from_value::<MessageBatch<AgentToRendererMessage>>(
            json!({"messages":[],"extra":true})
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<MessageBatch<AgentToRendererMessage>>(json!({"messages":null}))
            .is_err()
    );
    let empty: MessageBatch<AgentToRendererMessage> =
        serde_json::from_value(json!({"messages":[]}))?;
    assert!(empty.messages.is_empty());
    Ok(())
}

#[test]
fn outgoing_renderer_properties_cannot_change_the_error_or_action_shape() -> TestResult {
    let mut error: GenericError =
        serde_json::from_value(json!({"code":"FAILED","message":"failure","surfaceId":"s"}))?;
    error.code = "VALIDATION_FAILED".to_owned();
    assert!(serde_json::to_value(&error).is_err());
    error.code = "FAILED".to_owned();
    error
        .extra
        .insert("functionCallId".to_owned(), json!("call"));
    assert!(serde_json::to_value(&error).is_err());

    let mut action: Action = serde_json::from_value(
        json!({"name":"save","surfaceId":"s","sourceComponentId":"save","timestamp":"2026-11-23T12:00:00Z","context":{}}),
    )?;
    action.extra.insert("name".to_owned(), json!("override"));
    assert!(serde_json::to_value(&action).is_err());
    Ok(())
}
