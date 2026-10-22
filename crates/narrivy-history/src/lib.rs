//! History, Privacy Zones, and Dictionary — P8.
//!
//! This crate owns:
//! - SQLite per-user history with TTL (7d/30d/forever) respecting PRIV-002 off
//! - Privacy Zones (app-category deny) per PRIV-003
//! - Dictionary (Hunspell/SymSpell local, no cloud) per FUN-041
//! - Context provider isolated module (Privacy Zones override, PRIV-007 split)

#![forbid(unsafe_code)]

pub mod context;
pub mod context_adapter;
pub mod dict_test_helpers;
pub mod dictionary;
pub mod history;
pub mod privacy;
pub mod privacy_zones;

pub use context::{ContextField, ContextProvider, ContextSnapshot, NoopContextProvider};
pub use context_adapter::{ContextGateDecision, NoopContext, gate_context, maybe_collect_context};
pub use dictionary::{Dictionary, DictionaryEntry};
pub use history::{HistoryEntry, HistoryStore, RetentionPolicy};
pub use privacy_zones::{PrivacyZone, PrivacyZoneAction, ZoneMatcher};

#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    #[error("storage error: {0}")]
    Storage(String),
    #[error("policy error: {0}")]
    Policy(String),
}
