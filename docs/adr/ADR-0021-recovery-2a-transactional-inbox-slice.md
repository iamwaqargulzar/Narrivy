# ADR-0021 — Recovery 2A transactional Inbox slice

**Status:** Accepted for Recovery Phase 2A (2026-08-26)

## Context

The accepted Phase 1 recovery work deliberately left the release blocked. A
single native Linux microphone-to-Inbox path now needs a portable orchestration
contract without treating mock/session scaffolding, focused-app delivery, or a
release package as proof of the real configuration.

The earlier APIs had separate cancellation flags in core and ASR, and the
recovery journal did not persist either a declared delivery intent or the
actual delivery adapter report. That could leave a pending record on capture or
ASR initialization failure and did not make transaction identity mechanically
checkable across the Inbox artifact.

## Decision

- `SessionCancellation` in `narrivy-types` is the transaction-wide
  cooperative signal. Core worker, Linux capture, transcribe.cpp sidecar,
  correction, and Inbox delivery consume the same value. A recovery worker is
  registered with that same signal, so user cancellation and supervisor
  shutdown cannot diverge; completed workers remove their supervisor entry
  and daemon shutdown invokes bounded `shutdown_all` before exit.
- `narrivy-platform-api` owns portable `AudioCapturePort` and `DeliveryPort`.
  Linux owns `pw-record` execution, WAV validation, atomic Inbox persistence,
  and bounded child termination; core never invokes an OS command.
- `narrivy-asr-api` exposes `OneShotAsrPort` for a bounded final-audio
  transaction. The transcribe.cpp engine owns explicit CPU argv, process
  timeout, cancellation, and temporary sensitive-file cleanup. Capture and
  sidecar work directories use exclusive random creation below a checked
  non-symlink parent, are mode 0700, and contain mode-0600 audio/transcript
  files. Inbox and journal use exclusive mode-0600 temporary files, atomic
  rename, directory fsync, and RAII cleanup on all ordinary exit paths.
- `narrivy-policy::ProcessingPolicy` is consulted by core before each declared
  local processing boundary: captured audio to ASR, raw transcript to
  correction, and corrected transcript to delivery. The policy remains
  deny-by-default for remote boundaries.
- The journal writes corrected text plus an `inbox` `DeliveryIntent` before
  calling delivery, then stores only the exact `DeliveryReport` returned by
  the Inbox adapter. Inbox is the configured primary destination in this
  slice, so successful delivery is `Delivered`, never `InboxFallback`.
  `Delivered` therefore means the adapter reported one Inbox artifact for the
  identical `TransactionId`; no attempt is fabricated.
- Terminal journal states distinguish `Delivered`, `Cancelled`,
  `FailedRecoverable`, and `FailedTerminal`. Capture/ASR/policy failure is
  terminal and no longer leaves an unexplained pending entry. A final journal
  write failure after primary delivery is explicitly recoverable uncertainty;
  the prior committed intent remains rather than inventing a delivered row.
  Raw-transcript policy is checked before raw text reaches the journal.

## Consequences

- The strict daemon composition is opt-in as
  `NARRIVY_RECOVERY_MODE=linux-mic-inbox`, with PipeWire, transcribe.cpp, an
  explicit model path, and Inbox-only delivery all required. Missing runtime
  pieces are unavailable; mock, null, fixture, router, clipboard, wtype, and
  uinput are not fallbacks in that mode.
- This ADR does not alter the canonical P2 acceptance contract, enable a
  global shortcut, validate GNOME, package software, qualify a requirement,
  or make a release claim.
- The existing legacy/mock path remains for prior controlled tests only. It
  is not selected by the strict recovery pipeline.
- Strict recovery events carry `linux-mic-inbox` directly. Strict mode lists
  only that pipeline and marks it unavailable until readiness succeeds; the
  CLI forwards no global recovery default to a normal daemon. Readiness
  distinguishes missing/invalid/unsupported GGUF, missing/non-executable
  sidecar, and missing/non-executable PipeWire recorder; a filename alone
  never establishes model identity.

## Alternatives considered

1. Keep separate core and ASR cancellation flags. Rejected: an ASR decode
   could outlive a cancelled capture/delivery transaction.
2. Reuse the Linux delivery router and force its Inbox rung. Rejected: that
   still probes or permits focused-app/clipboard routes that Recovery 2A must
   not exercise.
3. Persist final text after delivery. Rejected: a delivery crash would lose
   recoverable text and weaken truthful recovery semantics.

## Verification

Controlled tests cover policy-before-ASR denial, successful transaction/report
identity, cancellation before delivery, Inbox atomicity, missing runtime/model
readiness, and typed terminal journal states. A separate content-free baseline
records any actual native microphone run; it is not schema-v2 evidence.
