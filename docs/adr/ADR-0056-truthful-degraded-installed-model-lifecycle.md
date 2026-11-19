# ADR-0056: Truthful degraded installed-model lifecycle

Status: Accepted design for controlled Recovery 4Q-C5B1 (2026-09-03);
non-release and not model provisioning or package qualification

## Context

ADR-0052 made installed resident startup fail closed unless a fixed-root worker
manifest and an exact bundled-registry model could both be bound. ADR-0055 then
added a local offline import transaction, but deliberately exposed no product
command. Consequently a correctly installed daemon binary and worker exited
before serving IPC when the selected bundled model had not yet been imported.
That made truthful status unavailable and prevented a later authenticated
provisioning phase from having a live daemon owner.

Not every startup failure is recoverable. Invalid configuration, an unknown
bundled model ID, or an invalid installed runtime means composition authority is
absent and startup must remain fatal. Only explicit absence of the already
selected bundled model is a truthful degraded operating state. An existing but
unverifiable installation is ambiguous and remains fatal.

## Decision

The installed daemon resolves and retains the fixed-root immutable runtime
binding first. It then classifies the selected bundled model observation as one
of:

- ready: bind the immutable model descriptor and initialize the existing
  resident worker normally;
- missing: the registry-derived digest directory does not exist.

Resident ownership is one mutex-protected lifecycle state: `Unconfigured`,
`AwaitingInstalledModel`, `Ready`, or `Closing`. Initialization, status
snapshots, and shutdown removal transition or read that state atomically. In
`AwaitingInstalledModel`, the verified runtime descriptor remains owned by the
daemon so its advertised runtime fact cannot drift, but no resident worker,
fallback ASR, capture worker, or session is started. The additive optional
`GetStatus.installed_model` field reports typed `awaiting_model` or `ready`
state with lossless unknown-tag handling. Capability strings remain diagnostic
detail rather than the lifecycle API.
`ListPipelines` leaves strict pipelines disabled. `StartSession` and
`SelectPipeline` reject before journal/session/worker side effects through the
same resident-admission decision.

Invalid configuration, unknown bundled model IDs, unsafe cache-root policy,
any incomplete/corrupt/unverifiable model, model observation I/O or JSON
failure, immutable model-binding failure, and any missing or invalid fixed
installed runtime remain fatal startup errors. A valid model whose resident
process cannot initialize also remains fatal; the daemon must not relabel an
engine/runtime/filesystem failure as a provisionable model condition.

Normal daemon shutdown drops the retained degraded binding and uses the
existing IPC endpoint lease cleanup. Since degraded initialization constructs
no resident engine, it has no resident child to stop or reap.

## Trust and privacy semantics

This state proves only that the exact fixed-root runtime was verified during
this daemon lifetime and that the selected bundled model was explicitly absent.
Diagnostics expose stable reason codes, never paths, payloads,
model bytes, or transcript content. No model is downloaded, imported, repaired,
deleted, or replaced.

The state does not prove package membership, installation correctness,
provenance, SBOM identity, service-manager lifecycle, model licensing, native
transcription, delivery, or release readiness.

## Consequences

- Installed mode can remain reachable and truthful before model provisioning.
- Recovery remains fail closed: there is no mock, sidecar, alternate model, or
  raw-path fallback.
- The future C5B2 provisioning command must be a separate authenticated,
  same-user, daemon-owned mutation with serialization and post-import resident
  initialization. C5B1 adds no such command or client surface.
- Native installed-service startup/shutdown and socket cleanup still require a
  separately authorized disposable installation environment.
