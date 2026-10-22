# Recovery Phase 2C foundation — portal/EIS eligibility, lifecycle, and permission driver

**Recorded:** 2026-08-27  
**HEAD:** `ff6d864012adb1468d02c6b5f80b9ce92258fa04` (unchanged)  
**Status:** non-release implementation complete; external delivery remains unqualified

## Objective and result

This bounded increment investigated the negative Recovery 2B COSMIC Edit
result and established the smallest non-interactive foundation for a better
delivery path. It did not repeat audio capture or ASR work.

Primary-source review and the current host both support XDG RemoteDesktop v2
plus EIS as the least-privilege candidate for focused-window keyboard events.
The host advertises RemoteDesktop v2 and a keyboard device. That observation
means only that Narrivy may offer an explicit permission request; it does not
prove a granted session, EIS compatibility, text retention, Unicode behavior,
or delivery.

The implementation adds a Linux-adapter-local, 750 ms bounded, read-only probe
that queries only portal version and advertised device types. Beside it are a
fakeable single-owner lifecycle contract, a daemon-owned actor running that
contract on one supervised worker, and a narrow direct-zbus permission driver.
The actor uses a bounded command queue,
linearizes command admission with shutdown, closes a successful activation if
its caller can no longer receive the result, distinguishes explicit close from
portal-originated revocation, and joins under a bounded shutdown budget. The
lifecycle admits at most one grant/transport, rejects duplicate and invalidated
activation, closes on cancellation before submission admission, and
distinguishes confirmed remote close from failed cleanup. Failed remote close
immediately drops local authority and leaves the owner terminally invalid;
Drop cannot falsely claim acknowledgement or revive it. The permission driver
implements only the approved RemoteDesktop request/session calls, owns but does
not expose the EIS file descriptor, and remains non-composed. No sender exists,
and neither the actor nor the driver is reachable from runtime or IPC.

The existing daemon `delivery` diagnostic no longer promotes `wtype` presence
to availability: it always reports external delivery unavailable/unqualified
while describing whether portal permission may be requested. Calling the
synchronous probe from an existing Tokio runtime fails closed rather than
risking nested-runtime panic.

No portal request, input event, clipboard operation, microphone run, target
window, transcript, evidence record, requirement update, manifest reseal,
package, commit, or push occurred.

Independent read-only review rounds found and drove rework for typed portal
absence, actual timeout/drop coverage, owner-drop cleanup, post-activation
cancellation admission, invalid-state truth, and revocation reporting. The
actor review additionally found and drove rework for atomic
admission/shutdown, caller-timeout cleanup, bounded queueing, immediate local
abort, and terminal invalidity. Separate driver reviews drove rework for opaque
CSPRNG tokens, authoritative path mismatch cleanup, pre-dispatch versus
post-dispatch ambiguity, session-revocation preemption, and truthful remote-close
acknowledgement before returning PASS. Permission IPC/UX, native portal behavior,
and a sender's first-event cancellation linearization remain deferred.

Source and pinned-library inspection found that ashpd 0.12's high-level
RemoteDesktop request methods do not expose a pending request while the user
dialog is active, and its public window API cannot reconstruct the validated
Wayland parent token inside the separate daemon. Wrapping those calls in a
timeout could abandon a live portal request. ADR-0023 therefore confines the
implemented adapter-local direct-zbus driver to only the required
request/session methods. The driver is independently reviewed, non-composed,
sender-free, and not native qualified in this record.

Primary-source and open-source comparison covered the XDG RemoteDesktop and
Session specifications, libei sender and `ei_text` protocols, Reis 0.7.1,
eitype, wdotool, and a Reis-based Wayland RDP implementation. It confirmed that
`ei_text.utf8` can be the lowest-overhead path when offered, but only for one
nonempty payload of at most 254 bytes and without proving target retention.
Reis is not approved yet: `REIS_DEBUG` can disclose text to stderr, its Tokio
adapter lacks writable readiness/backpressure handling, handshake flush errors
are insufficiently surfaced, and its default interface advertisement is wider
than Narrivy needs. eitype uses an older Reis plus XKB and does not remove these
bounded-I/O/privacy concerns; wdotool describes its libei Unicode support as
partial. No dependency was imported on popularity or self-reported readiness.

## Intended implementation scope

- `Cargo.toml`: exact-pinned `ashpd` 0.12.0, Tokio 1.53.1, zbus 5.13.2,
  futures-util 0.3.34, and rand 0.9.5 workspace entries.
- `Cargo.lock`: dependency resolution for the portal probe and permission driver (alongside the
  already-dirty recovery lockfile state).
- `adapters/platform-linux/Cargo.toml`: Linux-only portal/runtime dependencies.
- `adapters/platform-linux/src/lib.rs`: export the eligibility/lifecycle and
  direct-zbus permission-driver modules.
- `adapters/platform-linux/src/portal_eis.rs`: typed eligibility boundary,
  real bounded probe, fakeable owned lifecycle, controlled tests, and ignored
  host observation.
- `adapters/platform-linux/src/portal_eis_zbus.rs`: narrow permission-only
  direct-zbus driver with typed cancellation, deadline, revocation, and cleanup
  behavior; it is not composed into the daemon or IPC.
- `services/narrivyd/src/daemon/policy.rs`: fail-closed delivery diagnostic and
  controlled mapping test.
- `services/narrivyd/src/daemon/portal.rs`: bounded supervised actor over the
  fakeable lifecycle contract; no runtime composition or submission API.
- `services/narrivyd/src/daemon/mod.rs` and `services/narrivyd/src/main.rs`:
  expose the actor module to the daemon crate without composing or invoking it.
- `docs/11-adrs/ADR-0023-portal-eis-delivery-candidate.md`: architecture,
  security, lifecycle, latency, and qualification decision.
- This baseline and `agent/STATE.json`.

No Phase 2A/2B implementation was refactored by this increment.

## Validation

| Check | Exit | Material result |
|---|---:|---|
| `cargo test --workspace --all-targets --all-features --locked` | 0 | Full workspace passed after driver rework; Linux platform 176 passed/1 ignored, daemon main target 36 passed, and IPC single-connection test passed in about 30.15 s. |
| Portal-focused controlled tests | 0 | `cargo test -p narrivy-platform-linux portal_eis --lib --locked`: 35 passed/1 ignored. Daemon actor controlled coverage: 11 passed in each compiled target. Coverage includes probe truth, real pending-future timeout/drop, parent-token redaction, activation/connect/close ordering, bounded queue pressure, atomic admission/shutdown, caller-timeout cleanup, duplicate/invalid admission, grant/connect cancellation, post-activation cancellation admission, owner-drop/immediate abort, distinct close/revocation, cleanup-failure truth, exact D-Bus signatures/options, CSPRNG token format/uniqueness, path-mismatch cleanup retention, pre/post-dispatch ambiguity, and revocation preemption. |
| Read-only host observation | 0 | `ReadyToRequestPermission { version: 2 }`; never delivery-ready. |
| `cargo check --workspace --all-targets --all-features --locked` | 0 | No diagnostics. |
| `cargo +1.85.0 check -p narrivy-platform-linux -p narrivyd --all-targets --all-features --locked` | 0 | Pinned portable MSRV accepted the Linux lifecycle and daemon actor code. |
| `cargo fmt --all -- --check` | 0 | Pass. |
| Phase 2C production-code Clippy (`--no-deps`) | 0 | Linux adapter library and daemon binary passed with warnings denied after actor rework. |
| Full workspace Clippy including tests | 101 | Existing unrelated test/production lint debt remains, including mock/transcribe/history panic/unwrap and history style findings; no Phase 2C production diagnostic. |
| `cargo deny --offline --manifest-path adapters/platform-linux/Cargo.toml check advisories licenses sources` | 0 | New Linux dependency graph passed advisories, licenses, and sources; unmatched allow-list entries were warnings. |
| `python3 scripts/verify-boundaries.py` | 0 | Source boundaries pass. |
| `python3 scripts/verify_dependency_boundaries.py` | 1 | Existing metadata policy flags `windows-sys` through portable/core dependency resolution. Not weakened or bypassed. |
| Desktop `npm test -- --run` | 0 | 17 files/24 tests passed; existing React `act(...)` warnings remain. |
| Desktop `npx tsc --noEmit` | 0 | No diagnostics. |
| Governance unit tests | 0 | 12 passed. |
| `python3 scripts/verify_traceability.py` | 0 | 136 mapped; all 98 Linux release requirements remain blocked. |
| `python3 scripts/verify_evidence.py --production` | 1 | 148 schema-v1 records quarantined; zero v2 candidates. |
| `python3 scripts/validate_pack.py` | 1 | Expected dirty/unsealed read-only manifest failure. |
| `python3 scripts/release_gate.py --target linux-1.0` | 1 | Expected fail-closed; 98 requirements blocked. |
| `python3 scripts/run_gate.py --phase P11` | 2 | Quarantined; no writes. |

The six final governance commands were measured immediately before and after:

- workspace inventory/content:
  `ad845e675de7a4b93c62ba807612dedf9013f2b91e29f39928d0af4047414917`;
- Git diff: `139ffc772f250923d551b7939105ec680a99e7d299e6f30e9333bcf7ec3e8dc1`;
- Git status: `647a0b717d5ed78c85870664d6c8ba8e4d3b15b36b2ba079e83dc1cf6eb77da0`.

Each value was identical before and after the validator window.

Protected hashes after implementation and validation:

- `artifacts/releases/narrivy_0.2.3_amd64.deb`:
  `77733e7ce7c0ace6820c928a6827e0056d2d18490e166927217351724da419c0`;
- `packaging/deb/BUILD_LOG.md`:
  `0c52da5778bf9b8f6f3c843ff0cb0db6e9bc6d206a63d1c898ae5608fba85112`;
- `spec/requirements-traceability.csv`:
  `ffe19846bf4710ee2ce015b6081ceeb87fff4bc1acac68ac281e738579e59d0f`;
- `agent/evidence/INDEX.json`:
  `dfb5bd72bfb8f63d88b9f8d4027304d491b737c19a584bc293aff2b97923adc5`.

## Architectural boundary and latency plan

Portal negotiation and user consent are cold-path operations. A later daemon-
owned session must be established explicitly from the desktop client and kept
alive only while the grant is active. The dictation hot path must enqueue one
bounded submission to that owner; it must not create a portal session, perform
per-key D-Bus round trips, or log/clone transcript content for diagnostics.
EIS frames should be batched and flushed deliberately under one absolute
deadline. Cancellation truth is based on the first accepted event: before it,
clean cancellation; after it, submission uncertainty and exactly one durable
Inbox recovery.

## Required next increment (separate review boundary)

1. Perform a controlled, attended native behavior check of the accepted
   permission driver before composing it. The check may validate permission,
   cancellation, revocation, and cleanup behavior only; it must not create or
   submit input events, capture audio, create evidence, or claim delivery.
2. Add an authenticated, additive parent-window permission/revoke IPC command
   and explicit desktop UX only after the driver passes independent review. Do
   not overload generic `StartSession`.
3. Decide in a separate ADR between a reviewed minimal Reis patch/fork and a
   native libei boundary. The selected sender must fail closed when `REIS_DEBUG`
   or an equivalent payload-debug path is active, use writable readiness and
   backpressure, advertise only required interfaces, surface flush errors, and
   use one absolute deadline.
4. Add only a single-frame `ei_text` path first when the compositor advertises
   it. Reject empty or over-254-byte input to mandatory Inbox recovery; do not
   chunk or silently keymap-replay in that increment.
5. Qualify only with a fixed ASCII canary and controlled target readback—no
   microphone. A zero helper/API exit is never delivery proof.
6. Keep Inbox recovery mandatory for any unverified or post-dispatch-uncertain
   submission. Unicode, multiline, clipboard, restore tokens, and general
   target support remain separate decisions.

## Cleanup and rollback

This increment created no ordinary audio/transcript/payload files and left no
Narrivy, recorder, ASR, or wtype process. Repository additions are intentional
source, lockfile, ADR, and baseline files rather than generated evidence or
build products. Thirteen stale, user-owned `/tmp/narrivy-*` directories from
earlier Phase 2A work were inspected, confirmed as isolated native-run state or
reproducible Cargo targets, and deleted after confirming there were no active
processes or mounts. This reclaimed roughly 21 GB and removed old temporary
journal/Inbox payloads. It did not touch the repository, user model cache, or
protected files. A separate exact `/tmp/narrivy-research.*` directory used for
the open-source comparison was deleted afterward; its shallow clone was
incomplete and no source was copied into Narrivy. Final repository/history
cleanup remains a dedicated post-acceptance audit; it must preserve truthful
ADRs and baseline records rather than erase the recovery trail.

Rollback only the Phase 2C hunks/files listed under intended scope. Preserve
accepted Phase 1/2A/2B changes and the protected files above.
