//! Context provider isolated — declares sensitivity, Privacy Zones override.
//! Skeleton for P8; enforces PRIV-007 split (transcript vs context) via PolicyEngine.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContextField {
    AppName,
    WindowTitle,
    SelectedText,
    SurroundingText,
    Clipboard,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextSnapshot {
    pub fields: Vec<(ContextField, String)>,
}

pub trait ContextProvider: Send + Sync {
    fn requested_fields(&self) -> Vec<ContextField>;
    fn sensitivity(&self) -> &'static str;
    fn snapshot(&self) -> ContextSnapshot;
}

/// No-op provider (skeleton, satisfies vacuously when broad providers deferred).
pub struct NoopContextProvider;

impl ContextProvider for NoopContextProvider {
    fn requested_fields(&self) -> Vec<ContextField> {
        Vec::new()
    }
    fn sensitivity(&self) -> &'static str {
        "none"
    }
    fn snapshot(&self) -> ContextSnapshot {
        ContextSnapshot { fields: Vec::new() }
    }
}
