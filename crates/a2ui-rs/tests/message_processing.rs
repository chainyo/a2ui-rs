//! Tests JSON batch processing and renderer data model reports.

use std::error::Error;

use a2ui_rs::validation::{MessageProcessor, ProcessError};
use serde_json::json;

#[test]
fn a_json_batch_decodes_before_applying_and_stops_at_the_first_state_error() {
    let mut processor = MessageProcessor::default();
    let create = json!({"version": "1.0", "createSurface": {"surfaceId": "schedule"}});
    let later = json!({"version": "1.0", "createSurface": {"surfaceId": "later"}});
    let malformed = json!({"createSurface": {"surfaceId": "bad"}});
    let result = processor.apply_json(&json!([create, malformed, later]));
    assert!(matches!(
        result,
        Err(ProcessError::Envelope { index: 1, .. })
    ));
    assert!(processor.get("schedule").is_none());

    let result = processor.apply_json(&json!({"messages": [create, create, later]}));
    assert!(matches!(
        result,
        Err(ProcessError::Validation { index: 1, .. })
    ));
    assert!(processor.get("schedule").is_some());
    assert!(processor.get("later").is_none());
}

#[test]
fn renderer_data_model_tracks_active_surfaces_that_request_reporting() -> Result<(), Box<dyn Error>>
{
    let mut processor = MessageProcessor::default();
    assert_eq!(processor.renderer_data_model(), None);
    processor.apply_json(&json!([
        {"version": "1.0", "createSurface": {
            "surfaceId": "schedule", "sendDataModel": true, "dataModel": {"day": "Monday"}
        }},
        {"version": "1.0", "createSurface": {
            "surfaceId": "preview", "dataModel": {"day": "Tuesday"}
        }},
        {"version": "1.0", "updateDataModel": {
            "surfaceId": "schedule", "path": "/day", "value": "Wednesday"
        }}
    ]))?;
    assert_eq!(
        serde_json::to_value(processor.renderer_data_model())?,
        json!({"version":"1.0", "surfaces":{"schedule":{"day":"Wednesday"}}})
    );
    processor.apply_json(&json!({"version": "1.0", "deleteSurface": {"surfaceId": "schedule"}}))?;
    assert_eq!(processor.renderer_data_model(), None);
    Ok(())
}
