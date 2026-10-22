//! Typed text-delivery router for Linux (P2 prep).
//!
//! Salvage-adapted from the v0.4 platform adapter (see `delivery/README.md`)
//! and re-bound to v1.3 contracts: portable typed outcomes
//! ([`narrivy_types::DeliveryOutcome`]), bounded invocations, least-privilege
//! defaults, and an honest fallback ladder:
//!
//! `wtype` direct insertion -> uinput (opt-in only) -> clipboard copy-only
//! (marked [`DeliveryOutcome::CopiedOnly`], never `Delivered`) -> Inbox
//! atomic write as the final durable sink.
//!
//! # Privacy rule
//! Ordinary logs and `Debug` output must never contain transcript or user
//! text. Every external-tool invocation is built as an explicit argv array
//! ([`command::CommandSpec`]) whose `Debug` output redacts arguments, and no
//! function in this module prints or logs payload content; diagnostics carry
//! short stable reason codes only.
//!
//! # Bounded execution
//! External processes run through [`command::CommandRunner`]: explicit argv
//! (never a shell), stdin pumped by the child-owning thread then closed, and
//! a bounded kill-and-reap wait under one absolute deadline.
//!
//! P2 will wire this router into the daemon pipeline (journal-before-delivery,
//! target-aware formatting); P1 only requires it compilable and tested
//! offline, which the fake-backend unit tests guarantee.

pub mod backend;
pub mod clipboard_paste;
pub mod command;
pub mod inbox;
pub mod probe;
pub mod routed;
pub mod router;

pub use backend::{
    AttemptOutcome, BackendAvailability, ClipboardBackend, ClipboardTool, DeliveryBackend,
    DeliveryBackendId, DeliveryCheck, DeliveryRequest, MAX_DIRECT_TEXT_BYTES, UinputBackend,
    WlClipboardOrXclip, WtypeBackend,
};
pub use clipboard_paste::{WaylandClipboardHandoff, WtypePasteChord};
pub use inbox::{InboxError, InboxStore, validate_txid};
pub use probe::{generate_txid, probe_tool, probe_wtype, probe_wtype_at};
pub use routed::{
    ClipboardCopyRung, ClipboardHandoff, ClipboardOwnership, ClipboardPasteRung,
    ClipboardRestoreOutcome, DEFAULT_REPLAY_CAPACITY, DeliveryPermissionEvidence,
    DeliveryPermissionPort, DeliveryReplayCache, DirectRung, INBOX_BACKEND,
    PRE_ACTIVATION_BINDING_ABSENT, PasteChord, PortalEisTextRung, ROUTED_THEN_INBOX_BACKEND,
    ReplayCheck, RoutedThenInboxDelivery, RungOutcome, RungQualification, SystemWtypeKeyboardRung,
    TargetBinding, TargetBindingSource, UnavailablePreActivationTargetBinding, UnavailableRung,
    WtypeKeyboardRung, wayland_text_input_supported,
};
pub use router::{
    DeliveryRouter, DeliveryRouterConfig, route_and_deliver, route_and_deliver_tx_with,
    route_and_deliver_with, route_over_backends, route_with_attempts,
};

#[cfg(test)]
mod tests;
