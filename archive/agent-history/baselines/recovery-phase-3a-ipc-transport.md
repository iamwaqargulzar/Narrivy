# Recovery Phase 3A — Unix IPC transport boundary

**Recorded:** 2026-08-28  
**Base HEAD:** `ff6d864012adb1468d02c6b5f80b9ce92258fa04` (unchanged)  
**Status:** implementation complete and independently reviewed; non-release;
every requirement remains blocked

## Objective

Extract concrete Unix IPC transport, endpoint security, and lifecycle ownership
from portable `narrivy-ipc` into an inward-dependent `adapters/ipc-unix` crate.
Preserve the protocol wire contract and same-user security while repairing the
pre-existing handshake, write-serialization, registration, and connection
shutdown races discovered during independent inventory.

## Permitted scope

- `crates/narrivy-ipc`: retain only transport-neutral errors, bounded framing,
  protocol envelopes/DTOs, version negotiation, and their tests/benchmark.
- new `adapters/ipc-unix`: own Unix client/server, endpoint path/modes,
  peer-credential FFI, stale-socket handling, endpoint lock, and connection
  lifecycle.
- daemon, CLI, desktop, and existing service tests: compose the new adapter.
- boundary validators and isolated governance tests: detect real source
  coupling and inspect direct normal/build dependencies without transitive,
  dev-only, or host-target false positives.
- architecture/ADR/baseline/state documentation required to describe the
  actual boundary and command results.

## Required implementation behavior

1. Keep protocol major/minor, JSON payloads, frame bytes, request IDs, event
   sequencing, error wire tags, and bounded message sizes unchanged.
2. Authenticate kernel peer identity before reading a handshake or invoking a
   handler. A self-declared client role is never authorization.
3. Register a client for event publication only after handshake acceptance.
   Every terminal path must release its registration.
4. Serialize all post-handshake response, rejection, and event frames through
   one per-connection writer lock.
5. Close connection admission before shutdown snapshot; close sockets, reap
   every admitted connection worker within a bounded deadline, and prevent a
   successor endpoint generation while an old handler retains authority.
6. Preserve 0700 runtime-directory and 0600 socket/lock modes, exact-owner
   validation, symlink/regular/foreign stale-path refusal, directory fsync,
   and non-blocking endpoint flock.
7. Keep the daemon process-wide instance lock. Remove only the redundant
   daemon-side pre-bind socket deletion; the Unix IPC adapter is the sole
   socket cleanup authority.
8. Make `narrivy-ipc` use workspace `unsafe_code = forbid` and remove its
   platform/libc dependency.

## Acceptance tests and commands

- portable IPC contains no production `std::os::*`, Unix socket/raw-FD/fs
  extension, `libc`, platform FFI, endpoint path, or concrete transport code;
- real Unix tests cover handshake/request/event/reconnect, same-user peer
  credentials, endpoint modes, stale paths, and second-instance refusal;
- event publication cannot precede handshake acceptance;
- repeated concurrent event and response traffic always produces intact frames;
- rejected/malformed/disconnected handshakes leave zero event clients and
  connection workers;
- shutdown is bounded, closes admission, wakes/reaps clients, and prevents
  overlapping server generations;
- daemon/CLI/desktop composition and service integration tests pass;
- source validator fixtures distinguish source from comments/strings and reject
  platform imports in protected production sources;
- dependency validator checks direct normal/build dependencies, rejects
  renamed/target-specific direct forbidden packages, excludes dev-only edges,
  and does not reject reviewed portable dependencies for their target-specific
  transitive internals;
- `cargo fmt`, full workspace check/test, configured and forced strict Clippy,
  frontend tests, TypeScript, and governance tests retain truthful outcomes;
- cross-target portable IPC checks run only on actually installed targets;
  unavailable targets are reported, never simulated.

## Explicit non-goals

- no recovery-journal extraction (ADR-0030B);
- no portal runtime wiring, microphone/native observation, or product feature;
- no IPC wire/version change or compatibility shim;
- no evidence, evidence-index, requirement, manifest, packaging, commit, push,
  or release-state change;
- no dependency-advisory suppression and no tracked-target cleanup.

## Rollback boundary

The phase is one adapter extraction plus consumer import/dependency migration,
validator tests, and its documentation. Rollback must preserve accepted Phase
1/2A/2B/2C and quality-gate work, protected artifacts/catalogs, and all
user-owned changes. If bounded connection ownership requires a wire change or
policy weakening, stop for architectural review instead of broadening scope.

## Implemented result

- `narrivy-ipc` is now a transport-neutral, `unsafe_code = forbid` crate for
  envelopes, DTOs, version negotiation, framing, and typed errors. It has no
  production Unix imports, socket types, raw descriptors, filesystem-mode
  extensions, platform FFI, or `libc` dependency.
- `narrivy-ipc-unix` owns the blocking Unix client/server, default socket path,
  kernel peer credentials, 0700/0600 endpoint controls, safe stale-path rules,
  endpoint flock/lease, connection workers, and bounded shutdown.
- The server verifies the client UID before reading the handshake or invoking
  a handler. The client verifies the daemon UID before serializing or writing
  its hello; an injected rejection test proves the fake daemon receives zero
  bytes.
- Event eligibility starts only after the accepted handshake is written.
  Handshake responses, rejections, command responses, and events share one
  per-connection writer lock. Terminal RAII removes registry entries.
- Admission and shutdown are linearized under one lifecycle lock. An admitted
  worker is registered before its buffered start gate is released; shutdown's
  deadline includes admission closure, interrupts clients, and joins owned
  workers. An endpoint lease prevents a successor generation from overlapping
  a timed-out old handler.
- The adapter rejects symlink, wrong-type, wrong-owner, insecure-mode, and
  multiply linked lock paths. Identity-aware cleanup preserves a replacement
  inode. The daemon retains a distinct process-lifetime instance lock; all
  duplicate daemon/platform stale-socket cleanup was removed.
- Daemon, CLI, desktop, and service tests compose the Unix adapter explicitly.
  Protocol major/minor, payloads, tags, request IDs, four-byte little-endian
  framing, and the 1 MiB frame bound were not changed.
- Boundary validation is now importable and fixture-tested. Direct
  normal/build implementation dependencies fail; dev-only and reviewed
  transitive target internals do not manufacture architecture failures.
  Source scanning ignores comments/strings but catches real platform syntax.

## Independent review and rework

Two independent read-only reviews found and then verified repair of:

1. missing client-side daemon UID validation before hello bytes;
2. a zero-capacity worker-start rendezvous that could consume shutdown's
   deadline before admission closed;
3. duplicate stale-socket cleanup ownership and stale consumer documentation;
4. insufficient permanent write-interleaving stress coverage.

The final committed regression races 32 correlated responses against 32
uniquely indexed 64 KiB events on one authenticated connection and validates
every decoded result and event. The wire/consumer reviewer reran it ten times
(320 response races and 320 large events) and returned PASS. The lifecycle
reviewer ran the new authentication and admission/shutdown tests fifty times
each before returning its rework for independent acceptance.

## Security claim boundary

ADR-0031 records the limit discovered during review: Linux peer credentials
authenticate an effective UID, not one trusted process among hostile same-UID
processes. POSIX pathname checks cannot atomically exclude malicious same-UID
replacement. The implementation retains all cross-user, symlink, type, mode,
link-count, owner, inode, and flock controls, but makes no cryptographic or
per-process identity claim. Privileged command authorization and rate policy
remain the explicit release blocker B-IPC-012.

## Final command results

| Command | Exit | Material result |
| --- | ---: | --- |
| `cargo fmt --all -- --check` | 0 | workspace formatted |
| `git diff --check` | 0 | no whitespace errors |
| `cargo check --workspace --all-targets --all-features --locked` | 0 | no diagnostics |
| `cargo test --workspace --all-targets --all-features --locked` | 0 | full Rust workspace passed |
| forced strict workspace Clippy (`warnings`, `unwrap`, `expect`, `panic`) | 0 | no diagnostics |
| `cargo test -p narrivy-ipc-unix ...concurrent... --locked` | 0 | 32 response/event races passed |
| desktop `npm test -- --run` | 0 | 17 files, 24 tests; existing React `act(...)` warnings |
| desktop `npx tsc --noEmit` | 0 | no diagnostics |
| boundary-validator fixture suite | 0 | 17 tests passed |
| Recovery governance suite | 0 | 12 tests passed |
| `python3 scripts/verify_dependency_boundaries.py` | 0 | 8 protected packages pass |
| `python3 scripts/verify-boundaries.py` | 1 | only deferred core journal Unix coupling at lines 11 and 344 |
| `cargo deny --offline check bans licenses sources` | 0 | pass with duplicate/unmatched-allowance warnings |
| `cargo deny --offline check advisories` | 1 | unsuppressed GTK3/glib/quick-xml/time/unic findings remain |
| `rustup target list --installed` | 0 | Linux GNU only; no Windows/macOS claim |
| `python3 scripts/verify_traceability.py` | 0 | 136 mapped; 98 Linux requirements blocked |
| `python3 scripts/verify_evidence.py --production` | 1 | 148 schema-v1 records quarantined; zero v2 candidates |
| `python3 scripts/validate_pack.py` | 1 | truthful source-boundary failure; read-only |
| `python3 scripts/release_gate.py --target linux-1.0` | 1 | pack failure and all 98 requirements blocked |
| `python3 scripts/run_gate.py --phase P11` | 2 | quarantined; no writes |

## Preserved identities and deliberate red gates

- Protocol: `9cae75d20de9d4cd64632deac0f6702293bcb44b2ce5c4d9570cd74c7a7b321d`
- Framing: `fd7e4e9a2ace32548e823746562c49b5c704d31888e9fb4dcbe31df8d2ebc2e1`
- Typed IPC errors: `aa0e38d90905e3ff603233078bc7c7f3cef0d4e19e02ae8469a5876f500580ae`
- Protected `.deb`: `77733e7ce7c0ace6820c928a6827e0056d2d18490e166927217351724da419c0`
- Protected `BUILD_LOG.md`: `0c52da5778bf9b8f6f3c843ff0cb0db6e9bc6d206a63d1c898ae5608fba85112`
- Traceability catalog: `ffe19846bf4710ee2ce015b6081ceeb87fff4bc1acac68ac281e738579e59d0f`
- Empty evidence index: `dfb5bd72bfb8f63d88b9f8d4027304d491b737c19a584bc293aff2b97923adc5`

Immediately before and after the seven final validation commands, the hashes
matched exactly:

- tracked/unignored workspace inventory and content:
  `335ccba4fe63e49ba9a5ea93526736b26ca42eed230d07c30b55651dd8319d90`;
- binary Git diff from base HEAD:
  `356579e0106b18c965a8a71d822812008e2b2b628259f3c6813100cb5e631c0e`;
- porcelain Git status:
  `348c241b8b4486c8d1ca9d3694a036741507d3d1a6c0977f3aaa64016a66a361`.

Journal extraction is Phase 3B, so the source boundary gate intentionally
remains red. Supply-chain advisories remain unsuppressed. No target was
installed, no native microphone/portal action occurred, and no evidence,
requirement status, manifest, package, commit, or push was created.
