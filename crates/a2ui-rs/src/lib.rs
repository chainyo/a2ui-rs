//! Framework-independent foundations for A2UI v1.0 and later.
//!
//! The [`protocol`] module contains v1.0 message envelopes and catalog-independent
//! payload types for serialization and deserialization.
//!
//! ```
//! use a2ui_rs::ProtocolVersion;
//!
//! let version: ProtocolVersion = "1.0".parse()?;
//! assert_eq!(version, ProtocolVersion::V1_0);
//! assert!("0.9".parse::<ProtocolVersion>().is_err());
//! # Ok::<(), a2ui_rs::UnsupportedVersion>(())
//! ```

//! A typed message serializes with the required protocol tag:
//!
//! ```
//! use a2ui_rs::protocol::{AgentToRenderer, AgentToRendererMessage, DeleteSurface};
//!
//! let message = AgentToRendererMessage::V1(AgentToRenderer::DeleteSurface(
//!     DeleteSurface { surface_id: "contact".to_owned() },
//! ));
//! let json = serde_json::to_string(&message)?;
//! let decoded: AgentToRendererMessage = serde_json::from_str(&json)?;
//! assert_eq!(decoded, message);
//! # Ok::<(), serde_json::Error>(())
//! ```

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A protocol version implemented by this SDK.
///
/// Legacy versions are outside the project scope. Future versions require an
/// explicit implementation before this type accepts them.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ProtocolVersion {
    /// A2UI protocol version 1.0.
    #[serde(rename = "1.0")]
    V1_0,
}

impl fmt::Display for ProtocolVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::V1_0 => formatter.write_str("1.0"),
        }
    }
}

impl FromStr for ProtocolVersion {
    type Err = UnsupportedVersion;

    fn from_str(version: &str) -> Result<Self, Self::Err> {
        match version {
            "1.0" => Ok(Self::V1_0),
            _ => Err(UnsupportedVersion {
                version: version.to_owned(),
            }),
        }
    }
}

/// An input names a protocol version that this SDK does not implement.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("unsupported A2UI protocol version: {version}")]
pub struct UnsupportedVersion {
    version: String,
}

pub mod catalog;
pub mod data_model;
pub mod expression;
pub mod protocol;
pub mod resolution;
pub mod routing;
pub mod rpc;
pub mod surface;
pub mod validation;
