# ADR-0061 — Daemon-owned IPC command authorization and rate matrix

**Status:** Accepted for local implementation (2026-09-04); release evidence
and hostile-same-UID isolation remain separate

> **History composition amendment (2026-09-28):** the rate matrix is unchanged
> and still enforced in `services/narrivyd/src/daemon/ipc_policy.rs`
> (`GetHistory` privacy-sensitive 16 / 1 s, `GetHistoryEntry`
> privacy-sensitive 32 / 1 s). The "currently fail-closed" wording above is
> stale: `narrivyd` now composes a real optional History store
> (`services/narrivyd/src/daemon/retained_history.rs`), so both commands are
> served whenever an open store exists and return the typed not-composed
> rejection only when composition failed
> (`services/narrivyd/src/daemon/session.rs:4015-4060`).

## Context

ADR-0031 establishes that Linux `SO_PEERCRED` authenticates an effective UID,
not a Narrivy executable. The handshake role and reported PID are diagnostic
facts. They cannot authorize microphone, transcript, portal, delivery or model
operations because another process running as the same UID can claim the same
role and reconnect with a different PID.

Transport bounds already limit frame size, idle time and concurrent
connections. They do not provide an exhaustive product-command policy or stop
reconnects from resetting a connection-local budget.

## Decision

`narrivyd` owns one command-policy instance for one daemon generation and
shares it across every authenticated connection. The policy:

1. accepts only the endpoint owner's kernel-authenticated UID;
2. never receives or consults the self-declared handshake role;
3. classifies every command exhaustively, so adding a command makes the Rust
   match fail to compile until a rule is selected;
4. applies a fixed-window budget keyed by exact command across every
   connection for that daemon generation;
5. returns typed `busy` rejections with content-free class-level detail;
6. resets only when its time window expires or an explicit daemon restart
   creates a new policy instance.

The fixed windows are deliberately small, inspectable resource bounds rather
than behavioural identity heuristics:

| Command | Class | Burst / window | Daemon-owned intent or lifecycle constraint |
|---|---|---:|---|
| `GetStatus` | read-only | 64 / 1 s | content-safe snapshot; no transcript body |
| `StartSession` | state-changing | 8 / 1 s | explicit request plus single-session/admission state; `activation` remains diagnostic only |
| `StopSession` | state-changing | 16 / 1 s | active session must exist; graceful finalization |
| `CancelSession` | destructive | 16 / 1 s | active session must exist; typed cancellation and recovery terminalization |
| `Subscribe` | privacy-sensitive | 16 / 1 s | accepted connection-local subscription before transcript-bearing events |
| `Unsubscribe` | state-changing | 16 / 1 s | accepted acknowledgement is an event-publication barrier |
| `ListPipelines` | read-only | 32 / 1 s | bounded capability-derived catalog |
| `SelectPipeline` | state-changing | 8 / 1 s | named pipeline validation and live-runtime readiness |
| `GetHistory` | privacy-sensitive | 16 / 1 s | optional History only; served only while `history.optional=enabled`, otherwise the typed not-composed rejection |
| `GetHistoryEntry` | privacy-sensitive | 32 / 1 s | exact optional-History transaction; same composition gate as `GetHistory` |
| `GetRecoveryEntry` | privacy-sensitive | 32 / 1 s | exact transaction reference and exact journal lookup; never a listing |
| `RunCapabilityCheck` | read-only | 4 / 10 s | closed check-id set and bounded helper work |
| `RequestPortalKeyboardPermission` | state-changing | 2 / 60 s | composed single-flight service plus an attended system portal decision |
| `CancelPortalKeyboardPermission` | destructive | 8 / 10 s | exact in-flight operation reference |
| `RevokePortalKeyboardPermission` | destructive | 4 / 60 s | composed active portal authority and bounded close |
| `StartOfflineModelProvision` | state-changing | 2 / 60 s | awaiting-model state, daemon-selected identity/deadline, single flight and import cooldown |
| `CancelOfflineModelProvision` | destructive | 8 / 10 s | exact daemon-generated operation reference and commit-aware cancellation |

Operation and transaction references prevent accidental cross-operation
control and stale-client confusion. They are not described as credentials
against a hostile process already running as the same UID. User-attended
portal approval is authoritative only for the portal grant itself. A request's
`activation` text, source path, role, build string, PID and other caller data
never confer authority.

## Same-UID boundary

Linux 1.0 protects against other OS users, insecure endpoint state, malformed
or excessive traffic, accidental/stale clients and confused lifecycle
operations. A malicious process already executing as the user's UID is out of
scope for process identity and confidentiality: it may connect as the same
principal and can consume shared rate budgets. Stronger isolation would need
an independently protected broker/capability (for example a trusted desktop
launcher passing a protected file descriptor) and a separate threat review.
Role allowlists, executable-path inspection and PID checks are rejected as
false identity.

## Consequences

- Reconnects and role spoofing cannot reset command budgets while the daemon
  generation remains alive.
- A daemon restart resets in-memory windows; downstream single-flight,
  cooldown, exact-operation and recovery invariants remain authoritative.
- The policy stores only command identifiers, counts and monotonic timestamps.
  It does not store or log transcript, path, operation or transaction values.
- `busy` reaches CLI/Desktop through the existing typed IPC error path, so the
  client can present retry-later UX without guessing from strings.
- Rate limiting reduces accidental/resource abuse but is not a defence against
  a hostile same-UID process that can also stop or replace the user's daemon.

## Verification

- unit tests enumerate all 17 current commands and their classes;
- foreign UID rejection does not consume the owner's budget;
- exact commands use separate bounded buckets;
- real Unix-socket tests exhaust a state-changing budget across reconnects,
  change the claimed role to `admin`, observe the same typed `busy`, restart
  the daemon, and observe a fresh policy window;
- existing lifecycle, recovery, portal, provisioning, event-ordering and
  redaction suites remain required.
