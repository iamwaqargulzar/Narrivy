//! Delivery backend identities, availability contract, and the concrete
//! rungs of the P2-prep ladder (salvage-adapted; see `delivery/README.md`).
//!
//! Honesty rules carried over from v0.4 and made typed:
//! - only backends that can VERIFY insertion report
//!   [`DeliveryOutcome::Delivered`];
//! - clipboard copies report [`DeliveryOutcome::CopiedOnly`] — never upgraded;
//! - the Inbox reports [`DeliveryOutcome::InboxFallback`] on success.

use std::path::{Path, PathBuf};
use std::time::Duration;

use narrivy_types::{DeliveryAttempt, DeliveryOutcome, FallbackReason, TransactionId};

use super::command::{CommandRunner, CommandSpec, RunError};
use super::inbox::{InboxError, InboxStore};
use super::probe::{PROBE_TIMEOUT, now_unix_ms, probe_tool};

/// Hard cap for direct typing/copy attempts (v0.4 salvage: the upstream
/// 64 KiB message bound). Larger payloads are routed straight to the Inbox
/// with `FallbackReason::PolicyDenied`.
pub const MAX_DIRECT_TEXT_BYTES: usize = 64 * 1024;

/// Bound for one direct insertion attempt.
const DIRECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Bound for one clipboard copy attempt (copy tools daemonize after reading).
const CLIPBOARD_TIMEOUT: Duration = Duration::from_secs(3);

/// Which clipboard copy tool a backend instance uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WlClipboardOrXclip {
    WlCopy,
    Xclip,
}

/// Typed identity of one ladder rung (content-free).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryBackendId {
    /// Compositor-mediated insertion via the `wtype` zwp_virtual_keyboard helper.
    WtypeVirtualKeyboard,
    /// Clipboard copy-only honesty: never claims full delivery.
    ClipboardCopy(WlClipboardOrXclip),
    /// Broad synthetic input device; opt-in only, least-privilege default off.
    UinputOptIn,
    /// Durable Inbox sink; final rung, outcome `InboxFallback` on success.
    Inbox,
}

impl DeliveryBackendId {
    /// Stable content-free label used in portable [`DeliveryAttempt`] records.
    pub fn label(&self) -> &'static str {
        match self {
            Self::WtypeVirtualKeyboard => "wtype-virtual-keyboard",
            Self::ClipboardCopy(WlClipboardOrXclip::WlCopy) => "clipboard-copy:wl-copy",
            Self::ClipboardCopy(WlClipboardOrXclip::Xclip) => "clipboard-copy:xclip",
            Self::UinputOptIn => "uinput-opt-in",
            Self::Inbox => "inbox",
        }
    }
}

/// Result of a presence probe. The reason is a short stable code, not a
/// diagnostics dump (privacy: no environment dumps in typed records).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendAvailability {
    ProbeAvailable,
    ProbeMissing(String),
}

/// Availability contract shared by all rungs. Real backends probe real system
/// state ([`probe_wtype`](super::probe::probe_wtype) and friends); tests
/// substitute fakes.
pub trait DeliveryCheck {
    fn availability(&self) -> BackendAvailability;
}

/// One routed request: verbatim payload plus the transaction id naming its
/// Inbox artifact.
#[derive(Debug, Clone, Copy)]
pub struct DeliveryRequest<'a> {
    pub text: &'a str,
    pub txid: &'a TransactionId,
}

/// Record of one bounded attempt; convertible to the portable wire/storage
/// record via [`AttemptOutcome::to_attempt`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptOutcome {
    pub backend: DeliveryBackendId,
    pub outcome: DeliveryOutcome,
    pub fallback_reason: Option<FallbackReason>,
    pub at_unix_ms: u64,
}

impl AttemptOutcome {
    pub fn new(
        backend: DeliveryBackendId,
        outcome: DeliveryOutcome,
        fallback_reason: Option<FallbackReason>,
    ) -> Self {
        Self {
            backend,
            outcome,
            fallback_reason,
            at_unix_ms: now_unix_ms(),
        }
    }

    /// Rung probed missing: recorded honestly, then the ladder moves on.
    pub fn unavailable(backend: DeliveryBackendId) -> Self {
        Self::new(
            backend,
            DeliveryOutcome::Failed,
            Some(FallbackReason::NoQualifiedBackend),
        )
    }

    /// Payload refused by policy (oversize guard), not by a backend error.
    pub fn policy_denied(backend: DeliveryBackendId) -> Self {
        Self::new(
            backend,
            DeliveryOutcome::Failed,
            Some(FallbackReason::PolicyDenied),
        )
    }

    pub fn timeout(backend: DeliveryBackendId) -> Self {
        Self::new(
            backend,
            DeliveryOutcome::Failed,
            Some(FallbackReason::Timeout),
        )
    }

    pub fn backend_error(backend: DeliveryBackendId, code: &str) -> Self {
        Self::new(
            backend,
            DeliveryOutcome::Failed,
            Some(FallbackReason::BackendError {
                code: code.to_string(),
            }),
        )
    }

    /// Portable record for IPC / recovery-journal persistence.
    pub fn to_attempt(&self) -> DeliveryAttempt {
        DeliveryAttempt {
            backend: self.backend.label().to_string(),
            outcome: self.outcome.clone(),
            fallback_reason: self.fallback_reason.clone(),
            at_unix_ms: self.at_unix_ms,
        }
    }
}

/// One rung of the ladder. Implementations must stay bounded, panic-free, and
/// must never print or log `req.text`.
pub trait DeliveryBackend: DeliveryCheck {
    fn id(&self) -> DeliveryBackendId;

    /// Attempt delivery of `req`. See module honesty rules for which
    /// [`DeliveryOutcome`] variants each kind of backend may report.
    fn deliver(&mut self, req: &DeliveryRequest<'_>) -> AttemptOutcome;
}

fn exit_code_label(code: Option<i32>) -> String {
    code.map(|c| c.to_string())
        .unwrap_or_else(|| "signal".to_string())
}

// ---------------------------------------------------------------------------
// wtype direct insertion
// ---------------------------------------------------------------------------

/// Direct insertion via the `wtype` compositor helper (zwp_virtual_keyboard-v1).
///
/// The payload is always supplied by stdin via `wtype -`, never argv. A zero
/// exit means only that the helper accepted the submission, not that a focused
/// application retained it, so it reports `SubmittedUnverified`.
pub struct WtypeBackend<'r> {
    program: PathBuf,
    runner: &'r dyn CommandRunner,
}

impl<'r> WtypeBackend<'r> {
    /// `program=None` resolves `wtype` from PATH; tests pass explicit paths.
    pub fn new(program: Option<PathBuf>, runner: &'r dyn CommandRunner) -> Self {
        Self {
            program: program.unwrap_or_else(|| PathBuf::from("wtype")),
            runner,
        }
    }

    /// Explicit argv for one insertion attempt: `[wtype, -]`.
    pub fn insertion_spec(&self) -> CommandSpec {
        let mut spec = CommandSpec::new(&self.program);
        spec.arg("-");
        spec
    }
}

impl DeliveryCheck for WtypeBackend<'_> {
    fn availability(&self) -> BackendAvailability {
        probe_tool(&self.program, &["--version"], self.runner, PROBE_TIMEOUT)
    }
}

impl DeliveryBackend for WtypeBackend<'_> {
    fn id(&self) -> DeliveryBackendId {
        DeliveryBackendId::WtypeVirtualKeyboard
    }

    fn deliver(&mut self, req: &DeliveryRequest<'_>) -> AttemptOutcome {
        if req.text.len() > MAX_DIRECT_TEXT_BYTES {
            return AttemptOutcome::policy_denied(self.id());
        }
        let spec = self.insertion_spec();
        match self
            .runner
            .run(&spec, Some(req.text.as_bytes()), DIRECT_TIMEOUT)
        {
            Ok(out) if out.success => {
                AttemptOutcome::new(self.id(), DeliveryOutcome::SubmittedUnverified, None)
            }
            Ok(out) => AttemptOutcome::backend_error(
                self.id(),
                &format!("WTYPE_EXIT_{}", exit_code_label(out.code)),
            ),
            Err(RunError::Timeout | RunError::TerminationTimeout) => {
                AttemptOutcome::timeout(self.id())
            }
            Err(RunError::Spawn(_)) => {
                AttemptOutcome::backend_error(self.id(), "WTYPE_SPAWN_FAILED")
            }
            Err(RunError::StdinWrite(_) | RunError::Wait(_)) => {
                AttemptOutcome::backend_error(self.id(), "WTYPE_RUN_FAILED")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// uinput opt-in
// ---------------------------------------------------------------------------

/// Broad synthetic input via `/dev/uinput`. Least-privilege rule: the router
/// only includes this rung when the operator explicitly opted in
/// (`DeliveryRouterConfig::allow_uinput_opt_in`, default **false**).
///
/// P2 prep status (mirrors the v0.4 salvage decision): the opt-in gate,
/// device-presence probe, and typed outcomes are wired; actual key-event
/// synthesis is NOT qualified yet and fails honestly with
/// `UINPUT_NOT_QUALIFIED`. P2 qualifies it or drops the rung.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UinputBackend {
    pub allow_opt_in: bool,
    pub device: PathBuf,
}

impl Default for UinputBackend {
    fn default() -> Self {
        Self {
            allow_opt_in: false,
            device: PathBuf::from("/dev/uinput"),
        }
    }
}

impl UinputBackend {
    pub fn new(allow_opt_in: bool) -> Self {
        Self {
            allow_opt_in,
            ..Default::default()
        }
    }
}

impl DeliveryCheck for UinputBackend {
    fn availability(&self) -> BackendAvailability {
        if !self.allow_opt_in {
            return BackendAvailability::ProbeMissing("opt-in-disabled-by-policy".into());
        }
        // `Path::exists` cannot panic; a hostile device node path stays a probe miss.
        if !Path::new(&self.device).exists() {
            return BackendAvailability::ProbeMissing("dev-uinput-missing".into());
        }
        BackendAvailability::ProbeAvailable
    }
}

impl DeliveryBackend for UinputBackend {
    fn id(&self) -> DeliveryBackendId {
        DeliveryBackendId::UinputOptIn
    }

    fn deliver(&mut self, _req: &DeliveryRequest<'_>) -> AttemptOutcome {
        AttemptOutcome::backend_error(self.id(), "UINPUT_NOT_QUALIFIED")
    }
}

// ---------------------------------------------------------------------------
// clipboard copy-only
// ---------------------------------------------------------------------------

/// A configured clipboard copy tool: kind plus program to invoke.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardTool {
    pub kind: WlClipboardOrXclip,
    pub program: PathBuf,
}

impl ClipboardTool {
    pub fn wl_copy_default() -> Self {
        Self {
            kind: WlClipboardOrXclip::WlCopy,
            program: PathBuf::from("wl-copy"),
        }
    }

    pub fn xclip_default() -> Self {
        Self {
            kind: WlClipboardOrXclip::Xclip,
            program: PathBuf::from("xclip"),
        }
    }

    /// Version-style presence-probe flags per tool family.
    fn probe_args(&self) -> &'static [&'static str] {
        match self.kind {
            WlClipboardOrXclip::WlCopy => &["--version"],
            WlClipboardOrXclip::Xclip => &["-version"],
        }
    }

    /// Explicit argv prefix for a copy invocation; the payload goes to stdin,
    /// never into argv.
    fn copy_args(&self) -> &'static [&'static str] {
        match self.kind {
            WlClipboardOrXclip::WlCopy => &[],
            WlClipboardOrXclip::Xclip => &["-selection", "clipboard"],
        }
    }

    pub fn copy_spec(&self) -> CommandSpec {
        let mut spec = CommandSpec::new(&self.program);
        for a in self.copy_args() {
            spec.arg(*a);
        }
        spec
    }
}

/// Clipboard COPY-ONLY rung (v0.4 hard-won honesty): a successful copy is
/// reported as [`DeliveryOutcome::CopiedOnly`] and must never be upgraded to
/// `Delivered` — insertion was not proven. With `tool=None` the v0.4 auto
/// order is kept: wl-copy first, xclip (XWayland) second.
pub struct ClipboardBackend<'r> {
    tools: Vec<ClipboardTool>,
    runner: &'r dyn CommandRunner,
}

impl<'r> ClipboardBackend<'r> {
    pub fn new(tool: Option<ClipboardTool>, runner: &'r dyn CommandRunner) -> Self {
        let tools = match tool {
            Some(t) => vec![t],
            None => vec![
                ClipboardTool::wl_copy_default(),
                ClipboardTool::xclip_default(),
            ],
        };
        Self { tools, runner }
    }

    fn primary_kind(&self) -> WlClipboardOrXclip {
        self.tools
            .first()
            .map(|t| t.kind)
            .unwrap_or(WlClipboardOrXclip::Xclip)
    }
}

impl DeliveryCheck for ClipboardBackend<'_> {
    fn availability(&self) -> BackendAvailability {
        // Available when ANY configured tool probes present.
        let mut last = BackendAvailability::ProbeMissing("no-clipboard-tool-configured".into());
        for tool in &self.tools {
            match probe_tool(&tool.program, tool.probe_args(), self.runner, PROBE_TIMEOUT) {
                found @ BackendAvailability::ProbeAvailable => return found,
                missing => last = missing,
            }
        }
        last
    }
}

impl DeliveryBackend for ClipboardBackend<'_> {
    fn id(&self) -> DeliveryBackendId {
        DeliveryBackendId::ClipboardCopy(self.primary_kind())
    }

    fn deliver(&mut self, req: &DeliveryRequest<'_>) -> AttemptOutcome {
        if req.text.len() > MAX_DIRECT_TEXT_BYTES {
            return AttemptOutcome::policy_denied(self.id());
        }
        let mut last: Option<AttemptOutcome> = None;
        for tool in &self.tools {
            let rung = DeliveryBackendId::ClipboardCopy(tool.kind);
            let spec = tool.copy_spec();
            match self
                .runner
                .run(&spec, Some(req.text.as_bytes()), CLIPBOARD_TIMEOUT)
            {
                Ok(out) if out.success => {
                    return AttemptOutcome::new(rung, DeliveryOutcome::CopiedOnly, None);
                }
                Ok(out) => {
                    last = Some(AttemptOutcome::backend_error(
                        rung,
                        &format!("CLIPBOARD_EXIT_{}", exit_code_label(out.code)),
                    ));
                }
                Err(RunError::Timeout | RunError::TerminationTimeout) => {
                    last = Some(AttemptOutcome::timeout(rung));
                }
                Err(RunError::Spawn(_)) => {
                    last = Some(AttemptOutcome::backend_error(
                        rung,
                        "CLIPBOARD_SPAWN_FAILED",
                    ));
                }
                Err(RunError::StdinWrite(_)) => {
                    last = Some(AttemptOutcome::backend_error(
                        rung,
                        "CLIPBOARD_STDIN_FAILED",
                    ));
                }
                Err(RunError::Wait(_)) => {
                    last = Some(AttemptOutcome::backend_error(rung, "CLIPBOARD_WAIT_FAILED"));
                }
            }
        }
        last.unwrap_or_else(|| AttemptOutcome::backend_error(self.id(), "CLIPBOARD_UNAVAILABLE"))
    }
}

// ---------------------------------------------------------------------------
// inbox sink
// ---------------------------------------------------------------------------

/// Final rung: durable Inbox write. Success is reported as
/// [`DeliveryOutcome::InboxFallback`] (never `Delivered`): the text is safely
/// stored, not inserted.
pub struct InboxBackend {
    store: Option<InboxStore>,
}

impl InboxBackend {
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self {
            store: dir.map(InboxStore::new),
        }
    }
}

impl DeliveryCheck for InboxBackend {
    fn availability(&self) -> BackendAvailability {
        match &self.store {
            Some(_) => BackendAvailability::ProbeAvailable,
            None => BackendAvailability::ProbeMissing("no-inbox-dir-configured".into()),
        }
    }
}

impl DeliveryBackend for InboxBackend {
    fn id(&self) -> DeliveryBackendId {
        DeliveryBackendId::Inbox
    }

    fn deliver(&mut self, req: &DeliveryRequest<'_>) -> AttemptOutcome {
        let Some(store) = &self.store else {
            return AttemptOutcome::backend_error(self.id(), "INBOX_UNAVAILABLE");
        };
        match store.store_text(req.txid, req.text) {
            Ok(_) => AttemptOutcome::new(self.id(), DeliveryOutcome::InboxFallback, None),
            Err(InboxError::InvalidTxid) => {
                AttemptOutcome::backend_error(self.id(), "INBOX_INVALID_TXID")
            }
            Err(InboxError::Io(_)) => AttemptOutcome::backend_error(self.id(), "INBOX_IO_FAILED"),
        }
    }
}
