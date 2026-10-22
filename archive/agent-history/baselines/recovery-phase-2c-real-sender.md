# Recovery Phase 2C minimal EI sender — non-composed

**Recorded:** 2026-08-27  
**HEAD:** `ff6d864012adb1468d02c6b5f80b9ce92258fa04` (unchanged)  
**Status:** real controlled EI wire sender implemented; production composition,
native portal/target qualification, every requirement, and release remain
blocked

## Result

The direct-zbus portal owner now converts its `ConnectToEIS` descriptor into a
minimal actor-owned EI sender. It uses Reis 0.7.1 only as a low-level typed wire
codec and implements its own `Send` state machine, six-interface sender
handshake, text-only device binding, readable/writable polling, and stable
failure mapping. It emits exactly one start/text/frame/stop transaction for the
existing bounded printable-ASCII payload contract.

This is no longer a mock sender: controlled tests use a Unix socket pair and a
real EIS-side protocol decoder to observe the serialized handshake and
transaction. It remains non-native and non-composed. No compositor, portal
dialog, focused application, target readback, recovery pipeline, production
IPC, CLI, or desktop client exercised it.

## Privacy, ownership, and outcome rules

- Any `REIS_DEBUG` declaration is rejected before EI context construction.
  Payloads, peer explanations, wire objects, and transport diagnostics are not
  formatted or logged.
- The portal actor owns the portal session, EI context, device, buffer,
  sequence, cancellation, and cleanup. No helper process, detached thread,
  restore token, keymap, input group, or `/dev/uinput` access was added.
- Permission/device discovery is cold-path. The send path queues one bounded
  frame and flushes a nonblocking Unix socket under the original absolute
  deadline with five-millisecond cancellation polling.
- Clean cancellation is possible only before protocol requests are queued.
  After the first flush attempt, incomplete handoff is conservatively
  `DispatchUncertain` and destroys the EI context before bounded portal cleanup
  and Inbox recovery.
- Complete local handoff is only `SubmittedUnverified`. It is not a claim that
  the compositor processed the frame or a target retained text.

## Dependency decision

- `reis = 0.7.1`, exact, default features disabled; MIT; upstream master and
  crates.io current on 2026-08-27; Rust 1.71.1 MSRV; `unsafe_code = forbid`.
- `rustix = 1.1.4`, exact, `event` and `time` only; it was already in the locked
  transitive graph and is now a direct safe polling/timestamp dependency.
- The adapter dependency-policy check passes advisories, licenses, and sources.
  Four existing unmatched license-allowance warnings remain; no policy was
  weakened.

## Controlled validation completed

| Command | Exit | Material result |
|---|---:|---|
| `python3 scripts/doctor.py` | 0 | Non-destructive host/tool/portal/PipeWire diagnostic completed. |
| `cargo test -p narrivy-platform-linux portal_eis_sender --locked --no-fail-fast` | 0 | 5 controlled wire tests passed. |
| `cargo test -p narrivy-platform-linux portal_eis --locked --no-fail-fast` | 0 | 45 passed; one read-only host observation ignored. |
| `cargo test -p narrivy-platform-linux --lib --locked --no-fail-fast` | 0 | 190 passed; one read-only host observation ignored. |
| `cargo check -p narrivy-platform-linux --all-targets --locked` | 0 | Adapter and all targets compile. |
| `cargo clippy -p narrivy-platform-linux --lib --no-deps --locked -- -D warnings` | 0 | Touched production adapter code is warning-free. |
| Adapter Clippy including tests | 101 | Two pre-existing out-of-scope test lints in delivery/instance; recorded, not changed. |
| `cargo deny --offline --manifest-path adapters/platform-linux/Cargo.toml check advisories licenses sources` | 0 | Advisories, licenses, and sources pass; four existing unmatched allowance warnings. |
| `python3 scripts/verify-boundaries.py` | 0 | Source boundaries pass. |
| `python3 scripts/verify_architecture_map.py` | 0 | 20 feature groups, 136 requirements, 12 pipelines, 26 engines, 66 modules, 8 runtimes. |
| `cargo fmt --all -- --check` and `git diff --check` | 0 | Formatting and whitespace pass. |
| `cargo check --workspace --all-targets --all-features --locked` | 0 | Workspace and every target/feature compile. |
| `cargo test --workspace --all-targets --all-features --locked --no-fail-fast` | 0 | Full Rust workspace sweep passed; one explicitly ignored read-only host observation. |
| Desktop Vitest and TypeScript | 0 / 0 | 17 files and 24 tests passed; existing React `act(...)` warnings remain; TypeScript emitted no diagnostics. |
| `python3 scripts/verify_dependency_boundaries.py` | 1 | Existing forbidden transitive `windows-sys` edges from core, IPC, and pipeline remain release blockers. |
| Full workspace Clippy with `-D warnings` | 101 | Existing repository-wide lint failures remain; touched production adapter Clippy passes separately above. |

## Fail-closed governance and non-mutation

| Command | Exit | Material result |
|---|---:|---|
| `python3 -m unittest tests.governance.test_recovery_validators -v` | 0 | 12 passed. |
| `python3 scripts/verify_traceability.py` | 0 | 136 mapped exactly once; all 98 Linux-1.0 requirements remain blocked. |
| `python3 scripts/verify_evidence.py --production` | 1 | 148 historical/quarantined schema-v1 records; zero v2 candidates. |
| `python3 scripts/validate_pack.py` | 1 | Expected dirty/unsealed manifest mismatches; read-only. |
| `python3 scripts/release_gate.py --target linux-1.0` | 1 | Expected fail-closed result: pack failure, 98 blocked requirements, and quarantined legacy evidence. |
| `python3 scripts/run_gate.py --phase P11` | 2 | Quarantined; explicitly reported no files changed. |

The tracked-or-unignored workspace inventory/content, Git diff, and Git status
hashes were measured immediately before and after exactly those six governance
commands. Each pair was identical:

- workspace inventory/content:
  `22ef5adc18c3dbb31261cdf825b41fc6e0dc16ff56892b85af2efb6d7e5b54ef`;
- Git diff:
  `18d1e2544d525ff00ca251bc7100a7969b1cab02e5e1894ab325cb474c926334`;
- Git status:
  `6aea4a39cdc3c77f54920214fbccce199062df2906befa80fafd1ab2ce1c688a`.

Protected files and governance catalogs remained exact:

- `.deb`: `77733e7ce7c0ace6820c928a6827e0056d2d18490e166927217351724da419c0`;
- `BUILD_LOG.md`: `0c52da5778bf9b8f6f3c843ff0cb0db6e9bc6d206a63d1c898ae5608fba85112`;
- traceability: `ffe19846bf4710ee2ce015b6081ceeb87fff4bc1acac68ac281e738579e59d0f`;
- empty evidence index: `dfb5bd72bfb8f63d88b9f8d4027304d491b737c19a584bc293aff2b97923adc5`.

These hashes delimit the validator window. The permitted baseline and state
bookkeeping performed afterward changes the final workspace/diff hashes but
does not alter any protected file or governance catalog.

## Scoped cleanup

No Narrivy daemon, `pw-record`, or `transcribe-cli` process remained after the
checks. Controlled sender tests used anonymous socket pairs and created no
payload, audio, transcript, or portal-session file. A final host cleanup removed
only user-owned top-level `/tmp/narrivy-*` and `/tmp/narrivy_*` logs/scratch
lists left by earlier recovery diagnostics, plus the separately inspected
24 MiB `/tmp/narrivy-libei-research` source-research directory. The repository,
Cargo cache/target tree, model cache, protected package, evidence tree, and user
documents were not cleanup targets. Those temporary entries were deleted from
`/tmp` and are not recoverable through the repository.

The successful protocol-peer test observes exactly these negotiated interfaces:
`ei_callback`, `ei_connection`, `ei_seat`, `ei_device`, `ei_pingpong`, and
`ei_text`; sender context; and request order `start`, `text`, `frame`, `stop`.
The payload comparison stays inside the test process and is reported only as a
boolean and byte count. Negative tests cover a missing text capability, a peer
negotiating `ei_text` above Narrivy's advertised maximum, clean pre-queue
cancellation with zero payload request, and debug-mode rejection. EIS transport
loss before dispatch remains `TransportFailed`; lifecycle and daemon tests prove
bounded close, Idle state, and no manufactured portal-revocation claim.

## Independent review

Independent review returned **PASS** after rework. The reviewer independently
verified that pre-dispatch EIS loss remains `TransportFailed` and closes without
a false `Revoked` event, over-versioned interface negotiation is rejected, and
the `DispatchUncertain` contract says progress may have occurred rather than
claiming that it did. The review reran the five sender tests, the combined portal
sweep, daemon portal tests in all three targets, formatting, diff checking, and
touched-adapter Clippy. It made no edits and performed no native interaction.

## Explicit non-claims

No portal prompt, native EIS connection, external input event, microphone run,
target window, transcript log, evidence record/index, requirement status,
manifest reseal, package, commit, or push occurred. The direct driver and
permission/recovery services remain uncomposed; sender and delivery capability
remain false. All 98 Linux-1.0 requirements and the release remain blocked.
