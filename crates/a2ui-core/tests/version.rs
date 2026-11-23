//! Tests the public version boundary with realistic upstream messages.

use std::error::Error;
use std::fs;
use std::path::Path;
use std::str::FromStr;

use a2ui_core::ProtocolVersion;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct VersionHeader {
    version: ProtocolVersion,
}

#[test]
fn accepts_v1_in_a_realistic_surface_lifecycle() -> Result<(), Box<dyn Error>> {
    let messages: Vec<Value> =
        serde_json::from_str(include_str!("../../../fixtures/local/contact-surface.json"))?;
    assert_eq!(messages.len(), 4);
    for message in messages {
        let header: VersionHeader = serde_json::from_value(message)?;
        assert_eq!(header.version, ProtocolVersion::V1_0);
    }
    Ok(())
}

#[test]
fn rejects_legacy_future_and_malformed_versions() {
    for version in ["0.8", "0.9", "0.9.1", "1.1", "2.0", "v1.0", "1", ""] {
        assert!(ProtocolVersion::from_str(version).is_err(), "{version}");
        assert!(serde_json::from_value::<ProtocolVersion>(Value::from(version)).is_err());
    }
    for value in [Value::Null, Value::Bool(true), Value::from(1)] {
        assert!(serde_json::from_value::<ProtocolVersion>(value).is_err());
    }
    assert!(serde_json::from_str::<VersionHeader>("{}").is_err());
}

#[test]
fn serializes_the_canonical_version() -> Result<(), Box<dyn Error>> {
    let version = ProtocolVersion::from_str("1.0")?;
    assert_eq!(version.to_string(), "1.0");
    assert_eq!(serde_json::to_string(&version)?, "\"1.0\"");
    assert_eq!(
        ProtocolVersion::from_str("0.9")
            .err()
            .map(|error| error.to_string()),
        Some("unsupported A2UI protocol version: 0.9".to_owned())
    );
    Ok(())
}

#[test]
fn accepts_versions_in_upstream_contact_and_dashboard_examples() -> Result<(), Box<dyn Error>> {
    let examples = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/upstream/catalogs/basic/v1/examples");
    for name in ["contact-card.json", "incremental-dashboard.json"] {
        let source = fs::read_to_string(examples.join(name))?;
        let example: Value = serde_json::from_str(&source)?;
        let messages = example
            .get("messages")
            .and_then(Value::as_array)
            .ok_or("missing messages")?;
        assert!(!messages.is_empty());
        for message in messages {
            let header: VersionHeader = serde_json::from_value(message.clone())?;
            assert_eq!(header.version, ProtocolVersion::V1_0);
        }
    }
    Ok(())
}
