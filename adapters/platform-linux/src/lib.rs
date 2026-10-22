//! Linux platform adapter: daemon lifecycle primitives (ADR-0012).
//!
//! Unsafe policy: see crate-level docs. Each FFI call is wrapped, minimal,
//! and carries a SAFETY comment; no other unsafe exists in this crate.
pub mod delivery;
/// Desktop-managed keyboard shortcuts (the desktop's own custom-shortcut store).
pub mod desktop_shortcuts;
/// Fixed-root installed runtime-manifest observation. This establishes only
/// manifest-to-worker byte consistency, never package or release provenance.
pub mod installed_runtime;
pub mod instance;
/// Linux-only, descriptor-anchored offline model import transaction.
pub mod model_import;

/// How busy the GPU and NPU are, from their drivers' sysfs counters.
pub mod accelerator_usage;
/// Linux host facts for the portable compute-planning contract: accelerator
/// device nodes, bounded discovery helpers, and the logical core count.
pub mod compute_probe;
/// POSIX owner-only protection for the portable model-cache contract.
pub mod model_cache_policy;

/// Audio capture primitives (salvage landing, P4-hardened).
pub mod capture;
/// Echo cancellation: the microphone without the computer's own sound.
pub mod echo_cancel;
/// Press and hold: one chosen key read from the keyboards (opt-in).
pub mod hold_key;
/// Recovery Phase 2A real PipeWire capture and Inbox-only delivery ports.
pub mod recovery;
pub mod wayland_selection;

/// P4 audio policy engine (default-sink duck/restore via wpctl).
pub mod audio_policy;

/// P4 live PipeWire capture monitor (pw-record -> ring buffer).
pub mod pipewire_monitor;

/// Global trigger sources (P2 walking skeleton).
pub mod trigger;

/// Modular audio pipeline components (P2-P4 latency/jitter).
pub mod audio;

/// Linux context provider (P8, Privacy Zone + PolicyEngine gated).
pub mod context;

/// Read-only RemoteDesktop/EIS eligibility plus the owned permission and text
/// sender primitives for the permission-mediated Linux candidate. Daemon
/// startup composes the permission lifecycle; delivery stays unqualified.
pub mod portal_eis;
mod portal_eis_sender;
pub mod portal_eis_zbus;

/// Secrets in the desktop's keyring (Secret Service), for the assistant's
/// web search key (D35).
pub mod secrets;
