use std::error::Error;
use std::mem::discriminant;

use serde_json::{Value, json};

use super::{Catalog, MessageProcessor, ValidationConfig, ValidationError};
use crate::protocol::{
    AgentToRendererMessage, RendererError, RendererToAgent, RendererToAgentMessage, ValidationCode,
};
use crate::schema::common_types_schema;

type TestError = Box<dyn Error>;

const SURFACE: &str = "warehouse";
const DEFAULT_CATALOG: &str = "example:warehouse";
const OVERRIDE_CATALOG: &str = "example:cold-storage";
const COMPONENTS: [&str; 7] = [
    "root",
    "receiving",
    "label",
    "summary",
    "pallet",
    "shelf",
    "staging",
];

#[test]
fn seeded_strict_updates_match_full_surface_validation() -> Result<(), TestError> {
    run_sequences(ValidationConfig::default())
}

#[test]
fn seeded_streaming_updates_match_full_surface_validation() -> Result<(), TestError> {
    run_sequences(ValidationConfig::RELAXED)
}

fn run_sequences(config: ValidationConfig) -> Result<(), TestError> {
    let common = common_types_schema();
    let catalogs = catalog_documents();
    let mut accepted = 0;
    let mut rejected = 0;
    for seed in 0..16 {
        let mut processors = Processors::new(config, &catalogs, &common)?;
        check_known_updates(&mut processors, config)?;
        let mut generator = Batches(seed);
        for step in 0..32 {
            let batch = generator.next_batch();
            let outcome =
                processors.compare(&batch, &format!("seed {seed}, step {step}, {config:?}"))?;
            if outcome.is_none() {
                accepted += 1;
            } else {
                rejected += 1;
            }
        }
    }
    assert!(accepted > 0 && rejected > 0, "{config:?}");
    Ok(())
}

fn catalog_documents() -> [Value; 2] {
    let panel = json!({
        "type": "object",
        "allowedParents": ["Column"],
        "allowedChildren": ["Column", "Text"],
        "properties": {
            "component": {"const": "Panel"},
            "children": {"$ref": "common_types.json#/$defs/ChildList"}
        },
        "required": ["component", "children"]
    });
    let mut column = panel.clone();
    column["properties"]["component"]["const"] = json!("Column");
    column["allowedParents"] = json!(["Surface", "Column", "Panel"]);
    column["allowedChildren"] = json!(["Column", "Panel", "Text"]);

    let warehouse = json!({
        "catalogId": DEFAULT_CATALOG,
        "protocolVersion": "1.0",
        "components": {
            "Column": column,
            "Panel": panel,
            "Text": {
                "type": "object",
                "allowedParents": ["Column", "Panel"],
                "properties": {
                    "component": {"const": "Text"},
                    "text": {"$ref": "common_types.json#/$defs/DynamicString"}
                },
                "required": ["component", "text"]
            }
        }
    });
    let mut cold_storage = warehouse.clone();
    cold_storage["catalogId"] = json!(OVERRIDE_CATALOG);
    cold_storage["components"]["Text"]["allowedParents"] = json!(["Panel"]);
    cold_storage["components"]["Panel"]["allowedChildren"] = json!(["Column"]);
    [warehouse, cold_storage]
}

struct Processors {
    incremental: MessageProcessor,
    full: MessageProcessor,
}

impl Processors {
    fn new(
        config: ValidationConfig,
        catalogs: &[Value],
        common: &Value,
    ) -> Result<Self, TestError> {
        let create: AgentToRendererMessage = serde_json::from_value(json!({
            "version": "1.0",
            "createSurface": {
                "surfaceId": SURFACE,
                "catalogId": DEFAULT_CATALOG,
                "dataModel": {"shipment": "Warehouse transfer"}
            }
        }))?;
        let mut processors = Self {
            incremental: MessageProcessor::new(config),
            full: MessageProcessor::new(config),
        };
        for processor in [&mut processors.incremental, &mut processors.full] {
            for catalog in catalogs {
                processor.add_catalog(Catalog::from_json(catalog.clone(), common)?)?;
            }
            processor.apply(create.clone())?;
        }
        Ok(processors)
    }

    fn compare(
        &mut self,
        batch: &[Value],
        context: &str,
    ) -> Result<Option<ValidationError>, TestError> {
        let message: AgentToRendererMessage = serde_json::from_value(json!({
            "version": "1.0",
            "updateComponents": {"surfaceId": SURFACE, "components": batch}
        }))?;
        let before = self.incremental.get(SURFACE).cloned();
        self.full.topologies.clear();
        let expected = self.full.apply(message.clone()).err();
        let actual = self.incremental.apply(message).err();

        let outcome = |error: &ValidationError| {
            (
                discriminant(error),
                error.to_string(),
                error.renderer_error(SURFACE),
            )
        };
        assert_eq!(
            actual.as_ref().map(outcome),
            expected.as_ref().map(outcome),
            "{context}, batch {batch:?}"
        );
        let state = self.incremental.get(SURFACE);
        assert_eq!(state, self.full.get(SURFACE), "{context}, batch {batch:?}");
        if actual.is_some() {
            assert_eq!(state, before.as_ref(), "{context}, batch {batch:?}");
        }
        Ok(actual)
    }
}

fn check_known_updates(
    processors: &mut Processors,
    config: ValidationConfig,
) -> Result<(), TestError> {
    let initialize = [
        json!({"id": "root", "component": "Column", "children": ["receiving", "summary"]}),
        json!({"id": "receiving", "component": "Panel", "children": ["label"]}),
        json!({"id": "label", "component": "Text", "@catalogId": OVERRIDE_CATALOG, "text": "Pallet label"}),
        json!({"id": "summary", "component": "Text", "text": "Incoming shipment"}),
    ];
    assert!(
        processors
            .compare(&initialize, "initialize mixed catalog surface")?
            .is_none()
    );

    let patch = [
        json!({"id": "label", "component": "Text", "@catalogId": OVERRIDE_CATALOG, "text": "Label updated"}),
    ];
    assert!(
        processors
            .compare(&patch, "update the leaf under its existing parent")?
            .is_none()
    );

    let invalid_child = [
        json!({"id": "label", "component": "Text", "text": "Do not store on rejection"}),
        json!({"id": "summary", "component": "Text", "@catalogId": OVERRIDE_CATALOG, "text": "Cold storage only"}),
    ];
    check_field_error(
        processors.compare(&invalid_child, "reject the leaf catalog override")?,
        ValidationCode::UnallowedParent,
        "/components/1",
    )?;

    let invalid_parent = [
        json!({"id": "receiving", "component": "Panel", "@catalogId": OVERRIDE_CATALOG, "children": ["label"]}),
    ];
    check_field_error(
        processors.compare(&invalid_parent, "reject the parent catalog override")?,
        ValidationCode::UnallowedChild,
        "/components/0/children/0",
    )?;

    let parent = json!({"id": "receiving", "component": "Panel", "children": ["label", "pallet"]});
    let child = json!({"id": "pallet", "component": "Text", "@catalogId": OVERRIDE_CATALOG, "text": "Pallet received"});
    let missing =
        processors.compare(std::slice::from_ref(&parent), "refer to a later component")?;
    if config.allow_dangling_references {
        assert!(missing.is_none());
        assert!(
            processors
                .compare(&[child], "fill the existing reference")?
                .is_none()
        );
    } else {
        assert!(
            matches!(missing, Some(ValidationError::DanglingReference { child, .. }) if child == "pallet")
        );
        assert!(
            processors
                .compare(&[parent, child], "send the parent and child together")?
                .is_none()
        );
    }
    Ok(())
}

fn check_field_error(
    actual: Option<ValidationError>,
    code: ValidationCode,
    path: &str,
) -> Result<(), TestError> {
    let envelope = actual.and_then(|error| error.renderer_error(SURFACE));
    let Some(RendererToAgentMessage::V1(RendererToAgent::Error(RendererError::Validation(error)))) =
        envelope
    else {
        return Err(format!("expected a field error at {path}, got {envelope:?}").into());
    };
    assert_eq!(error.code, code);
    assert_eq!(error.path, path);
    assert_eq!(error.surface_id, SURFACE);
    Ok(())
}

struct Batches(u32);

impl Batches {
    fn pick(&mut self, upper: usize) -> usize {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        usize::from(self.0.to_be_bytes()[0]) % upper
    }

    fn next_batch(&mut self) -> Vec<Value> {
        (0..=self.pick(3)).map(|_| self.component()).collect()
    }

    fn component(&mut self) -> Value {
        let id = COMPONENTS[self.pick(COMPONENTS.len())];
        let kind = ["Text", "Column", "Panel"][self.pick(3)];
        let mut component = json!({"id": id, "component": kind});
        if self.pick(3) == 0 {
            component["@catalogId"] = json!(OVERRIDE_CATALOG);
        }
        if kind == "Text" {
            component["text"] = json!(format!("Shipment {}", self.pick(100)));
        } else {
            let children = (0..self.pick(4))
                .map(|_| COMPONENTS[self.pick(COMPONENTS.len())])
                .collect::<Vec<_>>();
            component["children"] = json!(children);
        }
        component
    }
}
