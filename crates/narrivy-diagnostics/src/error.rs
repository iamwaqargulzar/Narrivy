//! Typed errors for diagnostics primitives.

use thiserror::Error;

/// Errors produced by `narrivy-diagnostics`.
#[derive(Debug, Error)]
pub enum DiagnosticsError {
    /// JSON (de)serialization failure while producing or parsing a report.
    #[error("json (de)serialization failed: {0}")]
    Json(#[from] serde_json::Error),
    /// A caller-supplied configuration value is invalid.
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
}
