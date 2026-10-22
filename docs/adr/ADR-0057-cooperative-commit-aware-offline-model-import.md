# ADR-0057: Cooperative, commit-aware offline model import

Status: Accepted design for controlled Recovery 4Q-C5B2A (2026-09-03);
non-release and not an IPC provisioning surface

## Context

ADR-0055 established a complete-directory, no-replace offline model import.
That primitive had no caller-owned cancellation or deadline contract. A later
daemon provisioning command cannot safely own shutdown or give truthful results
unless the long copy and verification phases can observe one cancellation token
and one absolute time budget. Cancellation also cannot be allowed to turn a
successfully published directory into a reported abort.

Filesystem calls can block in the kernel. A synchronous filesystem primitive
therefore cannot promise a hard wall-clock bound merely by accepting a
deadline. It can provide bounded-size work between cooperative checks and make
its irreversible commit boundary explicit.

## Decision

The Linux import API accepts a shared `SessionCancellation` reference and one
absolute `Instant` deadline. Cancellation has deterministic priority when both
conditions are observed. Copying and digest verification use fixed-size chunks
with control checks around each read/write phase; partial writes also recheck
control. Metadata remains bounded. The portable model manager exposes the same
typed cooperative budget for the final bundled installed-model observation, so
a concurrent/idempotent loser cannot fall into an unbounded second hash after
its Linux transaction has honored cancellation.

The staged model is independently reread and rehashed after its leaf sync. The
completed stage is synced before publication. The final control check occurs
immediately before descriptor-relative `RENAME_NOREPLACE`; that rename is the
irreversible publication point.

Before publication, a cancellation or timeout is reported as clean only after
the owned fixed leaves and staging directory have been removed and the staging
parent has been synced. Failure to confirm that cleanup returns
`CleanupFailedBeforePublication`, not a cancellation or timeout result.

After publication, the operation never removes the destination and never maps
control to a clean cancellation/timeout. The original cancellation/deadline
may stop the remaining confirmation hash; that is reported as
`VerificationFailedAfterPublication`, because publication occurred but exact
confirmation did not. Parent-sync failure remains
`DurabilityFailedAfterPublication`. The production wrapper performs one
controlled post-publication installed observation rather than duplicating a
full model hash. A concurrent loser removes and syncs only its own stage, then
checks control and verifies the winner's exact directory before returning
success.

The API documentation explicitly describes its deadline as cooperative. The
later daemon layer must run this blocking primitive in an owned worker and
provide its own bounded lifecycle policy; this ADR does not claim that an
arbitrary blocking filesystem syscall can be preempted.

## Security and privacy

Source and cache traversal, ownership/mode checks, nofollow opens, fixed leaf
names, descriptor-held parents, and no-replace publication remain Linux-adapter
responsibilities. The cancellation API does not accept caller-defined polling
callbacks in production. Typed errors contain stable categories only and do
not embed model IDs, source paths, cache paths, or model bytes.

Cleanup is fixed-leaf and descriptor-relative; it never recursively follows a
replacement pathname. Best-effort `Drop` cleanup remains a last resort, while
control outcomes use the explicit cleanup-and-sync result path.

## Consequences

- C5B2A provides the cancellation/commit primitive needed by a later daemon-
  owned provisioning operation without adding that operation.
- Cancellation and timeout can stop bounded user-space copy/hash work, but the
  primitive does not make kernel filesystem calls hard-deadline operations.
- A caller can distinguish clean abort, pre-publication failure, and published-
  but-unconfirmed failure without inspecting paths or filesystem details.
- C5B2B must separately define authenticated IPC authorization, one-operation
  admission, worker ownership, shutdown behavior, lifecycle transition, and
  resident initialization. CLI and desktop UX are also outside this phase.
- Controlled temporary-file tests are not proof of native installed-package,
  real-model, performance, provenance, evidence, or release behavior.
