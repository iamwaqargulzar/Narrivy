# Recovery Phase 2C additive IPC contract — non-composed

**Recorded:** 2026-08-27  
**HEAD:** `ff6d864012adb1468d02c6b5f80b9ce92258fa04` (unchanged)  
**Status:** controlled wire-contract and generic permission-coordinator
implementation complete; native permission, runtime composition, text
submission, target qualification, and release remain blocked

## Result

Protocol minor 2 adds explicit request/cancel/revoke vocabulary and typed,
sequenced status for a future keyboard-only portal permission service. The
contract is bounded, preserves future enum/error strings, binds each decoded
command to its outer request tag, and redacts the opaque operation and parent
values from debug/rejection surfaces.

The daemon now has a generic, controlled-test-only permission coordinator over
the existing actor. It implements one pending/active operation, same-ID
idempotence, different-ID `Busy`, cancellation, explicit revoke,
portal-originated revocation, sequenced status, cleanup invalidation, and
bounded shutdown. Activation-generation binding plus deterministic contention
tests prevent a matching revocation from being overwritten by `Active` or by
an overlapping explicit revoke.

The coordinator and direct-zbus driver remain uncomposed. Every production IPC
command returns `DaemonUnavailable` with one generic content-free detail; no
desktop or CLI command can request a portal dialog. `sender_available` and
`delivery_qualified` remain false in all controlled status and no production
status advertises the optional portal object.

Independent review found and drove rework for disguised command payloads,
minor-version enum compatibility, debug redaction, new serializer lint, and
two service event-linearization races. The final scoped re-review returned
PASS.

## Scope

- `crates/narrivy-ipc/src/protocol.rs`: minor version, stable kinds/commands,
  bounds, typed DTOs/response/event, command-kind binding support, redacted
  debug, and controlled tests.
- `crates/narrivy-ipc/src/error.rs`: stable `busy` code.
- `crates/narrivy-ipc/src/lib.rs`: public re-exports and version assertion.
- `crates/narrivy-ipc/src/server.rs` and existing status constructors:
  mechanical `portal_keyboard: None` initialization only.
- `services/narrivyd/src/daemon/session.rs`: command-kind enforcement and
  explicit fail-closed handling for the three uncomposed commands.
- `services/narrivyd/src/daemon/portal.rs`: generic non-production-composed
  coordinator, activation-generation reconciliation, and deterministic
  lifecycle contention tests.
- `apps/narrivy-cli/src/main.rs`: exhaustive unexpected-response handling only;
  no portal command was exposed.
- ADR-0024, this baseline, and the truthful state record.

## Validation

| Check | Exit | Material result |
|---|---:|---|
| `cargo test -p narrivy-ipc --lib --locked` | 0 | 39 passed, including exact tags, bounds, unknown-value preservation, debug redaction, and existing IPC security/framing tests. |
| `cargo test -p narrivyd --locked` | 0 | Daemon targets passed; controlled regressions reject disguised commands and all uncomposed portal commands without echoing canaries. |
| `cargo test -p narrivy-cli --locked` | 0 | 3 passed. |
| `cargo test -p narrivyd portal --all-targets --all-features --locked` | 0 | 22 selected tests passed in each of three compiled daemon targets (20 portal service/actor tests plus two adjacent portal contract tests). |
| `cargo check --workspace --all-targets --all-features --locked` | 0 | Pass. |
| `cargo test --workspace --all-targets --all-features --locked` | 0 | Pass; the slow IPC accept-once test completed in 30.19 seconds. Linux portal adapter total was 176 passed/1 ignored; daemon main was 47 passed. |
| `cargo clippy -p narrivy-ipc --lib --all-features --locked -- -D warnings` | 0 | Touched IPC production code passes. |
| `cargo clippy -p narrivyd --bin narrivyd --all-features --locked --no-deps -- -D warnings` | 0 | Touched daemon production code passes. |
| `cargo deny --offline --manifest-path adapters/platform-linux/Cargo.toml check advisories licenses sources` | 0 | Advisories, licenses, and sources pass; four pre-existing unmatched-license-allowance warnings remain. |
| `python3 scripts/verify-boundaries.py` | 0 | Pass after portable IPC test handles were made platform-neutral; no OS implementation token remains in the wire crate. |
| `cargo fmt --all -- --check` | 0 | Pass. |
| `git diff --check` | 0 | Pass. |

The six final governance commands were measured immediately before and after:

- workspace inventory/content:
  `b47c9e2a8938a497ce30758f7af1b5ef3ef469c14c59cdc71d6e95e116dc5365`;
- Git diff: `87aa98b8c43472b6dedfeb70a7ac5f775c3ec1226fbe353f1d5c38b95b4ea715`;
- Git status: `2c3eb4157846c9f27ab3baca0ac5810f64ffed9bbe524fd874aecfa651cc2496`.

Each value was identical before and after. Governance tests and traceability
exited 0; production evidence, pack validation, and the release gate exited 1;
the quarantined P11 runner exited 2 without writing. The evidence inventory
remains 148 historical/quarantined schema-v1 records and zero v2 candidates;
all 98 Linux-1.0 requirements remain blocked.

An isolated Tao window-harness experiment was rejected and fully rolled back
before any native run because its GTK3 dependency chain introduced nine
RustSec advisories. No harness source, workspace member, feature, dependency,
lock entry, dialog, or external capability remains. The reviewed adapter graph
continues to pass the scoped offline dependency policy above.

## Explicit non-claims

No production portal request, permission dialog, grant, EIS connection, input event,
microphone run, target window, transcript, evidence record, evidence index,
requirement status, manifest reseal, package, commit, or push occurred. All 98
Linux-1.0 requirements and the release remain blocked.

## Next safe boundary

Do not revive the advisory-bearing harness. The next implementation boundary is
a separately reviewed, dependency-clean way to export a real desktop main-window
parent handle and compose only permission ownership. Its native check must be
attended and content-free: parented prompt, deny/allow/cancel/revoke, portal
loss, bounded cleanup, and zero residual resources. It must not submit input or
use a microphone. EIS sending remains a later ADR and work order.

## Subsequent controlled sender-contract increment

ADR-0025 later added a non-composed sender and Inbox-recovery contract with no
real EIS implementation or production route. This historical IPC result remains
unchanged: permission composition and native permission behavior are still
blocked, and sender/delivery capability still reports false.
