# ADR-0072: Admission-bound target routed delivery composition with durable Inbox recovery

- Status: accepted for source composition; installed default unchanged; native target-app qualification pending
- Date: 2026-09-14 (revised 2026-09-15 after review)
- Phase: P11 recovery, release task 009
- Numbering: this isolated draft previously carried the number ADR-0071, which
  collides with the accepted capture ADR-0071 in the main workspace. It is
  renumbered to ADR-0072; the main ADR-0071 is not touched.

## Context

ADR-0070 made `raw` and `clean` the pipeline identities of the strict installed
composition and demoted `linux-mic-inbox`/`linux-mic-wtype-inbox` to process
composition lanes. After it, the installed strict composition still had exactly
two delivery behaviors:

- `linux-mic-inbox` + `NARRIVY_DELIVERY_BACKEND=inbox`: durable atomic Inbox
  write only; no target, clipboard, wtype or portal route is considered.
- `linux-mic-wtype-inbox` + `NARRIVY_DELIVERY_BACKEND=wtype-inbox`: one
  unverified stdin-only `wtype` attempt followed by mandatory Inbox recovery.

Separately, the building blocks for a broader composition were already reviewed
and tested in isolation, but nothing composed them into a production process:

- `narrivy_platform_linux::delivery::RoutedThenInboxDelivery` (rung
  qualification, commit-once, ownership-aware clipboard restore, mandatory
  durable Inbox recovery);
- the portable routed delivery-mode identity (`routed-then-inbox`) and its
  report-shape validation (verified commit versus attempted/unverified recovery,
  no intermediate `Delivered`);
- the daemon-owned single-flight portal permission service and its
  `PortalPermissionSubmitter` delivery handle (ADR-0066), whose status
  deliberately keeps `sender_available=false`/`delivery_qualified=false` until
  native behavior is qualified.

The first task009 landing composed that adapter with a *late* target query
(`SystemTargetIdentity`, an `xprop` probe executed immediately before delivery),
composed PATH-resolved clipboard helpers, and migrated the packaged unit to the
routed lane. Review rejected that shape: the target context must be bound to the
session before activation/focus can change, the PATH-resolved helpers were not
contained to release standard, the `committed` cache only protected an immediate
retry, and the packaged default could not be migrated on compile alone. This ADR
records the corrected, narrower contract.

## Decision

1. **One new composition lane, identity unchanged for pipelines.**
   `NARRIVY_RECOVERY_MODE=linux-mic-routed-inbox` selects owned PipeWire
   capture, the installed resident/sidecar transcribe.cpp engine and the
   target-aware routed delivery backend (`NARRIVY_DELIVERY_BACKEND=routed-inbox`).
   It is composition configuration in the same namespace as the Phase 2A/2B
   lanes: never a selectable id, never advertised as a pipeline, never journaled
   or evented as one. The selectable modes over it remain exactly `raw` and
   `clean`, and the ADR-0070 single admitted `AdmittedPipeline` (mode +
   deterministic correction + lane) is still resolved once, before any side
   effect, and consumed by admission, journal, `Listening` deferral, events,
   batch/streaming orchestration and the owned worker.

2. **The delivery target is bound at admission, never queried at delivery.**
   The daemon binds the target exactly once on the Start/admission path, before
   any capture/activation/focus side effect, and carries the immutable,
   content-free `TargetBinding` into the owned delivery worker.
   `RoutedThenInboxDelivery` consumes that value and performs **no** target query
   in `deliver`; a later focus change can therefore never redirect a rung. The
   current IPC `StartSession` command carries no trustworthy pre-activation
   target identity (its `activation` string is a self-declared reason, not a
   window identity) and no accepted resolver supplies one, so production binds
   the explicit typed-unavailable input
   (`pre_activation_target_binding_absent`). Every target-requiring rung is then
   skipped, the exact corrected text goes durably to the Inbox, and this is
   reported as the remaining live-trigger integration blocker rather than faked
   with a late probe, a desktop name or the activation string.

3. **Only contained mechanism rungs are composed.** The routed lane composes:
   - the portal/EIS text rung only through the daemon-owned
     `PortalPermissionSubmitter`, gated on an already-active attended grant with
     a connected transport; the daemon never requests permission from delivery
     and the rung is always reported `SubmittedUnverified`;
   - the trusted `SystemWtypeSubmitter` virtual-keyboard rung, gated on helper
     trust/presence and a real Wayland session environment (no desktop/OS name is
     used as a feature flag, and an attempt is never `Delivered`).
   The rejected `SystemTargetIdentity`/`run_captured` helpers are removed: they
   spawned PATH-resolved `xprop`, lacked the ADR-0048 fixed-directory identity
   resolver, scrubbed environment, process group, parent-death behavior and
   descendant cleanup, and `run_captured` could join a pipe reader forever when a
   descendant retained stdout. The `SystemClipboardHandoff`,
   `SystemWtypePasteChord` and `ClipboardCopyRung::system` compositions are
   removed from production for the same reason (`wl-copy`/`wl-paste`/`xclip`/paste
   chord over the legacy `StdCommandRunner` and PATH resolution): no
   trusted-helper implementation is accepted for them, so the installed lane
   *reports* `clipboard-paste`/`clipboard-copy` unavailable with a stable reason
   and can never dispatch them. Controlled pure ports
   (`ClipboardHandoff`/`PasteChord`/`ClipboardPasteRung`/`paste_with_ownership_restore`)
   and their tests remain.
   Exactly one atomic durable Inbox recovery write under the same transaction
   identity follows every outcome that is not a verified insertion.

4. **Delivery-mode identity is explicit and new.** The routed lane persists
   `RecoveryDeliveryMode::TargetRoutedThenInboxRecovery`, whose intent backend is
   `routed-then-inbox`. Verified insertion and durable recovery stay distinct
   terminal facts (`Delivered` versus `InboxRecovered`); a task that only spawned
   a helper can never become a delivery claim, and the portable tail rejects a
   malformed or double-commit report before a terminal journal fact.

5. **Fail closed when the composition is incomplete.** Selecting
   `NARRIVY_DELIVERY_BACKEND=routed-inbox` without the routed lane is a hard
   startup error (`routed_inbox_requires_routed_lane`); only one strict lane may
   be configured; the routed lane still requires PipeWire, transcribe.cpp, a
   bound model and no forced delivery failure. `linux-mic-inbox` keeps exactly
   its Inbox-only meaning and `linux-mic-wtype-inbox` keeps exactly its
   single-unverified-wtype meaning; neither was rewritten.

6. **Commit-once is a bounded transaction-keyed replay record.** Each dispatched
   transaction is recorded with a SHA-256 content digest plus byte length and the
   content-free report — never transcript plaintext. A stale retry of a recorded
   transaction replays the observed facts; a reused transaction id with a
   different payload is rejected before any dispatch or Inbox overwrite; and a
   full record latches fail-closed instead of evicting a dispatched entry, so an
   old transaction cannot become dispatchable again during the adapter lifetime
   (new transactions then recover durably to the Inbox without a direct
   dispatch).

7. **Capability advertising is evidence-based.** The routed lane publishes
   `delivery.route=target-aware`, `delivery.verified=unavailable`,
   `delivery.inbox-recovery=enabled`,
   `delivery.target-binding=unavailable:pre_activation_target_binding_absent`,
   the real helper status under `delivery.wtype-helper=`,
   `delivery.wtype-submission=unverified`,
   `delivery.backend.portal-eis=unverified:permission-gated` and clipboard
   tokens (`delivery.backend.clipboard`, `delivery.clipboard-restore`,
   `delivery.rung.clipboard-*`) that are
   `unavailable:clipboard_not_authorized` or
   `unavailable:trusted_clipboard_helper_not_accepted` — never an available,
   verified or ownership-aware clipboard mechanism. It never publishes a
   verified-delivery token and never publishes a lane id as a pipeline token.
   The frontend renders those tokens (no delivery policy), names the missing
   pre-activation target binding, and keeps the existing permission guidance and
   recovery Inbox UI.

8. **The packaged default is not migrated.** The packaged user unit, recovery
   candidate unit and the builder/qualification helpers keep the accepted
   `linux-mic-inbox` + `inbox` default (and continue to unset
   `NARRIVY_CLIPBOARD_AUTHORIZED`). The routed lane stays explicit and selectable
   by configuration for controlled/native qualification. Switching the installed
   default requires the real pre-activation target binding and at least one
   contained external rung to be composed and independently qualified.

## Contract surface

This change set adds/modifies:

- `narrivy_platform_linux::delivery`:
  `TargetBinding`/`TargetBindingSource`/`UnavailablePreActivationTargetBinding`
  (admission binding), `UnavailableRung`, `DeliveryReplayCache`/`ReplayCheck`
  (bounded replay), `RoutedThenInboxDelivery` (binding by value) — and removes
  `SystemTargetIdentity`, `TargetIdentityPort`, `run_captured`,
  `SystemClipboardHandoff`, `SystemWtypePasteChord` and
  `ClipboardCopyRung::system`;
- `narrivyd` daemon composition (`compose_strict_routed_delivery[_with]`,
  `admission_target_binding`, Start-path binding capture, capability tokens);
- packaging/qualification helper environment mirrors (retained Inbox default).

All of these are additive or corrective to contracts that
`docs/architecture/api-stability.md` still classifies as Phase-0 scaffolds
(typed delivery outcomes are an explicit P1–P5 redesign area).
`RecoveryDeliveryMode` is not serialized; the journal persists
`DeliveryIntent.backend` as a string and keeps its existing shape, so a journal
written by an older daemon stays readable and no IPC field, protocol minor bump
or requirement promotion is involved. No portable core crate gains OS-specific
policy.

## Alternatives considered

- **Keep the late `xprop` target query.** Rejected: it can identify the wrong
  window after the user's focus has moved, and it violates the repository
  state-machine contract that target context is bound before activation can
  change. The correct behaviour without a trustworthy binding is a truthful
  Inbox recovery, not a guess.
- **Fake a native target from the activation string or a desktop name.**
  Rejected: an activation reason is not a window identity and OS names are not
  feature flags; inventing a target would be fabricated qualification.
- **Patch `run_captured` with a reader sleep.** Rejected: it would preserve an
  unbounded join behind a false bound instead of removing the uncontained
  helper.
- **Compose the legacy clipboard helpers behind explicit authorization.**
  Rejected: PATH-resolved `wl-copy`/`wl-paste`/`xclip`/paste chord are not
  contained to release standard; they stay unavailable until a separate trusted
  implementation is accepted.
- **Leave the `committed` single-slot cache.** Rejected: A -> B -> stale A would
  dispatch A twice.
- **Evict the oldest replay entry when full.** Rejected: eviction would make an
  old dispatched transaction dispatchable again; the record fails closed
  instead.
- **Migrate the packaged default because the adapter compiles.** Rejected: the
  installed default changes only when the real pre-activation binding and at
  least one contained external rung exist and are qualified.

## Consequences

- The installed strict composition reaches the approved delivery router with the
  exact admitted mode and transaction identity, and every non-verified outcome
  leaves the final corrected text durably recoverable in the Inbox.
- With no admission binding, target-requiring rungs (portal/EIS, clipboard
  paste) are skipped even under an active grant; the permitted trusted `wtype`
  rung may still submit unverified when its helper is qualified.
- Cancellation before dispatch is a clean terminal cancellation; a stale retry
  of any recorded transaction returns the observed facts and never inserts or
  stores twice, across A -> B -> stale A and across a full replay record.
- `narrivy-core` keeps no OS-specific policy; `narrivyd` composes the adapter
  ports; the frontend only renders advertised evidence.
- Ordinary diagnostics, capability tokens and attempt records remain
  content-free (no target identity, clipboard, transcript, digest or context
  content); the replay record is bounded and holds only id + SHA-256 digest/byte
  length + content-free report.
- **Not qualified:** no live pre-activation target provider is wired into Start,
  no native COSMIC/GNOME target-application matrix, no attended portal/EIS
  insertion on a real desktop, no contained clipboard implementation, and no
  installed-package end-to-end delivery run happened here. The controlled
  composition regressions prove the composition, fail-closed behavior, durable
  fallback, frozen binding, replay and cancellation-boundary rules on injected OS
  boundaries; they are not native delivery qualification and no external support
  is claimed. The packaged default deliberately remains Inbox-only.
- Integration seams for parallel work (task007 capture evolution): the daemon
  changes are confined to `services/narrivyd/src/daemon/policy.rs` (lane
  enum/config/validation/capability tokens), `services/narrivyd/src/daemon/session.rs`
  (`admission_target_binding`, `compose_strict_routed_delivery[_with]`,
  `SessionDeliveryPermission`, worker lane match) and the adapter
  `adapters/platform-linux/src/delivery/routed.rs`; the packaging change is
  limited to keeping the accepted default (lane/backend pair) in the three
  service units and their two mirroring scripts.

## Amendment, 2026-10-01: clipboard paste through data-control (D29)

With `NARRIVY_CLIPBOARD_AUTHORIZED=1`, the ladder now composes a
clipboard-paste rung, ahead of the virtual-keyboard rung:

- **The clipboard** is reached directly through the Wayland
  `ext-data-control-v1` protocol (`adapters/platform-linux/src/wayland_selection.rs`).
  No helper is resolved from `PATH`.
- **The paste chord** is Shift+Insert, sent through the same system-owned,
  revalidated `wtype` the typing rung uses, given only key names.
- **The user's clipboard is protected:**
  - both the clipboard and the primary selection are captured whole (every
    format, bounded);
  - after the paste, the user's values are put back only on a selection
    Narrivy still owns;
  - a clipboard that cannot be captured whole is never replaced, and the
    ladder moves on to typing.
- **No bound target is needed:** like the typing rung, the paste goes to the
  focused window and changes no focus or activation.
- **Reporting is unchanged in strength:**
  - the rung's outcome is `SubmittedUnverified`;
  - the Inbox recovery write still follows;
  - the tokens read `unverified:wayland-data-control`, never verified.
- **Copy-only** remains `unavailable:trusted_clipboard_helper_not_accepted`.

## Amendment, 2026-10-02: at most one rung dispatches per transaction (T2.1)

- **At most one rung may dispatch per transaction.** A rung that reports
  `SubmittedUnverified`, `CopiedOnly`, or a failure after the payload may have
  left the process ends the ladder; only the Inbox recovery write follows.
  Without this, a paste followed by typing inserted the text twice.
- **A failure before dispatch** (`NotDispatched`: the clipboard could not be
  captured, the portal rejected the frame before sending, no submitter) lets
  the ladder continue, as a policy-denied payload already did.
- **The delivery result** is published on the `committed` state event
  (protocol minor 18), derived from the journaled attempts.
