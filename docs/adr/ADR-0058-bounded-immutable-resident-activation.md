# ADR-0058: Bounded immutable resident activation

Status: Accepted design for controlled Recovery 4Q-C5B2B0 (2026-09-03);
non-release and not an IPC provisioning surface

## Context

ADR-0057 made offline model import cooperatively cancellable and commit-aware.
The next daemon-owned provisioning step must bind the installed model into an
immutable resident snapshot and start its native worker without losing the same
absolute budget. The existing immutable data binding and initial resident
startup paths were secure at rest, but large snapshot and executable digest
passes were not consistently controlled. Initial cleanup could also extend a
deadline or hold a global ownership mutex during process waits.

These gaps would make a future provisioning command's cancellation and shutdown
claims false even if the import itself behaved correctly.

## Decision

Trust-anchored executable and model snapshots accept a shared
`SessionCancellation` and an absolute `Instant` deadline. Copying and complete
digest verification use fixed 64 KiB chunks with cancellation-priority checks
around filesystem operations and each partial write. Trust-anchor ancestry is
checked component by component under the same control.

The executable is hashed once while its sealed snapshot is created. Controlled
command construction validates only the immutable descriptor structure; the
single complete launch-time digest pass occurs at `spawn_with_control`, followed
by a final control check immediately before process creation. This preserves a
late integrity check without duplicate full hashes.

Initial resident startup receives one caller-owned absolute cleanup deadline.
It reserves the existing two-second termination allowance inside that deadline,
using the earlier operation deadline for process creation and handshake. Every
startup failure uses the original cleanup deadline to kill and reap the child;
if certainty cannot be reached, ownership is retained and a typed lifecycle
failure is returned. Cancellation and timeout discovered in executable handling
map to the resident startup's existing typed control outcomes.

The global initial-retirement state is a short critical section only. One owner
atomically transfers the retained queue and marks cleanup in progress, releases
the mutex, performs process waits, then merges any unresolved ownership back
before clearing the marker. Contending constructors poll that marker under
their own cancellation and deadline instead of blocking behind another caller's
longer cleanup budget.

## Security and privacy

The APIs accept no caller-provided callbacks in production. Errors contain
stable categories and no executable path, model path, digest, audio, or
transcript content. Immutable memfd sealing, exact digest/size binding,
descriptor-only model inheritance, scrubbed environment, process-group
ownership, parent-death hardening, and descendant cleanup remain mandatory.

Cancellation is cooperative around filesystem syscalls; this ADR does not claim
kernel I/O preemption. A process is never created after the controlled launch
boundary observes cancellation or deadline expiry.

## Consequences

- A later daemon provisioning worker can carry one token and one absolute
  budget through import, immutable model binding, native startup, and cleanup.
- Controlled launch avoids two redundant full executable hashes while retaining
  a complete digest check at the last safe launch boundary.
- Concurrent constructor cleanup cannot silently consume another caller's
  earlier deadline, and unreaped ownership is never discarded.
- Existing unbounded compatibility wrappers remain for current callers; the
  provisioning path must use only the controlled APIs.
- This work adds no IPC command, lifecycle mutation, CLI/UI, model import,
  package, evidence, requirement, installation, or release claim.

