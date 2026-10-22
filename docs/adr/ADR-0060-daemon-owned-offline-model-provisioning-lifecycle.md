# ADR-0060: Daemon-owned offline model provisioning lifecycle

Status: Implemented for controlled Recovery 4Q-C5B2B2 (2026-09-04);
independent final review and native installed validation remain pending

## Context

ADR-0059 defined a source-only asynchronous IPC contract but deliberately left
both commands unavailable. Activating a missing installed model spans local
copy, irreversible publication, immutable descriptor binding, and native
resident startup. Splitting those stages across unrelated flags could publish
stale readiness, lose cancellation, or misstate whether bytes reached the
cache.

## Decision

One mutex-owned coordinator is the authority for resident and provisioning
lifecycle. It owns the phase, daemon-generated operation ID plus private
generation, cancellation token, cooldown, typed status, and resident candidate.
The phases are unconfigured, awaiting, provisioning, ready,
published-unconfirmed, activation-failed, and closing. Only a matching active
generation may advance or publish a resident candidate. Closing cancels the
operation before the supervisor takes its worker snapshot; any late candidate
is returned to its owner for bounded shutdown.

The authenticated client supplies only one bounded absolute local source path.
The daemon selects the bundled model identity, digest, cache destination,
no-network store, ten-minute cooperative deadline, and worker supervisor. One
token and one absolute deadline flow through complete-directory import,
immutable installed-model binding, and resident startup. The deadline starts at
admission rather than when a worker happens to run.

Provisioning is asynchronous and globally single-flight with a 30-second start
cooldown. Status snapshots expose monotonically increasing state, publication,
cleanup, and failure categories. Cancellation that linearizes before another
clean pre-publication result returns to awaiting; cancellation after verified
publication fails closed without deleting installed bytes. Ambiguous cleanup,
existing-install, post-publication, binding, activation, and worker failures do
not auto-retry.

The Linux source open is nonblocking before regular-file validation, so an
owned FIFO cannot trap the daemon worker before it can observe control. After
atomic publication, confirmation remains under the original control budget;
expiry is publication uncertainty, never rollback or clean cancellation. The
production path performs one post-publication full verification hash.

## Security and privacy

The bundled registry is compiled authority and cannot be replaced by IPC.
Source traversal remains descriptor-relative, no-follow, same-real-user,
regular-file, private-mode, and exact-size/digest checked. Narrivy rejects a
real/effective UID mismatch at immutable binding. Generic command/status debug
output redacts source and operation identity, and ordinary daemon errors contain
no paths or model content.

Same-UID IPC is the current authorization boundary. This ADR does not claim to
close ADR-0031 for hostile processes already running as the same account or to
provide privileged administrative roles.

## Consequences and limits

- The installed daemon can move from truthful awaiting state to Ready without
  accepting client-selected model authority or falling back to mock ASR.
- Controlled tests establish state-machine, filesystem, cancellation, and
  compilation behavior only. No real bundled model was imported by this work.
- No CLI/Desktop provisioning UX, event stream, package install, evidence,
  requirement pass, manifest seal, commit, push, or release claim is included.
- A dedicated successor review must add/verify production orchestration race
  tests for shutdown during import and after candidate creation, spawn rejection,
  and a content-free disposable real-model observation before accepting this
  work item.
