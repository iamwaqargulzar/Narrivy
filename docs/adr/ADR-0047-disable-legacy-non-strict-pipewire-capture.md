# ADR-0047: Disable legacy non-strict PipeWire capture

- Status: Accepted for controlled Recovery 4O review
- Date: 2026-08-31
- Release effect: none; all Linux 1.0 requirements remain blocked

## Context

The daemon had two unrelated implementations for microphone capture. Strict
recovery pipelines use the Linux platform adapter's owned, fileless raw-f32
capture session. Historical non-strict pipelines called a service-local helper
that wrote microphone audio to a predictable temporary WAV, could not observe
Stop or Cancel during capture, accepted any recorder exit when the WAV parsed,
and could block while reaping. Non-strict session admission also published
`Listening` before the selected source produced a frame.

Replacing only the helper call with the platform adapter would retain false
lifecycle events, misleading pipeline availability, and orchestration outside
the portable capture transaction. That is not a safe migration.

## Decision

1. Remove the daemon-local PipeWire/WAV capture helper and all re-exports.
   Production daemon capture code no longer creates a native audio path or
   payload file.
2. `AudioSourceKind::PipeWire` remains a configuration value because both
   strict recovery pipelines require it. It is invalid when neither strict
   recovery mode is selected.
3. Environment validation rejects non-strict PipeWire composition at daemon
   startup with a stable `non_strict_pipewire_disabled` configuration reason.
4. IPC pipeline selection independently rejects an in-process invalid
   configuration before journal admission, session registration, worker spawn,
   recorder spawn, or any payload side effect.
5. Pipeline listing marks every historical non-strict pipeline disabled for
   that defensive invalid configuration.
6. The worker match arm retains a last-resort typed terminal failure but never
   starts capture. It exists only to fail closed if normal selection is
   bypassed by future internal code.
7. Explicit WAV input remains a controlled fixture source. It is not native
   microphone capture and cannot satisfy native or release qualification.

## Consequences

- The insecure temporary microphone WAV path is removed rather than repaired.
- Default mock/sine and explicit fixture tests remain available.
- Strict Inbox-primary and wtype-to-Inbox recovery pipelines retain the owned
  fileless batch/streaming `PipeWireCapture` adapter without behavior changes.
- Non-strict real microphone behavior is unavailable until it is migrated as a
  complete owned capture transaction.

## Required future migration

A future non-strict live pipeline must:

- compose `AudioCapturePort`/`AudioCaptureSession` or the streaming equivalent
  through the platform boundary;
- remain `Preparing` until actual source readiness, then publish `Listening`;
- pass the session's shared Stop/Cancel control into capture;
- apply raw-audio policy before audio reaches ASR;
- own and boundedly reap every child/parser on success, failure, cancellation,
  shutdown, and panic;
- report only typed content-free failures and truthful capability state; and
- use deterministic fake-owner tests before any native observation.

## Deferred blockers

Executable identity/TOCTOU hardening for external helpers, descendant-process
containment, resolved PipeWire device identity, core-dump/swap hardening,
native calibration, package qualification, evidence, and release gates remain
separate blocked work.

## Non-goals

- microphone use or native observation;
- changing strict capture, ASR, policy, correction, or delivery behavior;
- creating evidence or changing requirement state;
- manifest resealing, packaging, commits, or pushes.
