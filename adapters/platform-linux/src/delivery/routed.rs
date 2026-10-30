//! Target-aware routed recovery delivery for the strict installed composition.
//!
//! The Phase 2A/2B lanes each compose exactly one destination (Inbox, or one
//! unverified `wtype` submission plus Inbox). This module generalizes that
//! contract into one *bounded approved ladder* over real, capability-gated
//! backends:
//!
//! 1. consume the immutable, content-free [`TargetBinding`] captured while the
//!    session was admitted; the adapter never queries the target at delivery
//!    time, so a later focus/activation change cannot redirect a rung;
//! 2. evaluate the current permission/authorization evidence;
//! 3. attempt only rungs whose qualification evidence is present, in priority
//!    order, and record why every other rung was skipped (including rungs that
//!    are deliberately reported unavailable);
//! 4. the first rung that can VERIFY its durable target result ends the ladder
//!    as [`DeliveryOutcome::Delivered`]; a rung whose helper accepted the
//!    whole payload without verification ends the ladder as
//!    [`DeliveryOutcome::SubmittedUnverified`] with no Inbox copy (F-M4), since
//!    a later rung would insert the text twice; every other outcome (clipboard
//!    only, a failure after dispatch, nothing qualified) is followed by exactly
//!    one atomic Inbox recovery write under the same transaction id;
//! 5. a bounded, transaction-keyed replay record binds every dispatched
//!    transaction to the SHA-256 digest and byte length of the exact payload,
//!    so a stale retry can never re-dispatch and a reused transaction id with
//!    different text is rejected before dispatch.
//!
//! No rung may claim `Delivered` from "the helper was spawned": only a rung
//! that observed a durable target result reports [`RungOutcome::Committed`].
//! The concrete Linux candidates in production remain unverified, so a
//! successful typed submission is reported as `SubmittedUnverified` and a
//! failed or uncertain one as Inbox recovery — exactly the shapes the
//! portable tail validates.
//!
//! # Privacy
//! Target identity, clipboard contents and transcript text are routing inputs
//! only. They are never logged, never rendered in `Debug`, never written to
//! ordinary diagnostics, and never leave the adapter. The replay record keeps
//! only a transaction id, a SHA-256 payload digest with its byte length, and
//! the content-free report; it never retains transcript plaintext.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use narrivy_platform_api::{DeliveryPort, PlatformError};
use narrivy_types::{
    DeliveryAttempt, DeliveryIntent, DeliveryOutcome, DeliveryReport, FallbackReason,
    SessionCancellation, TransactionId,
};
use sha2::{Digest, Sha256};

use crate::portal_eis::{PortalEisTextFailure, PortalEisTextPayload, PortalEisTextSubmission};
use crate::recovery::{
    PortalEisTextSubmitter, SystemWtypeSubmitter, WtypeSubmission, WtypeSubmitter,
    WtypeSubmitterStatus, wtype_submitter_status,
};

use super::backend::{
    AttemptOutcome, BackendAvailability, ClipboardBackend, ClipboardTool, DeliveryBackend,
    DeliveryCheck, DeliveryRequest, MAX_DIRECT_TEXT_BYTES,
};
use super::command::CommandRunner;
use super::inbox::{InboxError, InboxStore};
use super::probe::now_unix_ms;

/// Portable identity of the routed recovery report. This is a delivery-mode
/// identity, never a pipeline or composition-lane id.
pub const ROUTED_THEN_INBOX_BACKEND: &str = "routed-then-inbox";
/// Durable final sink rung identity.
pub const INBOX_BACKEND: &str = "inbox";

/// Bound for one portal/EIS text-frame submission.
const PORTAL_TEXT_BOUND: Duration = Duration::from_secs(2);

// ---------------------------------------------------------------------------
// Admission-time target binding
// ---------------------------------------------------------------------------

/// Stable reason code for a session whose admission produced no trustworthy
/// pre-activation target binding. This is the explicit typed-unavailable input
/// the current installed Start path supplies; it is never a captured target and
/// never derived from a desktop name or an activation reason string.
pub const PRE_ACTIVATION_BINDING_ABSENT: &str = "pre_activation_target_binding_absent";

/// Immutable, content-free target binding captured while a session was
/// admitted, BEFORE any capture/activation/focus side effect could run.
///
/// The application/window strings are routing inputs for compatibility and
/// stability checks only. They are deliberately absent from `Debug` output and
/// must never be logged or persisted by callers.
#[derive(Clone, PartialEq, Eq)]
pub enum TargetBinding {
    /// A real target was bound at admission, before any rung ran.
    Bound { application: String, window: String },
    /// No trustworthy pre-activation binding exists for this session.
    Unavailable(&'static str),
}

impl TargetBinding {
    #[must_use]
    pub fn is_bound(&self) -> bool {
        matches!(self, Self::Bound { .. })
    }

    /// Content-free class used for capability advertising and diagnostics.
    #[must_use]
    pub fn diagnostic_class(&self) -> &'static str {
        match self {
            Self::Bound { .. } => "bound",
            Self::Unavailable(_) => "unavailable",
        }
    }
}

impl std::fmt::Debug for TargetBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // PRIVACY: never render captured application/window identity.
        match self {
            Self::Bound { .. } => formatter.write_str("TargetBinding::Bound([redacted])"),
            Self::Unavailable(reason) => formatter
                .debug_tuple("TargetBinding::Unavailable")
                .field(reason)
                .finish(),
        }
    }
}

/// Admission-time source of the immutable target binding.
///
/// The daemon calls this exactly once while admitting a session — before any
/// capture, activation or focus side effect — and carries the returned value
/// into the owned delivery worker. Implementations must be observers: they may
/// never activate, focus or synthesize input, and they must never be consulted
/// again from [`RoutedThenInboxDelivery::deliver`].
pub trait TargetBindingSource: Send {
    fn bind(&mut self) -> TargetBinding;
}

/// The production admission binding source for the current installed Start
/// path.
///
/// The IPC `StartSession` command carries only a self-declared activation
/// reason string, which is not a window identity, and no accepted resolver
/// binds a target at admission yet. This source therefore returns the explicit
/// typed-unavailable input: every target-requiring rung is skipped and the
/// exact text is recovered durably in the Inbox. Supplying a real binding (a
/// trustworthy pre-activation target provider wired into Start) is the
/// remaining live-trigger integration blocker; it must not be faked with a
/// late query, a desktop name or the activation string.
pub struct UnavailablePreActivationTargetBinding;

impl TargetBindingSource for UnavailablePreActivationTargetBinding {
    fn bind(&mut self) -> TargetBinding {
        TargetBinding::Unavailable(PRE_ACTIVATION_BINDING_ABSENT)
    }
}

// ---------------------------------------------------------------------------
// Permission evidence
// ---------------------------------------------------------------------------

/// Content-free evidence that gates the external boundaries of the ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DeliveryPermissionEvidence {
    /// A live, permissioned external insertion path exists (for example an
    /// active portal keyboard session with a connected sender). `false` means
    /// every activation-requiring rung is skipped, never retried here.
    pub keyboard_insert: bool,
    /// Prior clipboard content may be read so the previous value can be
    /// restored after an ownership-aware paste. Privacy authorization must be
    /// decided before this flag is set; the adapter never reads the clipboard
    /// without it.
    pub clipboard_restore: bool,
}

/// Permission/authorization evidence port. Implementations must be
/// side-effect-free observers (snapshot + policy), never permission requests.
pub trait DeliveryPermissionPort: Send {
    fn evidence(&mut self) -> DeliveryPermissionEvidence;
}

/// Default evidence: every external boundary denied. Used by tests and by any
/// composition that has not proven permission.
pub struct DeniedDeliveryPermission;

impl DeliveryPermissionPort for DeniedDeliveryPermission {
    fn evidence(&mut self) -> DeliveryPermissionEvidence {
        DeliveryPermissionEvidence::default()
    }
}

// ---------------------------------------------------------------------------
// Rung contract
// ---------------------------------------------------------------------------

/// Evidence gate result for one rung.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RungQualification {
    /// All required evidence is present; the rung may be attempted.
    Qualified,
    /// Required evidence is absent. The reason is a short stable code.
    Unqualified(&'static str),
}

/// Fact observed by one rung attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RungOutcome {
    /// The rung verified its own durable target result.
    Committed,
    /// A transport/helper accepted the payload but target retention cannot be
    /// verified. An attempt fact, never a verified-delivery claim. Like
    /// `Committed`, it ends the ladder without an Inbox recovery write (F-M4).
    SubmittedUnverified,
    /// The payload is on the clipboard; insertion was not attempted or not
    /// proven.
    CopiedOnly,
    /// Bounded failure after the payload may have left the process. The
    /// ladder ends here: another rung could insert the text twice.
    Failed(FallbackReason),
    /// Bounded failure before anything left the process (for example the
    /// clipboard could not be captured). The ladder continues.
    NotDispatched(FallbackReason),
    /// Nothing was dispatched by this rung. Cancellation is the only cause.
    CancelledBeforeDispatch,
}

/// One priority rung of the approved ladder.
pub trait DirectRung: Send {
    /// Stable content-free rung identity.
    fn id(&self) -> &'static str;

    /// Backend identity recorded on the portable attempt. Defaults to
    /// [`Self::id`]; tool-specific rungs may refine it.
    fn attempt_backend(&self) -> String {
        self.id().to_owned()
    }

    /// True when this rung may change activation/focus and therefore requires
    /// a target bound before it runs (at admission, never at delivery time).
    fn requires_target(&self) -> bool {
        false
    }

    /// Evaluate qualification evidence. Must be side-effect-free.
    fn qualification(
        &mut self,
        target: &TargetBinding,
        permission: &DeliveryPermissionEvidence,
        cancellation: &SessionCancellation,
    ) -> RungQualification;

    /// Attempt one bounded dispatch. Must never panic and must never log text.
    fn attempt(&mut self, text: &str, cancellation: &SessionCancellation) -> RungOutcome;

    /// Optional content-free restore outcome observed by the last attempt.
    fn last_restore_outcome(&self) -> Option<ClipboardRestoreOutcome> {
        None
    }
}

/// A ladder place that is deliberately NOT composed in the installed lane
/// because no accepted trusted implementation exists for it yet.
///
/// The rung is still reported: it always answers
/// [`RungQualification::Unqualified`] with the stable reason it was declared
/// with, so every report truthfully names the mechanism and why it could not
/// run instead of silently omitting it. It can never dispatch.
pub struct UnavailableRung {
    id: &'static str,
    reason: &'static str,
}

impl UnavailableRung {
    #[must_use]
    pub fn new(id: &'static str, reason: &'static str) -> Self {
        Self { id, reason }
    }

    /// Clipboard-paste rung declared unavailable (no trusted helper accepted).
    #[must_use]
    pub fn clipboard_paste(reason: &'static str) -> Self {
        Self::new("clipboard-paste", reason)
    }

    /// Clipboard-copy rung declared unavailable (no trusted helper accepted).
    #[must_use]
    pub fn clipboard_copy(reason: &'static str) -> Self {
        Self::new("clipboard-copy", reason)
    }

    #[must_use]
    pub fn reason(&self) -> &'static str {
        self.reason
    }
}

impl DirectRung for UnavailableRung {
    fn id(&self) -> &'static str {
        self.id
    }

    fn qualification(
        &mut self,
        _target: &TargetBinding,
        _permission: &DeliveryPermissionEvidence,
        cancellation: &SessionCancellation,
    ) -> RungQualification {
        if cancellation.is_cancelled() {
            return RungQualification::Unqualified("cancelled");
        }
        RungQualification::Unqualified(self.reason)
    }

    fn attempt(&mut self, _text: &str, _cancellation: &SessionCancellation) -> RungOutcome {
        // Unreachable in the ladder: qualification is always unqualified. The
        // rung reports a bounded, content-free failure if it is ever driven
        // directly instead of dropping payload.
        RungOutcome::NotDispatched(FallbackReason::NoQualifiedBackend)
    }
}

// ---------------------------------------------------------------------------
// Bounded transaction-keyed replay record
// ---------------------------------------------------------------------------

/// Default bound on dispatched transactions retained by one adapter instance.
pub const DEFAULT_REPLAY_CAPACITY: usize = 64;

/// Content-free binding of one dispatched transaction to its exact payload.
///
/// A SHA-256 digest plus the byte length is retained, never the payload itself,
/// so replay validation can reject a reused transaction id with different text
/// without storing transcript plaintext anywhere. A stable content digest is
/// required: a process-local `DefaultHasher` value is neither stable nor
/// collision-resistant, so two equal-length payloads could collide and replay
/// another transaction's report for different text.
#[derive(Clone, Copy, PartialEq, Eq)]
struct PayloadFingerprint {
    digest: [u8; 32],
    length: usize,
}

impl PayloadFingerprint {
    fn of(text: &str) -> Self {
        let digest = Sha256::digest(text.as_bytes());
        let mut bytes = [0_u8; 32];
        bytes.copy_from_slice(digest.as_slice());
        Self {
            digest: bytes,
            length: text.len(),
        }
    }
}

impl std::fmt::Debug for PayloadFingerprint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // PRIVACY: the digest is content-derived; it is compared for replay
        // ownership, never rendered in diagnostics or UI.
        formatter
            .debug_struct("PayloadFingerprint")
            .field("digest", &"[redacted]")
            .field("length", &self.length)
            .finish()
    }
}

#[derive(Debug, Clone)]
struct ReplayEntry {
    transaction_id: TransactionId,
    fingerprint: PayloadFingerprint,
    report: DeliveryReport,
}

/// Result of checking one transaction against the replay record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayCheck {
    /// The transaction already dispatched: return the observed facts verbatim.
    Replay(DeliveryReport),
    /// The transaction id was already used for a different payload. The call
    /// is rejected before any dispatch or Inbox overwrite.
    RejectedPayloadMismatch,
    /// The transaction is new and the record can accept it.
    Admissible,
    /// The bounded record is full. Nothing is evicted: every new transaction
    /// is skipped (no direct dispatch) and recovered durably in the Inbox, so
    /// an old dispatched transaction can never become dispatchable again.
    Saturated,
}

/// Bounded, transaction-keyed dispatch-ownership record.
///
/// Capacity is fixed for the adapter's lifetime and no dispatched record is
/// ever evicted. Once the record is full every further transaction fails
/// closed to the durable Inbox sink without a direct dispatch, which keeps the
/// commit-once guarantee valid for the whole adapter lifetime instead of only
/// for an immediate retry.
#[derive(Debug, Clone)]
pub struct DeliveryReplayCache {
    capacity: usize,
    entries: VecDeque<ReplayEntry>,
    saturated: bool,
}

impl DeliveryReplayCache {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: VecDeque::new(),
            saturated: false,
        }
    }

    /// Classify one `(transaction id, payload)` pair without side effects.
    #[must_use]
    pub fn check(&self, transaction_id: &TransactionId, text: &str) -> ReplayCheck {
        let fingerprint = PayloadFingerprint::of(text);
        if let Some(entry) = self
            .entries
            .iter()
            .find(|entry| &entry.transaction_id == transaction_id)
        {
            return if entry.fingerprint == fingerprint {
                ReplayCheck::Replay(entry.report.clone())
            } else {
                ReplayCheck::RejectedPayloadMismatch
            };
        }
        if self.saturated || self.entries.len() >= self.capacity {
            ReplayCheck::Saturated
        } else {
            ReplayCheck::Admissible
        }
    }

    /// Record one dispatched transaction. Existing entries are never replaced
    /// and no entry is ever evicted; a full record latches [`Self::saturated`]
    /// so later transactions fail closed.
    fn record(&mut self, transaction_id: &TransactionId, text: &str, report: &DeliveryReport) {
        if self
            .entries
            .iter()
            .any(|entry| &entry.transaction_id == transaction_id)
        {
            return;
        }
        if self.entries.len() >= self.capacity {
            self.saturated = true;
            return;
        }
        self.entries.push_back(ReplayEntry {
            transaction_id: transaction_id.clone(),
            fingerprint: PayloadFingerprint::of(text),
            report: report.clone(),
        });
    }
}

// ---------------------------------------------------------------------------
// Routed adapter
// ---------------------------------------------------------------------------

/// Approved target-aware routed delivery adapter.
///
/// The durable Inbox destination is established before any direct rung can
/// receive payload; the immutable admission-time [`TargetBinding`] is consumed
/// here and never re-queried, so later focus state cannot redirect a rung. The
/// first observed report for a transaction is the only dispatch for it.
pub struct RoutedThenInboxDelivery {
    store: InboxStore,
    binding: TargetBinding,
    permission: Box<dyn DeliveryPermissionPort>,
    rungs: Vec<Box<dyn DirectRung>>,
    replay: DeliveryReplayCache,
}

impl RoutedThenInboxDelivery {
    /// Compose the adapter with the immutable target binding captured while the
    /// session was admitted. The Inbox target is prepared here: a composition
    /// that cannot durably recover text never starts a direct attempt.
    pub fn new(
        inbox_dir: PathBuf,
        binding: TargetBinding,
        permission: Box<dyn DeliveryPermissionPort>,
        rungs: Vec<Box<dyn DirectRung>>,
    ) -> Result<Self, PlatformError> {
        Self::with_replay_capacity(
            DEFAULT_REPLAY_CAPACITY,
            inbox_dir,
            binding,
            permission,
            rungs,
        )
    }

    /// Compose the adapter with an explicit bounded replay-record capacity.
    /// Controlled compositions use a small capacity to exercise the
    /// fail-closed saturation boundary; it never changes dispatch semantics.
    pub fn with_replay_capacity(
        replay_capacity: usize,
        inbox_dir: PathBuf,
        binding: TargetBinding,
        permission: Box<dyn DeliveryPermissionPort>,
        rungs: Vec<Box<dyn DirectRung>>,
    ) -> Result<Self, PlatformError> {
        let store = InboxStore::new(inbox_dir);
        store
            .ensure_private_directory()
            .map_err(|_| error("inbox_unavailable"))?;
        Ok(Self {
            store,
            binding,
            permission,
            rungs,
            replay: DeliveryReplayCache::new(replay_capacity),
        })
    }

    /// Content-free classification of the immutable admission binding; never
    /// returns the captured identity.
    #[must_use]
    pub fn target_binding_class(&self) -> &'static str {
        self.binding.diagnostic_class()
    }

    /// Content-free rung identities in priority order.
    #[must_use]
    pub fn rung_ids(&self) -> Vec<&'static str> {
        self.rungs.iter().map(|rung| rung.id()).collect()
    }

    /// Content-free restore outcome of the last paste rung attempt, if any.
    #[must_use]
    pub fn last_restore_outcome(&self) -> Option<ClipboardRestoreOutcome> {
        self.rungs
            .iter()
            .find_map(|rung| rung.last_restore_outcome())
    }

    fn report(
        &self,
        transaction_id: &TransactionId,
        attempts: Vec<DeliveryAttempt>,
        outcome: DeliveryOutcome,
        failure_reason: Option<FallbackReason>,
    ) -> DeliveryReport {
        DeliveryReport {
            transaction_id: transaction_id.clone(),
            backend: ROUTED_THEN_INBOX_BACKEND.to_owned(),
            attempts,
            outcome,
            failure_reason,
        }
    }
}

impl DeliveryPort for RoutedThenInboxDelivery {
    fn deliver(
        &mut self,
        intent: &DeliveryIntent,
        text: &str,
        cancellation: &SessionCancellation,
    ) -> Result<DeliveryReport, PlatformError> {
        if intent.backend != ROUTED_THEN_INBOX_BACKEND {
            return Err(error("delivery_intent_invalid"));
        }
        // Commit-once: a stale/late retry for a transaction that already
        // crossed a dispatch boundary receives the same observed facts and can
        // never insert the text a second time. A reused transaction id with a
        // different payload is rejected here, before any rung or Inbox write.
        let replay_saturated = match self.replay.check(&intent.transaction_id, text) {
            ReplayCheck::Replay(report) => return Ok(report),
            ReplayCheck::RejectedPayloadMismatch => {
                return Err(error("delivery_transaction_reuse_rejected"));
            }
            ReplayCheck::Admissible => false,
            ReplayCheck::Saturated => true,
        };
        if cancellation.is_cancelled() {
            return Err(error("cancelled"));
        }

        // The target was bound while the session was admitted, before any
        // activation/focus side effect; it is never queried here.
        let target = &self.binding;
        let permission = self.permission.evidence();
        let oversize = text.len() > MAX_DIRECT_TEXT_BYTES;
        let mut attempts: Vec<DeliveryAttempt> = Vec::new();
        let mut dispatched = false;

        for rung in self.rungs.iter_mut() {
            if cancellation.is_cancelled() {
                break;
            }
            if replay_saturated {
                attempts.push(attempt(
                    rung.attempt_backend(),
                    DeliveryOutcome::Failed,
                    Some(FallbackReason::BackendError {
                        code: "delivery_replay_capacity_exhausted".to_owned(),
                    }),
                ));
                continue;
            }
            if oversize {
                attempts.push(attempt(
                    rung.attempt_backend(),
                    DeliveryOutcome::Failed,
                    Some(FallbackReason::PolicyDenied),
                ));
                continue;
            }
            if rung.requires_target() && !target.is_bound() {
                attempts.push(attempt(
                    rung.attempt_backend(),
                    DeliveryOutcome::Failed,
                    Some(FallbackReason::NoQualifiedBackend),
                ));
                continue;
            }
            match rung.qualification(target, &permission, cancellation) {
                RungQualification::Unqualified(code) => {
                    attempts.push(attempt(
                        rung.attempt_backend(),
                        DeliveryOutcome::Failed,
                        Some(FallbackReason::BackendError {
                            code: code.to_owned(),
                        }),
                    ));
                    continue;
                }
                RungQualification::Qualified => {}
            }
            match rung.attempt(text, cancellation) {
                RungOutcome::CancelledBeforeDispatch => break,
                RungOutcome::Committed => {
                    attempts.push(attempt(
                        rung.attempt_backend(),
                        DeliveryOutcome::Delivered,
                        None,
                    ));
                    let report = self.report(
                        &intent.transaction_id,
                        attempts,
                        DeliveryOutcome::Delivered,
                        None,
                    );
                    self.replay.record(&intent.transaction_id, text, &report);
                    return Ok(report);
                }
                RungOutcome::SubmittedUnverified => {
                    // The helper accepted the whole payload, so the text most
                    // likely landed. A later rung would insert it a second
                    // time, so the ladder ends here. A successful submission
                    // leaves no plaintext Inbox copy (F-M4): the Inbox keeps
                    // only text whose delivery failed or is uncertain.
                    attempts.push(attempt(
                        rung.attempt_backend(),
                        DeliveryOutcome::SubmittedUnverified,
                        None,
                    ));
                    let report = self.report(
                        &intent.transaction_id,
                        attempts,
                        DeliveryOutcome::SubmittedUnverified,
                        None,
                    );
                    self.replay.record(&intent.transaction_id, text, &report);
                    return Ok(report);
                }
                RungOutcome::CopiedOnly => {
                    dispatched = true;
                    attempts.push(attempt(
                        rung.attempt_backend(),
                        DeliveryOutcome::CopiedOnly,
                        None,
                    ));
                    break;
                }
                RungOutcome::NotDispatched(reason) => {
                    attempts.push(attempt(
                        rung.attempt_backend(),
                        DeliveryOutcome::Failed,
                        Some(reason),
                    ));
                }
                RungOutcome::Failed(reason) => {
                    // Policy-denied payloads never left the process and the
                    // ladder continues. Every other bounded failure may
                    // already have crossed a dispatch boundary: at most one
                    // rung dispatches per transaction, so only the durable
                    // recovery write follows.
                    let denied = matches!(reason, FallbackReason::PolicyDenied);
                    attempts.push(attempt(
                        rung.attempt_backend(),
                        DeliveryOutcome::Failed,
                        Some(reason),
                    ));
                    if !denied {
                        dispatched = true;
                        break;
                    }
                }
            }
        }

        if !dispatched && cancellation.is_cancelled() {
            // Nothing left the process: a cancellation can terminate the
            // transaction without a delivery record or an Inbox artifact.
            return Err(error("cancelled"));
        }

        match self.store.store_text(&intent.transaction_id, text) {
            Ok(_) => {
                attempts.push(attempt(
                    INBOX_BACKEND.to_owned(),
                    DeliveryOutcome::InboxFallback,
                    None,
                ));
                let report = self.report(
                    &intent.transaction_id,
                    attempts,
                    DeliveryOutcome::InboxFallback,
                    None,
                );
                // The recovered transaction still owns its dispatch decision:
                // recording it here is what makes an A -> B -> stale A retry
                // safe for the whole adapter lifetime.
                self.replay.record(&intent.transaction_id, text, &report);
                Ok(report)
            }
            Err(err) => {
                let code = match err {
                    InboxError::InvalidTxid => "INBOX_TXID_INVALID",
                    InboxError::Io(_) => "INBOX_WRITE_FAILED",
                };
                attempts.push(attempt(
                    INBOX_BACKEND.to_owned(),
                    DeliveryOutcome::Failed,
                    Some(FallbackReason::BackendError {
                        code: code.to_owned(),
                    }),
                ));
                let report = self.report(
                    &intent.transaction_id,
                    attempts,
                    DeliveryOutcome::Failed,
                    Some(FallbackReason::BackendError {
                        code: code.to_owned(),
                    }),
                );
                // A stale retry must not re-dispatch insertion when a direct
                // rung already ran; only a purely-unqualified ladder may retry.
                if dispatched {
                    self.replay.record(&intent.transaction_id, text, &report);
                }
                Ok(report)
            }
        }
    }
}

fn attempt(
    backend: String,
    outcome: DeliveryOutcome,
    fallback_reason: Option<FallbackReason>,
) -> DeliveryAttempt {
    DeliveryAttempt {
        backend,
        outcome,
        fallback_reason,
        at_unix_ms: now_unix_ms(),
    }
}

fn error(code: &'static str) -> PlatformError {
    PlatformError {
        code,
        message: code.to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Portal/EIS text rung
// ---------------------------------------------------------------------------

/// One portal/EIS keyboard text-frame rung. Submission is always an
/// unverified attempt: the portal contract cannot prove target retention.
pub struct PortalEisTextRung<S> {
    submitter: S,
}

impl<S> PortalEisTextRung<S> {
    pub fn new(submitter: S) -> Self {
        Self { submitter }
    }
}

impl<S: PortalEisTextSubmitter> DirectRung for PortalEisTextRung<S> {
    fn id(&self) -> &'static str {
        "portal-eis-text"
    }

    fn requires_target(&self) -> bool {
        // The portal session may activate/focus the target, so the real target
        // must have been captured before this rung is allowed to run.
        true
    }

    fn qualification(
        &mut self,
        _target: &TargetBinding,
        permission: &DeliveryPermissionEvidence,
        cancellation: &SessionCancellation,
    ) -> RungQualification {
        if cancellation.is_cancelled() {
            return RungQualification::Unqualified("cancelled");
        }
        if !permission.keyboard_insert {
            return RungQualification::Unqualified("permission_inactive");
        }
        RungQualification::Qualified
    }

    fn attempt(&mut self, text: &str, cancellation: &SessionCancellation) -> RungOutcome {
        let deadline = Instant::now() + PORTAL_TEXT_BOUND;
        let payload = match PortalEisTextPayload::parse(text) {
            Ok(payload) => payload,
            Err(failure) => return portal_failure(failure),
        };
        match self.submitter.submit(payload, cancellation, deadline) {
            PortalEisTextSubmission::CancelledBeforeDispatch => {
                RungOutcome::CancelledBeforeDispatch
            }
            PortalEisTextSubmission::SubmittedUnverified => RungOutcome::SubmittedUnverified,
            PortalEisTextSubmission::RejectedBeforeDispatch(failure) => {
                match portal_failure(failure) {
                    RungOutcome::Failed(reason) => RungOutcome::NotDispatched(reason),
                    other => other,
                }
            }
            PortalEisTextSubmission::DispatchUncertain(failure) => portal_failure(failure),
        }
    }
}

fn portal_failure(failure: PortalEisTextFailure) -> RungOutcome {
    RungOutcome::Failed(match failure {
        PortalEisTextFailure::TimedOut => FallbackReason::Timeout,
        PortalEisTextFailure::Cancelled => FallbackReason::BackendError {
            code: "PORTAL_EIS_CANCELLED".to_owned(),
        },
        other => FallbackReason::BackendError {
            code: other.code().to_owned(),
        },
    })
}

// ---------------------------------------------------------------------------
// Virtual-keyboard rung
// ---------------------------------------------------------------------------

/// Session support evidence for the Wayland virtual-keyboard helper: the
/// helper is trusted/present AND the scrubbed launch environment it needs is
/// actually present. No desktop/OS name is used as a feature flag.
#[must_use]
pub fn wayland_text_input_supported() -> bool {
    let non_empty = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
    non_empty("XDG_RUNTIME_DIR") && non_empty("WAYLAND_DISPLAY")
}

/// Real `wtype` rung. Qualification is decided when the rung is composed from
/// real helper/trust/session evidence; an attempt is always unverified.
pub struct SystemWtypeKeyboardRung {
    submitter: Option<SystemWtypeSubmitter>,
    unavailable: Option<&'static str>,
}

impl SystemWtypeKeyboardRung {
    #[must_use]
    pub fn new() -> Self {
        if !wayland_text_input_supported() {
            return Self {
                submitter: None,
                unavailable: Some("wayland_session_absent"),
            };
        }
        match wtype_submitter_status() {
            WtypeSubmitterStatus::Available(_) => {}
            WtypeSubmitterStatus::Missing => {
                return Self {
                    submitter: None,
                    unavailable: Some("wtype_missing"),
                };
            }
            WtypeSubmitterStatus::NonExecutable => {
                return Self {
                    submitter: None,
                    unavailable: Some("wtype_nonexecutable"),
                };
            }
            WtypeSubmitterStatus::Untrusted => {
                return Self {
                    submitter: None,
                    unavailable: Some("wtype_untrusted"),
                };
            }
            WtypeSubmitterStatus::IdentityChanged => {
                return Self {
                    submitter: None,
                    unavailable: Some("wtype_identity_changed"),
                };
            }
            WtypeSubmitterStatus::UnsupportedPlatform => {
                return Self {
                    submitter: None,
                    unavailable: Some("wtype_isolation_unavailable"),
                };
            }
        }
        match SystemWtypeSubmitter::new() {
            Ok(submitter) => Self {
                submitter: Some(submitter),
                unavailable: None,
            },
            Err(err) => Self {
                submitter: None,
                unavailable: Some(err.code),
            },
        }
    }
}

impl Default for SystemWtypeKeyboardRung {
    fn default() -> Self {
        Self::new()
    }
}

/// Deterministic rung over any [`WtypeSubmitter`] (controlled tests).
pub struct WtypeKeyboardRung<S> {
    submitter: S,
}

impl<S> WtypeKeyboardRung<S> {
    pub fn new(submitter: S) -> Self {
        Self { submitter }
    }
}

impl<S: WtypeSubmitter> DirectRung for WtypeKeyboardRung<S> {
    fn id(&self) -> &'static str {
        "wtype-virtual-keyboard"
    }

    fn qualification(
        &mut self,
        _target: &TargetBinding,
        _permission: &DeliveryPermissionEvidence,
        cancellation: &SessionCancellation,
    ) -> RungQualification {
        if cancellation.is_cancelled() {
            RungQualification::Unqualified("cancelled")
        } else {
            RungQualification::Qualified
        }
    }

    fn attempt(&mut self, text: &str, cancellation: &SessionCancellation) -> RungOutcome {
        match self.submitter.submit(text.as_bytes(), cancellation) {
            WtypeSubmission::CancelledBeforeDispatch => RungOutcome::CancelledBeforeDispatch,
            WtypeSubmission::SubmittedUnverified => RungOutcome::SubmittedUnverified,
            WtypeSubmission::Failed(reason) => RungOutcome::Failed(reason),
        }
    }
}

impl DirectRung for SystemWtypeKeyboardRung {
    fn id(&self) -> &'static str {
        "wtype-virtual-keyboard"
    }

    fn qualification(
        &mut self,
        _target: &TargetBinding,
        _permission: &DeliveryPermissionEvidence,
        cancellation: &SessionCancellation,
    ) -> RungQualification {
        if cancellation.is_cancelled() {
            return RungQualification::Unqualified("cancelled");
        }
        match self.unavailable {
            Some(reason) => RungQualification::Unqualified(reason),
            None => RungQualification::Qualified,
        }
    }

    fn attempt(&mut self, text: &str, cancellation: &SessionCancellation) -> RungOutcome {
        let Some(submitter) = self.submitter.as_mut() else {
            return RungOutcome::NotDispatched(FallbackReason::NoQualifiedBackend);
        };
        match submitter.submit(text.as_bytes(), cancellation) {
            WtypeSubmission::CancelledBeforeDispatch => RungOutcome::CancelledBeforeDispatch,
            WtypeSubmission::SubmittedUnverified => RungOutcome::SubmittedUnverified,
            WtypeSubmission::Failed(reason) => RungOutcome::Failed(reason),
        }
    }
}

// ---------------------------------------------------------------------------
// Clipboard snapshot / ownership-aware restoration
// ---------------------------------------------------------------------------

/// Read/write/ownership boundary of a system clipboard. Implementations must
/// bound every tool invocation, never log clipboard content, and must not read
/// the clipboard unless the caller's authorization evidence allowed it.
pub trait ClipboardHandoff: Send {
    /// What a snapshot holds (every format of the user's clipboard, or plain
    /// text for a simple implementation). Never rendered or logged.
    type Snapshot: Send;

    /// Content-free availability probe.
    fn probe(&mut self) -> Result<(), &'static str>;

    /// Capture the current value so it can be put back.
    /// - `Ok(Some(_))`: captured, and restorable after the paste;
    /// - `Ok(None)`: nothing to restore (an empty clipboard);
    /// - `Err(code)`: the value exists but cannot be captured whole (too
    ///   large, unreadable). The paste is then refused rather than lose the
    ///   user's clipboard, and the ladder moves on.
    fn snapshot(&mut self) -> Result<Option<Self::Snapshot>, &'static str>;

    /// Replace the clipboard with `text`.
    fn write(&mut self, text: &str) -> Result<ClipboardOwnership, &'static str>;

    /// Wait, at most `bound`, until the target has read what `ownership`
    /// wrote, so the user's value is not restored before the paste consumed
    /// ours. Returns whether a read was observed, or `None` when reads cannot
    /// be observed (the default, which returns at once).
    fn wait_until_consumed(
        &mut self,
        _ownership: &ClipboardOwnership,
        _bound: Duration,
    ) -> Option<bool> {
        None
    }

    /// Called just before the paste chord, so reads that happened earlier
    /// (a clipboard manager copies every new value at once) are not taken
    /// for the target's paste. The default does nothing.
    fn before_chord(&mut self, _ownership: &ClipboardOwnership) {}

    /// True only when the current clipboard still holds exactly what
    /// `ownership` wrote; a newer user value must never be overwritten.
    fn still_owned(&mut self, ownership: &ClipboardOwnership, text: &str) -> bool;

    /// Restore a previously snapshotted value. The caller must have verified
    /// ownership first.
    fn restore(&mut self, prior: &Self::Snapshot) -> Result<(), &'static str>;
}

/// How long a paste may take to read the clipboard before the user's value is
/// put back anyway.
pub const PASTE_CONSUME_BOUND: Duration = Duration::from_millis(2_000);

/// The user's clipboard never comes back sooner than this after the chord.
pub const PASTE_MIN_HOLD: Duration = Duration::from_millis(500);

/// Opaque per-write ownership token. Content-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipboardOwnership {
    pub(crate) write_serial: u64,
}

/// What happened to the user's clipboard after an ownership-aware paste.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardRestoreOutcome {
    /// No prior value could be read, so nothing was restored.
    NotRestorable,
    /// The prior value was restored because Narrivy still owned the clipboard.
    RestoredPrior,
    /// A newer clipboard value exists; it was deliberately left untouched.
    PreservedNewerContent,
    /// The restoration write itself failed (bounded, content-free).
    Failed(&'static str),
    /// Nothing read the clipboard after the chord, so the paste did not
    /// happen: the text was left on the clipboard for the user to paste.
    LeftForUser,
}

/// Synthesized paste-chord boundary.
pub trait PasteChord: Send {
    fn qualification(&self) -> RungQualification;
    fn send_paste_chord(&mut self, cancellation: &SessionCancellation) -> Result<(), &'static str>;
}

/// Deterministic chord used when no real keyboard mechanism is qualified.
pub struct UnavailablePasteChord(&'static str);

impl UnavailablePasteChord {
    #[must_use]
    pub fn new(reason: &'static str) -> Self {
        Self(reason)
    }
}

impl PasteChord for UnavailablePasteChord {
    fn qualification(&self) -> RungQualification {
        RungQualification::Unqualified(self.0)
    }

    fn send_paste_chord(&mut self, _: &SessionCancellation) -> Result<(), &'static str> {
        Err(self.0)
    }
}

/// Pure ownership/restoration policy for one clipboard paste.
///
/// Order is fixed: snapshot -> write ours -> paste -> ownership check ->
/// restore-only-if-owned. A clipboard that no longer holds exactly our payload
/// is a newer user change and is preserved, never clobbered.
pub fn paste_with_ownership_restore<H: ClipboardHandoff + ?Sized>(
    handoff: &mut H,
    chord: &mut dyn PasteChord,
    text: &str,
    cancellation: &SessionCancellation,
) -> (RungOutcome, ClipboardRestoreOutcome) {
    if cancellation.is_cancelled() {
        return (
            RungOutcome::CancelledBeforeDispatch,
            ClipboardRestoreOutcome::NotRestorable,
        );
    }
    // A clipboard that cannot be captured whole is never replaced: the paste
    // is refused and the ladder moves on to the next rung.
    let prior = match handoff.snapshot() {
        Ok(prior) => prior,
        Err(code) => {
            return (
                RungOutcome::NotDispatched(FallbackReason::BackendError {
                    code: code.to_owned(),
                }),
                ClipboardRestoreOutcome::NotRestorable,
            );
        }
    };
    let ownership = match handoff.write(text) {
        Ok(ownership) => ownership,
        Err(code) => {
            // No chord was sent, so nothing reached a window.
            return (
                RungOutcome::NotDispatched(FallbackReason::BackendError {
                    code: code.to_owned(),
                }),
                ClipboardRestoreOutcome::Failed(code),
            );
        }
    };
    handoff.before_chord(&ownership);
    let chord_result = chord.send_paste_chord(cancellation);
    // Give the target time to read our value before the user's comes back:
    // until it has read it, and never sooner than the minimum hold (some
    // applications read more than once, or a beat after the key).
    let mut unread = false;
    if chord_result.is_ok() {
        let sent = Instant::now();
        unread = handoff.wait_until_consumed(&ownership, PASTE_CONSUME_BOUND) == Some(false);
        if let Some(rest) = PASTE_MIN_HOLD.checked_sub(sent.elapsed()) {
            std::thread::sleep(rest);
        }
    }
    // Restoration must happen whether or not the chord itself succeeded:
    // leaving Narrivy's temporary value behind would silently replace the
    // user's clipboard.
    if !handoff.still_owned(&ownership, text) {
        return (
            chord_outcome(chord_result),
            ClipboardRestoreOutcome::PreservedNewerContent,
        );
    }
    // Still ours and never read after the chord: the window did not paste
    // (focus moved, the key went elsewhere). Restoring would lose the text,
    // so it stays on the clipboard and the user is told to paste it.
    if unread {
        return (
            RungOutcome::CopiedOnly,
            ClipboardRestoreOutcome::LeftForUser,
        );
    }
    match prior {
        Some(prior) => match handoff.restore(&prior) {
            Ok(()) => (
                chord_outcome(chord_result),
                ClipboardRestoreOutcome::RestoredPrior,
            ),
            Err(code) => (
                chord_outcome(chord_result),
                ClipboardRestoreOutcome::Failed(code),
            ),
        },
        None => (
            chord_outcome(chord_result),
            ClipboardRestoreOutcome::NotRestorable,
        ),
    }
}

fn chord_outcome(result: Result<(), &'static str>) -> RungOutcome {
    match result {
        Ok(()) => RungOutcome::SubmittedUnverified,
        Err(code) => RungOutcome::Failed(FallbackReason::BackendError {
            code: code.to_owned(),
        }),
    }
}

/// Clipboard-paste rung. Requires a known target (it synthesizes input), an
/// authorization for reading/restoring the prior value, and a qualified paste
/// chord.
pub struct ClipboardPasteRung<H: ClipboardHandoff, C: PasteChord> {
    handoff: H,
    chord: C,
    last_restore: Option<ClipboardRestoreOutcome>,
    requires_target: bool,
}

impl<H: ClipboardHandoff, C: PasteChord> ClipboardPasteRung<H, C> {
    /// A rung that runs only when a target was bound at admission.
    pub fn new(handoff: H, chord: C) -> Self {
        Self {
            handoff,
            chord,
            last_restore: None,
            requires_target: true,
        }
    }

    /// A rung that pastes into whichever window has keyboard focus, exactly
    /// like the virtual-keyboard typing rung: it changes no focus and no
    /// activation, so it needs no bound target.
    pub fn for_focused_window(handoff: H, chord: C) -> Self {
        Self {
            requires_target: false,
            ..Self::new(handoff, chord)
        }
    }
}

impl<H: ClipboardHandoff, C: PasteChord> DirectRung for ClipboardPasteRung<H, C> {
    fn id(&self) -> &'static str {
        "clipboard-paste"
    }

    fn requires_target(&self) -> bool {
        self.requires_target
    }

    fn qualification(
        &mut self,
        _target: &TargetBinding,
        permission: &DeliveryPermissionEvidence,
        cancellation: &SessionCancellation,
    ) -> RungQualification {
        if cancellation.is_cancelled() {
            return RungQualification::Unqualified("cancelled");
        }
        if !permission.clipboard_restore {
            return RungQualification::Unqualified("clipboard_restore_not_authorized");
        }
        let chord = self.chord.qualification();
        if chord != RungQualification::Qualified {
            return chord;
        }
        match self.handoff.probe() {
            Ok(()) => RungQualification::Qualified,
            Err(code) => RungQualification::Unqualified(code),
        }
    }

    fn attempt(&mut self, text: &str, cancellation: &SessionCancellation) -> RungOutcome {
        let (outcome, restore) =
            paste_with_ownership_restore(&mut self.handoff, &mut self.chord, text, cancellation);
        self.last_restore = Some(restore);
        outcome
    }

    fn last_restore_outcome(&self) -> Option<ClipboardRestoreOutcome> {
        self.last_restore
    }
}

// ---------------------------------------------------------------------------
// Clipboard copy-only rung
// ---------------------------------------------------------------------------

/// Clipboard copy-only rung over the shared ladder backend. Copies without
/// synthesizing input and reports [`DeliveryOutcome::CopiedOnly`]; the routed
/// adapter then continues to durable Inbox recovery.
pub struct ClipboardCopyRung<'r> {
    backend: ClipboardBackend<'r>,
}

impl<'r> ClipboardCopyRung<'r> {
    #[must_use]
    pub fn with_runner(tool: Option<ClipboardTool>, runner: &'r dyn CommandRunner) -> Self {
        Self {
            backend: ClipboardBackend::new(tool, runner),
        }
    }
}

impl DirectRung for ClipboardCopyRung<'_> {
    fn id(&self) -> &'static str {
        self.backend.id().label()
    }

    fn qualification(
        &mut self,
        _target: &TargetBinding,
        _permission: &DeliveryPermissionEvidence,
        cancellation: &SessionCancellation,
    ) -> RungQualification {
        if cancellation.is_cancelled() {
            return RungQualification::Unqualified("cancelled");
        }
        match self.backend.availability() {
            BackendAvailability::ProbeAvailable => RungQualification::Qualified,
            BackendAvailability::ProbeMissing(reason) => {
                RungQualification::Unqualified(leak_reason(reason))
            }
        }
    }

    fn attempt(&mut self, text: &str, cancellation: &SessionCancellation) -> RungOutcome {
        if cancellation.is_cancelled() {
            return RungOutcome::CancelledBeforeDispatch;
        }
        let request = DeliveryRequest {
            text,
            txid: &TransactionId::new("routed-probe"),
        };
        match self.backend.deliver(&request) {
            AttemptOutcome {
                outcome: DeliveryOutcome::CopiedOnly,
                ..
            } => RungOutcome::CopiedOnly,
            AttemptOutcome {
                outcome: DeliveryOutcome::SubmittedUnverified,
                ..
            } => RungOutcome::SubmittedUnverified,
            AttemptOutcome {
                outcome: DeliveryOutcome::Delivered,
                ..
            } => RungOutcome::Committed,
            AttemptOutcome {
                fallback_reason, ..
            } => RungOutcome::Failed(fallback_reason.unwrap_or(FallbackReason::BackendError {
                code: "CLIPBOARD_FAILED".to_owned(),
            })),
        }
    }
}

/// The availability reason is owned by the backend and short/statically
/// known in practice; map unknown dynamic reasons onto one stable code.
fn leak_reason(reason: String) -> &'static str {
    match reason.as_str() {
        "no-clipboard-tool-configured" => "no-clipboard-tool-configured",
        "spawn-failed" => "clipboard_tool_spawn_failed",
        "probe-timeout" => "clipboard_tool_probe_timeout",
        _ => "clipboard_tool_unavailable",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use narrivy_types::{DeliveryOutcome, FallbackReason, SessionCancellation};

    use super::*;
    use crate::delivery::inbox::InboxStore;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// PlatformError is intentionally not a std::error::Error; tests convert it
    /// explicitly so a failure message stays content-free.
    fn platform<T>(result: Result<T, PlatformError>) -> Result<T, std::io::Error> {
        result.map_err(|error| std::io::Error::other(format!("platform error {}", error.code)))
    }

    fn private_tempdir() -> std::io::Result<tempfile::TempDir> {
        let dir = tempfile::tempdir()?;
        std::fs::set_permissions(
            dir.path(),
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )?;
        Ok(dir)
    }

    struct FixedPermission(DeliveryPermissionEvidence);
    impl DeliveryPermissionPort for FixedPermission {
        fn evidence(&mut self) -> DeliveryPermissionEvidence {
            self.0
        }
    }

    struct ScriptedRung {
        id: &'static str,
        requires_target: bool,
        qualification: RungQualification,
        outcome: RungOutcome,
        attempts: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl DirectRung for ScriptedRung {
        fn id(&self) -> &'static str {
            self.id
        }

        fn requires_target(&self) -> bool {
            self.requires_target
        }

        fn qualification(
            &mut self,
            _target: &TargetBinding,
            _permission: &DeliveryPermissionEvidence,
            _cancellation: &SessionCancellation,
        ) -> RungQualification {
            self.qualification
        }

        fn attempt(&mut self, _text: &str, _cancellation: &SessionCancellation) -> RungOutcome {
            self.attempts
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.outcome.clone()
        }
    }

    fn rung(
        id: &'static str,
        qualification: RungQualification,
        outcome: RungOutcome,
    ) -> (
        Box<dyn DirectRung>,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) {
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        (
            Box::new(ScriptedRung {
                id,
                requires_target: false,
                qualification,
                outcome,
                attempts: std::sync::Arc::clone(&attempts),
            }),
            attempts,
        )
    }

    fn attempt_pairs(report: &DeliveryReport) -> Vec<(&str, DeliveryOutcome)> {
        report
            .attempts
            .iter()
            .map(|attempt| (attempt.backend.as_str(), attempt.outcome.clone()))
            .collect()
    }

    fn bound_target() -> TargetBinding {
        TargetBinding::Bound {
            application: "controlled-app".to_owned(),
            window: "0x1".to_owned(),
        }
    }

    fn unavailable_target() -> TargetBinding {
        TargetBinding::Unavailable(PRE_ACTIVATION_BINDING_ABSENT)
    }

    fn intent(txid: &str) -> DeliveryIntent {
        DeliveryIntent {
            transaction_id: TransactionId::new(txid),
            backend: ROUTED_THEN_INBOX_BACKEND.to_owned(),
        }
    }

    #[test]
    fn routed_ladder_falls_back_to_durable_inbox_when_nothing_qualifies() -> TestResult {
        let dir = private_tempdir()?;
        let inbox = dir.path().join("inbox");
        let (first, first_attempts) = rung(
            "portal-eis-text",
            RungQualification::Unqualified("permission_inactive"),
            RungOutcome::SubmittedUnverified,
        );
        let (second, second_attempts) = rung(
            "wtype-virtual-keyboard",
            RungQualification::Unqualified("wayland_session_absent"),
            RungOutcome::SubmittedUnverified,
        );
        let mut delivery = platform(RoutedThenInboxDelivery::new(
            inbox.clone(),
            unavailable_target(),
            Box::new(FixedPermission(DeliveryPermissionEvidence::default())),
            vec![first, second],
        ))?;
        let intent = intent("tx-routed-fallback");
        let report =
            platform(delivery.deliver(&intent, "Grüße\nsecond line", &SessionCancellation::new()))?;
        assert_eq!(report.outcome, DeliveryOutcome::InboxFallback);
        assert_eq!(report.backend, ROUTED_THEN_INBOX_BACKEND);
        assert_eq!(report.attempts.len(), 3);
        assert_eq!(
            report.attempts[0].fallback_reason,
            Some(FallbackReason::BackendError {
                code: "permission_inactive".to_owned()
            })
        );
        assert_eq!(report.attempts[2].backend, "inbox");
        assert_eq!(report.attempts[2].outcome, DeliveryOutcome::InboxFallback);
        assert_eq!(first_attempts.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(second_attempts.load(std::sync::atomic::Ordering::SeqCst), 0);
        let stored = InboxStore::new(inbox).read_text(&intent.transaction_id)?;
        assert_eq!(stored, "Grüße\nsecond line", "Unicode/multiline preserved");
        Ok(())
    }

    #[test]
    fn routed_ladder_verified_commit_ends_without_inbox_write() -> TestResult {
        let dir = private_tempdir()?;
        let inbox = dir.path().join("inbox");
        let (first, first_attempts) = rung(
            "portal-eis-text",
            RungQualification::Qualified,
            RungOutcome::Committed,
        );
        let (second, second_attempts) = rung(
            "wtype-virtual-keyboard",
            RungQualification::Qualified,
            RungOutcome::SubmittedUnverified,
        );
        let mut delivery = platform(RoutedThenInboxDelivery::new(
            inbox.clone(),
            bound_target(),
            Box::new(FixedPermission(DeliveryPermissionEvidence {
                keyboard_insert: true,
                clipboard_restore: false,
            })),
            vec![first, second],
        ))?;
        let intent = intent("tx-routed-verified");
        let report = platform(delivery.deliver(&intent, "verified", &SessionCancellation::new()))?;
        assert_eq!(report.outcome, DeliveryOutcome::Delivered);
        assert_eq!(report.attempts.len(), 1);
        assert_eq!(report.attempts[0].outcome, DeliveryOutcome::Delivered);
        assert_eq!(first_attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            second_attempts.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "a verified commit must end the ladder"
        );
        assert!(
            !inbox.join("tx-routed-verified.txt").exists(),
            "verified commit needs no recovery artifact"
        );
        Ok(())
    }

    #[test]
    fn routed_ladder_stops_after_unverified_submission() -> TestResult {
        let dir = private_tempdir()?;
        let inbox = dir.path().join("inbox");
        let (paste, paste_attempts) = rung(
            "clipboard-paste",
            RungQualification::Qualified,
            RungOutcome::SubmittedUnverified,
        );
        let (typing, typing_attempts) = rung(
            "wtype-virtual-keyboard",
            RungQualification::Qualified,
            RungOutcome::SubmittedUnverified,
        );
        let mut delivery = platform(RoutedThenInboxDelivery::new(
            inbox.clone(),
            unavailable_target(),
            Box::new(FixedPermission(DeliveryPermissionEvidence {
                keyboard_insert: true,
                clipboard_restore: true,
            })),
            vec![paste, typing],
        ))?;
        let intent = intent("tx-routed-once");
        let report = platform(delivery.deliver(&intent, "only once", &SessionCancellation::new()))?;
        assert_eq!(paste_attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            typing_attempts.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "a later rung would insert the same text a second time"
        );
        assert_eq!(report.outcome, DeliveryOutcome::SubmittedUnverified);
        assert_eq!(report.failure_reason, None);
        assert_eq!(
            attempt_pairs(&report),
            vec![("clipboard-paste", DeliveryOutcome::SubmittedUnverified)]
        );
        assert!(
            !inbox.join("tx-routed-once.txt").exists(),
            "a successful submission leaves no plaintext Inbox copy (F-M4)"
        );
        Ok(())
    }

    #[test]
    fn routed_ladder_stops_after_dispatch_failure() -> TestResult {
        let dir = private_tempdir()?;
        let inbox = dir.path().join("inbox");
        let (first, first_attempts) = rung(
            "portal-eis-text",
            RungQualification::Qualified,
            RungOutcome::Failed(FallbackReason::Timeout),
        );
        let (second, second_attempts) = rung(
            "wtype-virtual-keyboard",
            RungQualification::Qualified,
            RungOutcome::SubmittedUnverified,
        );
        let mut delivery = platform(RoutedThenInboxDelivery::new(
            inbox.clone(),
            bound_target(),
            Box::new(FixedPermission(DeliveryPermissionEvidence {
                keyboard_insert: true,
                clipboard_restore: false,
            })),
            vec![first, second],
        ))?;
        let intent = intent("tx-routed-failures");
        let report =
            platform(delivery.deliver(&intent, "recover me", &SessionCancellation::new()))?;
        assert_eq!(report.outcome, DeliveryOutcome::InboxFallback);
        assert_eq!(first_attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            second_attempts.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "a failure that may have dispatched ends the ladder"
        );
        assert_eq!(
            attempt_pairs(&report),
            vec![
                ("portal-eis-text", DeliveryOutcome::Failed),
                ("inbox", DeliveryOutcome::InboxFallback),
            ]
        );
        assert_eq!(
            report.attempts[0].fallback_reason,
            Some(FallbackReason::Timeout)
        );
        assert_eq!(
            InboxStore::new(inbox).read_text(&intent.transaction_id)?,
            "recover me"
        );
        Ok(())
    }

    #[test]
    fn routed_ladder_continues_after_a_failure_before_dispatch() -> TestResult {
        let dir = private_tempdir()?;
        let inbox = dir.path().join("inbox");
        let (paste, paste_attempts) = rung(
            "clipboard-paste",
            RungQualification::Qualified,
            RungOutcome::NotDispatched(FallbackReason::BackendError {
                code: "clipboard_too_large".to_owned(),
            }),
        );
        let (typing, typing_attempts) = rung(
            "wtype-virtual-keyboard",
            RungQualification::Qualified,
            RungOutcome::SubmittedUnverified,
        );
        let (revoked, revoked_attempts) = rung(
            "clipboard-copy",
            RungQualification::Unqualified("permission_revoked"),
            RungOutcome::CopiedOnly,
        );
        let mut delivery = platform(RoutedThenInboxDelivery::new(
            inbox,
            unavailable_target(),
            Box::new(FixedPermission(DeliveryPermissionEvidence {
                keyboard_insert: true,
                clipboard_restore: true,
            })),
            vec![paste, typing, revoked],
        ))?;
        let report = platform(delivery.deliver(
            &intent("tx-routed-continue"),
            "typed instead",
            &SessionCancellation::new(),
        ))?;
        assert_eq!(paste_attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(typing_attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            revoked_attempts.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert_eq!(
            attempt_pairs(&report),
            vec![
                ("clipboard-paste", DeliveryOutcome::Failed),
                (
                    "wtype-virtual-keyboard",
                    DeliveryOutcome::SubmittedUnverified
                ),
            ]
        );
        assert_eq!(report.outcome, DeliveryOutcome::SubmittedUnverified);
        Ok(())
    }

    #[test]
    fn routed_ladder_cancellation_and_stale_retry_never_double_dispatch() -> TestResult {
        let dir = private_tempdir()?;
        let inbox = dir.path().join("inbox");
        let (first, first_attempts) = rung(
            "wtype-virtual-keyboard",
            RungQualification::Qualified,
            RungOutcome::SubmittedUnverified,
        );
        let mut delivery = platform(RoutedThenInboxDelivery::new(
            inbox.clone(),
            bound_target(),
            Box::new(FixedPermission(DeliveryPermissionEvidence::default())),
            vec![first],
        ))?;
        let retry_intent = intent("tx-routed-retry");
        let first_report =
            platform(delivery.deliver(&retry_intent, "once only", &SessionCancellation::new()))?;
        assert_eq!(first_report.outcome, DeliveryOutcome::SubmittedUnverified);
        assert_eq!(first_attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
        let retried =
            platform(delivery.deliver(&retry_intent, "once only", &SessionCancellation::new()))?;
        assert_eq!(
            retried, first_report,
            "a stale retry re-observes the same facts"
        );
        assert_eq!(
            first_attempts.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a stale retry must never re-dispatch insertion"
        );
        assert!(
            !inbox.join("tx-routed-retry.txt").exists(),
            "a successful submission leaves no plaintext Inbox copy (F-M4)"
        );

        // Cancellation before any dispatch is a clean terminal cancellation:
        // no report and no recovery artifact for the cancelled transaction.
        let mut fresh = platform(RoutedThenInboxDelivery::new(
            inbox.clone(),
            bound_target(),
            Box::new(FixedPermission(DeliveryPermissionEvidence::default())),
            Vec::new(),
        ))?;
        let cancelled_intent = intent("tx-routed-cancelled");
        let cancelled = SessionCancellation::new();
        cancelled.cancel();
        let err = match fresh.deliver(&cancelled_intent, "never", &cancelled) {
            Ok(report) => {
                return Err(std::io::Error::other(format!(
                    "cancelled delivery must not produce a report: {report:?}"
                ))
                .into());
            }
            Err(err) => err,
        };
        assert_eq!(err.code, "cancelled");
        assert!(
            !inbox.join("tx-routed-cancelled.txt").exists(),
            "cancellation before dispatch must not create an artifact"
        );
        Ok(())
    }

    #[test]
    fn routed_rung_requires_target_before_activation() -> TestResult {
        let dir = private_tempdir()?;
        let inbox = dir.path().join("inbox");
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let rung = Box::new(ScriptedRung {
            id: "portal-eis-text",
            requires_target: true,
            qualification: RungQualification::Qualified,
            outcome: RungOutcome::SubmittedUnverified,
            attempts: std::sync::Arc::clone(&attempts),
        });
        let mut delivery = platform(RoutedThenInboxDelivery::new(
            inbox,
            unavailable_target(),
            Box::new(FixedPermission(DeliveryPermissionEvidence {
                keyboard_insert: true,
                clipboard_restore: false,
            })),
            vec![rung],
        ))?;
        let report = platform(delivery.deliver(
            &intent("tx-routed-no-target"),
            "no target",
            &SessionCancellation::new(),
        ))?;
        assert_eq!(report.outcome, DeliveryOutcome::InboxFallback);
        assert_eq!(
            attempts.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "activation-requiring rungs must not run without a captured target"
        );
        assert_eq!(
            report.attempts[0].fallback_reason,
            Some(FallbackReason::NoQualifiedBackend)
        );
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Clipboard ownership/restoration policy
    // -----------------------------------------------------------------------

    struct ControlledHandoff {
        value: Option<String>,
        writes: usize,
        fail_restore: bool,
        ownership_lost_after_paste: bool,
        refuse_snapshot: Option<&'static str>,
    }

    impl ClipboardHandoff for ControlledHandoff {
        type Snapshot = String;

        fn probe(&mut self) -> Result<(), &'static str> {
            Ok(())
        }

        fn snapshot(&mut self) -> Result<Option<String>, &'static str> {
            match self.refuse_snapshot {
                Some(code) => Err(code),
                None => Ok(self.value.clone()),
            }
        }

        fn write(&mut self, text: &str) -> Result<ClipboardOwnership, &'static str> {
            self.writes += 1;
            self.value = Some(text.to_owned());
            Ok(ClipboardOwnership {
                write_serial: self.writes as u64,
            })
        }

        fn still_owned(&mut self, _ownership: &ClipboardOwnership, text: &str) -> bool {
            if self.ownership_lost_after_paste {
                return false;
            }
            self.value.as_deref() == Some(text)
        }

        fn restore(&mut self, prior: &String) -> Result<(), &'static str> {
            if self.fail_restore {
                return Err("restore_failed");
            }
            self.value = Some(prior.to_owned());
            Ok(())
        }
    }

    struct ControlledChord {
        result: Result<(), &'static str>,
        calls: usize,
    }

    impl PasteChord for ControlledChord {
        fn qualification(&self) -> RungQualification {
            RungQualification::Qualified
        }

        fn send_paste_chord(
            &mut self,
            _cancellation: &SessionCancellation,
        ) -> Result<(), &'static str> {
            self.calls += 1;
            self.result
        }
    }

    #[test]
    fn a_clipboard_that_cannot_be_captured_is_never_replaced() {
        let mut handoff = ControlledHandoff {
            value: Some("a large image".to_owned()),
            writes: 0,
            fail_restore: false,
            ownership_lost_after_paste: false,
            refuse_snapshot: Some("clipboard_too_large"),
        };
        let mut chord = ControlledChord {
            result: Ok(()),
            calls: 0,
        };
        let (outcome, restore) = paste_with_ownership_restore(
            &mut handoff,
            &mut chord,
            "dictated text",
            &SessionCancellation::new(),
        );
        assert_eq!(
            outcome,
            RungOutcome::NotDispatched(FallbackReason::BackendError {
                code: "clipboard_too_large".to_owned()
            })
        );
        assert_eq!(restore, ClipboardRestoreOutcome::NotRestorable);
        assert_eq!(handoff.writes, 0, "the user's clipboard was never replaced");
        assert_eq!(chord.calls, 0, "nothing was pasted");
        assert_eq!(handoff.value.as_deref(), Some("a large image"));
    }

    #[test]
    fn clipboard_ownership_race_preserves_newer_user_content() {
        let mut handoff = ControlledHandoff {
            value: Some("user's earlier clipboard".to_owned()),
            writes: 0,
            fail_restore: false,
            ownership_lost_after_paste: true,
            refuse_snapshot: None,
        };
        let mut chord = ControlledChord {
            result: Ok(()),
            calls: 0,
        };
        let (outcome, restore) = paste_with_ownership_restore(
            &mut handoff,
            &mut chord,
            "dictated text",
            &SessionCancellation::new(),
        );
        assert_eq!(outcome, RungOutcome::SubmittedUnverified);
        assert_eq!(restore, ClipboardRestoreOutcome::PreservedNewerContent);
        assert_eq!(
            handoff.value.as_deref(),
            Some("dictated text"),
            "the newer clipboard value must be left untouched"
        );
    }

    #[test]
    fn clipboard_restore_happens_only_while_narrivy_still_owns_the_value() {
        let mut handoff = ControlledHandoff {
            value: Some("user's earlier clipboard".to_owned()),
            writes: 0,
            fail_restore: false,
            ownership_lost_after_paste: false,
            refuse_snapshot: None,
        };
        let mut chord = ControlledChord {
            result: Ok(()),
            calls: 0,
        };
        let started = Instant::now();
        let (outcome, restore) = paste_with_ownership_restore(
            &mut handoff,
            &mut chord,
            "dictated text",
            &SessionCancellation::new(),
        );
        assert_eq!(outcome, RungOutcome::SubmittedUnverified);
        assert_eq!(restore, ClipboardRestoreOutcome::RestoredPrior);
        assert_eq!(handoff.value.as_deref(), Some("user's earlier clipboard"));
        assert_eq!(chord.calls, 1);
        assert!(
            started.elapsed() >= PASTE_MIN_HOLD,
            "the user's clipboard came back before the target could paste"
        );
    }

    /// A handoff that observes whether the target read the clipboard.
    struct ObservedHandoff {
        inner: ControlledHandoff,
        read_after_chord: bool,
    }

    impl ClipboardHandoff for ObservedHandoff {
        type Snapshot = String;

        fn probe(&mut self) -> Result<(), &'static str> {
            self.inner.probe()
        }

        fn snapshot(&mut self) -> Result<Option<String>, &'static str> {
            self.inner.snapshot()
        }

        fn write(&mut self, text: &str) -> Result<ClipboardOwnership, &'static str> {
            self.inner.write(text)
        }

        fn wait_until_consumed(
            &mut self,
            _ownership: &ClipboardOwnership,
            _bound: Duration,
        ) -> Option<bool> {
            Some(self.read_after_chord)
        }

        fn still_owned(&mut self, ownership: &ClipboardOwnership, text: &str) -> bool {
            self.inner.still_owned(ownership, text)
        }

        fn restore(&mut self, prior: &String) -> Result<(), &'static str> {
            self.inner.restore(prior)
        }
    }

    fn observed(read_after_chord: bool) -> ObservedHandoff {
        ObservedHandoff {
            inner: ControlledHandoff {
                value: Some("user's earlier clipboard".to_owned()),
                writes: 0,
                fail_restore: false,
                ownership_lost_after_paste: false,
                refuse_snapshot: None,
            },
            read_after_chord,
        }
    }

    #[test]
    fn a_paste_nothing_read_leaves_the_text_on_the_clipboard() {
        let mut handoff = observed(false);
        let mut chord = ControlledChord {
            result: Ok(()),
            calls: 0,
        };
        let (outcome, restore) = paste_with_ownership_restore(
            &mut handoff,
            &mut chord,
            "dictated text",
            &SessionCancellation::new(),
        );
        assert_eq!(outcome, RungOutcome::CopiedOnly);
        assert_eq!(restore, ClipboardRestoreOutcome::LeftForUser);
        assert_eq!(handoff.inner.value.as_deref(), Some("dictated text"));
    }

    #[test]
    fn a_paste_that_was_read_restores_the_users_clipboard() {
        let mut handoff = observed(true);
        let mut chord = ControlledChord {
            result: Ok(()),
            calls: 0,
        };
        let (outcome, restore) = paste_with_ownership_restore(
            &mut handoff,
            &mut chord,
            "dictated text",
            &SessionCancellation::new(),
        );
        assert_eq!(outcome, RungOutcome::SubmittedUnverified);
        assert_eq!(restore, ClipboardRestoreOutcome::RestoredPrior);
        assert_eq!(
            handoff.inner.value.as_deref(),
            Some("user's earlier clipboard")
        );
    }

    #[test]
    fn clipboard_restore_failure_is_bounded_and_reported() {
        let mut handoff = ControlledHandoff {
            value: Some("user's earlier clipboard".to_owned()),
            writes: 0,
            fail_restore: true,
            ownership_lost_after_paste: false,
            refuse_snapshot: None,
        };
        let mut chord = ControlledChord {
            result: Ok(()),
            calls: 0,
        };
        let (_, restore) = paste_with_ownership_restore(
            &mut handoff,
            &mut chord,
            "dictated text",
            &SessionCancellation::new(),
        );
        assert_eq!(restore, ClipboardRestoreOutcome::Failed("restore_failed"));
    }

    #[test]
    fn paste_rung_requires_authorization_and_chord_qualification() {
        let handoff = ControlledHandoff {
            value: None,
            writes: 0,
            fail_restore: false,
            ownership_lost_after_paste: false,
            refuse_snapshot: None,
        };
        let mut rung =
            ClipboardPasteRung::new(handoff, UnavailablePasteChord::new("wtype_missing"));
        let target = bound_target();
        let cancelled = SessionCancellation::new();
        assert_eq!(
            rung.qualification(&target, &DeliveryPermissionEvidence::default(), &cancelled),
            RungQualification::Unqualified("clipboard_restore_not_authorized")
        );
        assert_eq!(
            rung.qualification(
                &target,
                &DeliveryPermissionEvidence {
                    keyboard_insert: false,
                    clipboard_restore: true,
                },
                &cancelled
            ),
            RungQualification::Unqualified("wtype_missing")
        );
    }

    // -----------------------------------------------------------------------
    // Admission-frozen target binding and bounded replay record
    // -----------------------------------------------------------------------

    /// Controlled stand-in for whatever binds the target at admission. The
    /// adapter only ever sees the value returned by `bind()`; it holds no
    /// reference to this focus state.
    struct ControlledFocus(Arc<std::sync::Mutex<TargetBinding>>);

    impl TargetBindingSource for ControlledFocus {
        fn bind(&mut self) -> TargetBinding {
            self.0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        }
    }

    struct RecordingRung {
        id: &'static str,
        requires_target: bool,
        outcome: RungOutcome,
        observed: Arc<std::sync::Mutex<Option<TargetBinding>>>,
        dispatched: Arc<std::sync::Mutex<Vec<String>>>,
        cancel_after_dispatch: Option<SessionCancellation>,
    }

    impl DirectRung for RecordingRung {
        fn id(&self) -> &'static str {
            self.id
        }

        fn requires_target(&self) -> bool {
            self.requires_target
        }

        fn qualification(
            &mut self,
            target: &TargetBinding,
            _permission: &DeliveryPermissionEvidence,
            _cancellation: &SessionCancellation,
        ) -> RungQualification {
            *self
                .observed
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(target.clone());
            RungQualification::Qualified
        }

        fn attempt(&mut self, text: &str, _cancellation: &SessionCancellation) -> RungOutcome {
            self.dispatched
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(text.to_owned());
            if let Some(cancellation) = &self.cancel_after_dispatch {
                cancellation.cancel();
            }
            self.outcome.clone()
        }
    }

    type ObservedBinding = Arc<std::sync::Mutex<Option<TargetBinding>>>;
    type DispatchedTexts = Arc<std::sync::Mutex<Vec<String>>>;

    fn recording_rung(
        id: &'static str,
        requires_target: bool,
        outcome: RungOutcome,
    ) -> (Box<dyn DirectRung>, ObservedBinding, DispatchedTexts) {
        let observed = Arc::new(std::sync::Mutex::new(None));
        let dispatched = Arc::new(std::sync::Mutex::new(Vec::new()));
        (
            Box::new(RecordingRung {
                id,
                requires_target,
                outcome,
                observed: Arc::clone(&observed),
                dispatched: Arc::clone(&dispatched),
                cancel_after_dispatch: None,
            }),
            observed,
            dispatched,
        )
    }

    fn observed_binding(observed: &ObservedBinding) -> TargetBinding {
        observed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .unwrap_or(TargetBinding::Unavailable("rung-never-ran"))
    }

    fn dispatched_texts(dispatched: &DispatchedTexts) -> Vec<String> {
        dispatched
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn dispatch_pairs(report: &narrivy_types::DeliveryReport) -> Vec<(&str, DeliveryOutcome)> {
        report
            .attempts
            .iter()
            .map(|attempt| (attempt.backend.as_str(), attempt.outcome.clone()))
            .collect()
    }

    #[test]
    fn admission_bound_target_is_frozen_before_any_rung_and_ignores_later_focus() -> TestResult {
        let dir = private_tempdir()?;
        let focus = Arc::new(std::sync::Mutex::new(TargetBinding::Bound {
            application: "app-a".to_owned(),
            window: "0x1".to_owned(),
        }));
        // Admission: bind exactly once, before the session can activate.
        let admission_binding = ControlledFocus(Arc::clone(&focus)).bind();
        let (rung, observed, dispatched) =
            recording_rung("portal-eis-text", true, RungOutcome::SubmittedUnverified);
        let mut delivery = platform(RoutedThenInboxDelivery::new(
            dir.path().join("inbox"),
            admission_binding.clone(),
            Box::new(FixedPermission(DeliveryPermissionEvidence {
                keyboard_insert: true,
                clipboard_restore: false,
            })),
            vec![rung],
        ))?;

        // The user focus moves to a different window after admission. The
        // adapter must never observe this value: it has no late target query.
        *focus
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = TargetBinding::Bound {
            application: "app-b".to_owned(),
            window: "0x2".to_owned(),
        };

        let first = platform(delivery.deliver(
            &intent("tx-frozen-target-a"),
            "frozen admission text",
            &SessionCancellation::new(),
        ))?;
        assert_eq!(first.outcome, DeliveryOutcome::SubmittedUnverified);
        assert_eq!(
            observed_binding(&observed),
            admission_binding,
            "every rung must see the binding captured at admission"
        );
        assert_eq!(dispatched_texts(&dispatched), vec!["frozen admission text"]);
        assert_eq!(delivery.target_binding_class(), "bound");

        // A later delivery on the same adapter still sees the admission
        // binding, never the changed focus state.
        platform(delivery.deliver(
            &intent("tx-frozen-target-b"),
            "second frozen text",
            &SessionCancellation::new(),
        ))?;
        assert_eq!(observed_binding(&observed), admission_binding);
        assert_eq!(
            dispatched_texts(&dispatched),
            vec!["frozen admission text", "second frozen text"]
        );

        // A composition created after the change binds the new snapshot: the
        // trust is in the admission value, not in ambient focus, and the value
        // can never be replaced by later focus state on a live adapter.
        let late_admission = ControlledFocus(Arc::clone(&focus)).bind();
        assert_ne!(late_admission, admission_binding);
        let mut late = platform(RoutedThenInboxDelivery::new(
            dir.path().join("inbox"),
            late_admission.clone(),
            Box::new(FixedPermission(DeliveryPermissionEvidence::default())),
            Vec::new(),
        ))?;
        platform(late.deliver(
            &intent("tx-late-binding"),
            "late",
            &SessionCancellation::new(),
        ))?;
        assert_eq!(late.target_binding_class(), "bound");
        assert_eq!(observed_binding(&observed), admission_binding);
        Ok(())
    }

    #[test]
    fn replay_rejects_reused_transaction_with_changed_payload_before_dispatch() -> TestResult {
        let dir = private_tempdir()?;
        let inbox = dir.path().join("inbox");
        let (rung, _observed, dispatched) = recording_rung(
            "wtype-virtual-keyboard",
            false,
            // A failure after dispatch keeps an Inbox copy, which the
            // rejected reuse must not overwrite.
            RungOutcome::Failed(FallbackReason::Timeout),
        );
        let mut delivery = platform(RoutedThenInboxDelivery::new(
            inbox.clone(),
            bound_target(),
            Box::new(DeniedDeliveryPermission),
            vec![rung],
        ))?;
        let txid = intent("tx-replay-payload");
        let first =
            platform(delivery.deliver(&txid, "first payload", &SessionCancellation::new()))?;
        assert_eq!(first.outcome, DeliveryOutcome::InboxFallback);

        let error = match delivery.deliver(&txid, "different payload", &SessionCancellation::new())
        {
            Ok(report) => {
                return Err(
                    std::io::Error::other(format!("changed payload accepted: {report:?}")).into(),
                );
            }
            Err(error) => error,
        };
        assert_eq!(error.code, "delivery_transaction_reuse_rejected");
        assert_eq!(
            dispatched_texts(&dispatched),
            vec!["first payload"],
            "the reuse must be rejected before any dispatch"
        );
        assert_eq!(
            InboxStore::new(inbox).read_text(&txid.transaction_id)?,
            "first payload",
            "a rejected reuse must not overwrite the recovered text"
        );
        assert_eq!(delivery.replay.entries.len(), 1);
        Ok(())
    }

    #[test]
    fn payload_fingerprint_is_sha256_and_changes_for_equal_length_payloads() -> TestResult {
        // NIST known answers: the fingerprint must be SHA-256 over the exact
        // UTF-8 bytes, not a process-local hasher output.
        const EMPTY_SHA256: [u8; 32] = [
            0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f,
            0xb9, 0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b,
            0x78, 0x52, 0xb8, 0x55,
        ];
        const ABC_SHA256: [u8; 32] = [
            0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
            0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61,
            0xf2, 0x00, 0x15, 0xad,
        ];
        assert_eq!(PayloadFingerprint::of("").digest, EMPTY_SHA256);
        assert_eq!(PayloadFingerprint::of("").length, 0);
        assert_eq!(PayloadFingerprint::of("abc").digest, ABC_SHA256);
        assert_eq!(PayloadFingerprint::of("abc").length, 3);

        // Two equal-length, similar UTF-8 payloads must never share a
        // fingerprint: the replay record could otherwise accept a reused
        // transaction id and replay the previous report for different text.
        let first = "café ☕ 0001";
        let second = "café ☕ 0002";
        assert_eq!(first.len(), second.len());
        let first_fingerprint = PayloadFingerprint::of(first);
        let second_fingerprint = PayloadFingerprint::of(second);
        assert_ne!(first_fingerprint, second_fingerprint);
        assert_eq!(first_fingerprint.length, second_fingerprint.length);
        assert_ne!(first_fingerprint.digest, second_fingerprint.digest);
        Ok(())
    }

    #[test]
    fn replay_rejects_equal_length_similar_payload_reuse_before_dispatch() -> TestResult {
        let dir = private_tempdir()?;
        let inbox = dir.path().join("inbox");
        let (rung, _observed, dispatched) = recording_rung(
            "wtype-virtual-keyboard",
            false,
            // A failure after dispatch keeps an Inbox copy, which the
            // rejected reuse must not overwrite.
            RungOutcome::Failed(FallbackReason::Timeout),
        );
        let mut delivery = platform(RoutedThenInboxDelivery::new(
            inbox.clone(),
            bound_target(),
            Box::new(DeniedDeliveryPermission),
            vec![rung],
        ))?;
        let txid = intent("tx-replay-equal-length");
        let first = "café ☕ 0001";
        let second = "café ☕ 0002";
        assert_eq!(
            first.len(),
            second.len(),
            "the regression must reuse a transaction id with an equal-length payload"
        );
        let first_report = platform(delivery.deliver(&txid, first, &SessionCancellation::new()))?;
        assert_eq!(first_report.outcome, DeliveryOutcome::InboxFallback);

        let error = match delivery.deliver(&txid, second, &SessionCancellation::new()) {
            Ok(report) => {
                return Err(std::io::Error::other(format!(
                    "equal-length changed payload accepted: {report:?}"
                ))
                .into());
            }
            Err(error) => error,
        };
        assert_eq!(error.code, "delivery_transaction_reuse_rejected");
        assert_eq!(
            dispatched_texts(&dispatched),
            vec![first],
            "the equal-length reuse must be rejected before any dispatch"
        );
        assert_eq!(
            InboxStore::new(inbox).read_text(&txid.transaction_id)?,
            first,
            "a rejected reuse must not overwrite the recovered text"
        );
        assert_eq!(delivery.replay.entries.len(), 1);
        Ok(())
    }

    #[test]
    fn replay_a_b_a_never_dispatches_a_twice() -> TestResult {
        let dir = private_tempdir()?;
        let inbox = dir.path().join("inbox");
        let (rung, _observed, dispatched) = recording_rung(
            "wtype-virtual-keyboard",
            false,
            RungOutcome::SubmittedUnverified,
        );
        let mut delivery = platform(RoutedThenInboxDelivery::new(
            inbox,
            bound_target(),
            Box::new(DeniedDeliveryPermission),
            vec![rung],
        ))?;
        let a = intent("tx-replay-a");
        let b = intent("tx-replay-b");
        let first_a = platform(delivery.deliver(&a, "payload a", &SessionCancellation::new()))?;
        let first_b = platform(delivery.deliver(&b, "payload b", &SessionCancellation::new()))?;
        assert_eq!(first_b.outcome, DeliveryOutcome::SubmittedUnverified);

        // The stale retry of A after B must observe A's own facts.
        let stale_a = platform(delivery.deliver(&a, "payload a", &SessionCancellation::new()))?;
        assert_eq!(stale_a, first_a);
        assert_eq!(
            stale_a.transaction_id, a.transaction_id,
            "a stale retry returns the facts of its own transaction"
        );
        assert_eq!(
            dispatched_texts(&dispatched),
            vec!["payload a", "payload b"],
            "A -> B -> stale A must dispatch each transaction exactly once"
        );
        assert_eq!(delivery.replay.entries.len(), 2);
        assert!(!delivery.replay.saturated);
        Ok(())
    }

    #[test]
    fn replay_saturation_fails_closed_and_never_redispatches() -> TestResult {
        let dir = private_tempdir()?;
        let inbox = dir.path().join("inbox");
        let (rung, _observed, dispatched) = recording_rung(
            "wtype-virtual-keyboard",
            false,
            RungOutcome::SubmittedUnverified,
        );
        let mut delivery = platform(RoutedThenInboxDelivery::with_replay_capacity(
            2,
            inbox.clone(),
            bound_target(),
            Box::new(DeniedDeliveryPermission),
            vec![rung],
        ))?;
        let a = intent("tx-saturation-a");
        let b = intent("tx-saturation-b");
        let c = intent("tx-saturation-c");
        let first_a = platform(delivery.deliver(&a, "payload a", &SessionCancellation::new()))?;
        platform(delivery.deliver(&b, "payload b", &SessionCancellation::new()))?;
        assert_eq!(
            dispatched_texts(&dispatched),
            vec!["payload a", "payload b"]
        );
        assert!(!delivery.replay.saturated);
        assert_eq!(delivery.replay.entries.len(), 2);

        // A third transaction is not admitted: no eviction, no dispatch, and
        // the exact text is still durably recovered.
        let third = platform(delivery.deliver(&c, "payload c", &SessionCancellation::new()))?;
        assert_eq!(third.outcome, DeliveryOutcome::InboxFallback);
        assert_eq!(third.attempts[0].backend, "wtype-virtual-keyboard");
        assert_eq!(
            third.attempts[0].fallback_reason,
            Some(FallbackReason::BackendError {
                code: "delivery_replay_capacity_exhausted".to_owned(),
            })
        );
        assert_eq!(
            dispatched_texts(&dispatched),
            vec!["payload a", "payload b"],
            "a saturated record must not dispatch a new transaction"
        );
        assert!(
            delivery.replay.saturated,
            "the full record latches fail-closed"
        );
        assert_eq!(delivery.replay.entries.len(), 2);
        assert_eq!(
            InboxStore::new(inbox.clone()).read_text(&c.transaction_id)?,
            "payload c"
        );

        // The saturated third transaction has no dispatch record; retrying it
        // stays fail-closed instead of becoming dispatchable.
        platform(delivery.deliver(&c, "payload c", &SessionCancellation::new()))?;
        assert_eq!(
            dispatched_texts(&dispatched),
            vec!["payload a", "payload b"]
        );

        // The earliest dispatched transaction keeps its bounded record:
        // saturation never made it dispatchable again.
        let stale_a = platform(delivery.deliver(&a, "payload a", &SessionCancellation::new()))?;
        assert_eq!(stale_a, first_a);
        assert_eq!(
            dispatched_texts(&dispatched),
            vec!["payload a", "payload b"]
        );
        assert_eq!(delivery.replay.entries.len(), 2);
        Ok(())
    }

    #[test]
    fn cancellation_after_dispatch_still_recovers_inbox_and_preserves_replay() -> TestResult {
        let dir = private_tempdir()?;
        let inbox = dir.path().join("inbox");
        let cancellation = SessionCancellation::new();
        let first_observed = Arc::new(std::sync::Mutex::new(None));
        let first_dispatched = Arc::new(std::sync::Mutex::new(Vec::new()));
        let first_rung = Box::new(RecordingRung {
            id: "portal-eis-text",
            requires_target: true,
            // A bounded failure after dispatch: the text may have left the
            // process, so the Inbox copy is mandatory.
            outcome: RungOutcome::Failed(FallbackReason::Timeout),
            observed: Arc::clone(&first_observed),
            dispatched: Arc::clone(&first_dispatched),
            cancel_after_dispatch: Some(cancellation.clone()),
        });
        let (second_rung, _second_observed, second_dispatched) = recording_rung(
            "wtype-virtual-keyboard",
            false,
            RungOutcome::SubmittedUnverified,
        );
        let mut delivery = platform(RoutedThenInboxDelivery::new(
            inbox.clone(),
            bound_target(),
            Box::new(FixedPermission(DeliveryPermissionEvidence {
                keyboard_insert: true,
                clipboard_restore: false,
            })),
            vec![first_rung, second_rung],
        ))?;

        // The first rung dispatches and the session is cancelled right after:
        // the second rung must not run, and the dispatched transaction still
        // gets its durable Inbox recovery.
        let txid = intent("tx-cancellation-boundary");
        let report = platform(delivery.deliver(&txid, "boundary text", &cancellation))?;
        assert_eq!(report.outcome, DeliveryOutcome::InboxFallback);
        assert_eq!(
            dispatch_pairs(&report),
            vec![
                ("portal-eis-text", DeliveryOutcome::Failed),
                ("inbox", DeliveryOutcome::InboxFallback),
            ]
        );
        assert!(dispatched_texts(&second_dispatched).is_empty());
        assert_eq!(
            InboxStore::new(inbox.clone()).read_text(&txid.transaction_id)?,
            "boundary text"
        );

        // A stale retry re-observes the same facts and never dispatches again.
        let retry =
            platform(delivery.deliver(&txid, "boundary text", &SessionCancellation::new()))?;
        assert_eq!(retry, report);
        assert_eq!(dispatched_texts(&first_dispatched), vec!["boundary text"]);

        // A different transaction while cancelled is a clean terminal
        // cancellation: nothing is dispatched and nothing is stored.
        let cancelled_intent = intent("tx-cancelled-after-boundary");
        let error = match delivery.deliver(&cancelled_intent, "never", &cancellation) {
            Ok(report) => {
                return Err(std::io::Error::other(format!(
                    "cancelled delivery produced a report: {report:?}"
                ))
                .into());
            }
            Err(error) => error,
        };
        assert_eq!(error.code, "cancelled");
        assert_eq!(dispatched_texts(&second_dispatched).len(), 0);
        assert!(!inbox.join("tx-cancelled-after-boundary.txt").exists());
        Ok(())
    }

    #[test]
    fn unavailable_rungs_are_reported_and_never_attempted() -> TestResult {
        let dir = private_tempdir()?;
        let inbox = dir.path().join("inbox");
        let mut delivery = platform(RoutedThenInboxDelivery::new(
            inbox.clone(),
            bound_target(),
            Box::new(FixedPermission(DeliveryPermissionEvidence {
                keyboard_insert: false,
                clipboard_restore: true,
            })),
            vec![
                Box::new(UnavailableRung::clipboard_paste(
                    "trusted_clipboard_helper_not_accepted",
                )),
                Box::new(UnavailableRung::clipboard_copy(
                    "trusted_clipboard_helper_not_accepted",
                )),
            ],
        ))?;
        assert_eq!(
            delivery.rung_ids(),
            vec!["clipboard-paste", "clipboard-copy"]
        );
        let txid = intent("tx-unavailable-rungs");
        let report = platform(delivery.deliver(
            &txid,
            "no trusted clipboard helper",
            &SessionCancellation::new(),
        ))?;
        assert_eq!(report.outcome, DeliveryOutcome::InboxFallback);
        assert_eq!(
            report.attempts[0].fallback_reason,
            Some(FallbackReason::BackendError {
                code: "trusted_clipboard_helper_not_accepted".to_owned(),
            })
        );
        assert_eq!(
            report.attempts[1].fallback_reason,
            Some(FallbackReason::BackendError {
                code: "trusted_clipboard_helper_not_accepted".to_owned(),
            })
        );
        assert_eq!(report.attempts[2].backend, "inbox");
        assert_eq!(
            InboxStore::new(inbox).read_text(&txid.transaction_id)?,
            "no trusted clipboard helper"
        );
        Ok(())
    }

    #[test]
    fn routed_composition_rejects_wrong_intent_and_unavailable_inbox() -> TestResult {
        let dir = private_tempdir()?;
        let mut delivery = platform(RoutedThenInboxDelivery::new(
            dir.path().join("inbox"),
            unavailable_target(),
            Box::new(DeniedDeliveryPermission),
            Vec::new(),
        ))?;
        let wrong = DeliveryIntent {
            transaction_id: TransactionId::new("tx-wrong"),
            backend: "wtype-then-inbox".to_owned(),
        };
        let err = match delivery.deliver(&wrong, "text", &SessionCancellation::new()) {
            Ok(report) => {
                return Err(
                    std::io::Error::other(format!("wrong intent accepted: {report:?}")).into(),
                );
            }
            Err(err) => err,
        };
        assert_eq!(err.code, "delivery_intent_invalid");

        let report = platform(delivery.deliver(
            &intent("tx-empty-ladder"),
            "no rungs",
            &SessionCancellation::new(),
        ))?;
        assert_eq!(report.outcome, DeliveryOutcome::InboxFallback);
        assert_eq!(report.attempts.len(), 1);
        assert_eq!(report.attempts[0].backend, "inbox");
        Ok(())
    }
}
