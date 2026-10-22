# ADR-0050: Immutable resident model descriptor

Status: Accepted for controlled non-release recovery work (2026-09-01)

## Context

Recovery 4Q-A made the resident worker executable immutable, but the daemon
still validated a GGUF pathname and each worker reopened that pathname. A
rename or in-place mutation could therefore make the worker consume bytes
different from those used for readiness. The existing model-manager
`Installed` state is existence-based and does not provide a live descriptor or
package provenance, so it cannot close this boundary by itself.

## Decision

The composing service supplies an absolute model source path, exact SHA-256 and
exact byte size. `process-linux` opens the source without following a symlink,
requires a current-user-owned regular file with no set-ID or group/world write
bits, checks source identity before and after the copy, hashes while copying,
then retains one mode-0400 sealed memfd. Data snapshots are capped at 4 GiB.

For each worker generation, the process adapter duplicates that retained memfd
to a high-numbered close-on-exec descriptor. The child hardening boundary marks
all descriptors close-on-exec, then clears that flag only for the selected
model descriptor. The descriptor path is appended by the adapter; the engine
cannot substitute a pathname in its place.

Private resident protocol version 4 requires model digest, size and the
inherited `/proc/self/fd/<n>` path. The digest-bound worker rejects ordinary
paths and independently verifies descriptor shape, exact mode, required seals,
size, SHA-256 and GGUF identity before calling transcribe.cpp with strict CPU
options. A binding failure is a typed startup failure; it never selects a mock,
sidecar, alternate model or alternate backend.

Protocol version 5 retains that immutable binding and adds bounded model
capability and one-shot recognition-option frames. This wire revision does not
weaken the descriptor requirements described above.

The daemon binds and loads the model before accepting IPC. It retains both
worker and model binding state. Status, pipeline listing, selection, admission
and recovery use that stored state and never reopen the source path. Both
bindings must be controlled-digest-bound before a strict lane can be enabled.
Worker restart reuses the same immutable snapshot.

## Performance and ownership

The source is copied and hashed once at daemon startup. Each worker generation
rehashes the retained sealed bytes before native load; ordinary dictation
sessions do neither. This keeps the low-latency resident path free of pathname
I/O and per-session model loading while preserving independent child
attestation. The memfd consumes page-cache/memory proportional to the selected
model; model-size budgeting and installed-model residency accounting remain
required before release.

The daemon owns the retained snapshot. A worker owns only its inherited
descriptor and native model mapping. Existing bounded worker shutdown/recovery
rules remain unchanged.

## Rejected alternatives

- Revalidating a pathname at each spawn still leaves a validation/open race and
  can contradict a running resident.
- Retaining an ordinary open file descriptor prevents rename substitution but
  not in-place writes by the same user.
- Advisory locks do not constrain uncooperative writers.
- `fs-verity` and package-manager identity are not guaranteed for the current
  controlled development cache; they belong in installed-package composition.
- Trusting model-manager existence state would promote an unverified state to
  runtime identity.

## Consequences and deferred decisions

This closes the controlled model-byte substitution gap for the resident lane.
It does not prove that the model belongs to an installed package, model pack,
SBOM or release artifact. It does not qualify model accuracy, latency, memory,
power, native microphone behavior, core-dump/swap protection or packaging.

Recovery 4Q-C must decide, in a separate ADR, how installed worker and model
artifacts are resolved from package/model manifests, how package/SBOM identity
is reported, and how large-model memory/disk budgets are enforced without
weakening this descriptor contract.
