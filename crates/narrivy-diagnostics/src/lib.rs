//! Content-safe diagnostics primitives for Narrivy.
//!
//! This crate provides the building blocks for diagnostics that must never
//! leak user content:
//!
//! - [`Redactor`]: deny-list field redaction plus string scrubbing.
//! - [`HealthFact`] / [`HealthRegistry`]: latest-wins subsystem health with
//!   bounded history.
//! - [`LatencySample`] / [`LatencyTracker`]: bounded latency ring with
//!   deterministic percentiles.
//! - [`SupportReport`]: a user-shareable report built only from explicit
//!   caller-supplied inputs, with [`SupportReport::content_check`] as the
//!   pre-share content-safety verdict.
//!
//! Security invariants (see AGENTS.md): ordinary logs and diagnostics never
//! include transcript text, audio, clipboard/selection/context content,
//! tokens, secrets, or environment dumps. Environment data enters a
//! [`SupportReport`] only when the caller explicitly passes it.

mod error;
mod health;
mod latency;
mod redactor;
mod support;

pub use error::DiagnosticsError;
pub use health::{DEFAULT_HISTORY_CAP_PER_SUBSYSTEM, HealthFact, HealthRegistry};
pub use latency::{DEFAULT_RING_CAPACITY, LatencySample, LatencySummary, LatencyTracker};
pub use redactor::{DEFAULT_DENIED_KEYS, DEFAULT_MAX_STRING_LEN, Redactor};
pub use support::{ContentCheck, SUPPORT_REPORT_SCHEMA_VERSION, SupportReport};
