# Recovery quality-gate cleanup — non-release follow-on

**Recorded:** 2026-08-28  
**HEAD:** `ff6d864012adb1468d02c6b5f80b9ce92258fa04` (unchanged)  
**Status:** formatting, compilation, tests, and strict Rust linting pass;
dependency architecture, advisories, packaging, every requirement, and release
remain blocked

## Result

This bounded follow-on removed the failures surfaced by the repository's
strict-Clippy command without release-state changes or product-scope expansion.
An independent recheck then found that three manifests which declared local
Rust lints were not inheriting the workspace Clippy policy. The Linux adapter,
Tauri desktop crate, and desktop-parent adapter now declare the same
`unwrap_used = deny`, `expect_used = warn`, and `panic = deny` policy explicitly.
The Linux adapter's five module-level unwrap exemptions were removed. Both the
configured workspace command and a command-line-forced workspace run now pass.

Test code in the lint-cleanup areas outside the 16 disclosed historical test
modules propagates typed failures or matches expected error shapes rather than
using `unwrap`, `expect`, or `panic`. Small production cleanups were
semantics-preserving: aliases for complex argument types, derived defaults,
standard slice construction, impossible UTF-8 branch removal, redundant cast
removal, event-bus method placement, and typed poisoned-mutex recovery in the
PipeWire jitter monitor.

Two correctness regressions exposed during review were repaired. The IPC
single-connection test now performs an authenticated request/response rather
than relying on the server's 30-second disconnect timeout; its isolated hard
10-second run completes in 0.02 seconds. The worker supervisor no longer lets a
worker drop its own `JoinHandle`: finished entries are removed and joined by a
subsequent supervisor observation/admission, and controlled tests count that
external join. Admission remains serialized against shutdown. A timed-out
restart now refuses to start a replacement until the old worker is reaped, so
worker generations cannot overlap; timeout restoration returns the completion
receiver and `JoinHandle` to the registry atomically. Deterministic contention
tests cover explicit reap and shutdown snapshots. Concurrent stop, reap, and
shutdown calls now coalesce behind one bounded per-worker reap operation; each
caller retains its own deadline, and a longer-budget shutdown can retry after a
shorter owner times out and restores the worker. The coalescer uses a condition
variable with no persistent waiter registry, and panic resolution is published
before registry removal so coalesced and late callers cannot observe false
success.

The public inherent `RetentionPolicy::from_str(&str) -> Option<Self>` signature
was restored while retaining the typed `std::str::FromStr` implementation. One
narrow `clippy::should_implement_trait` expectation documents that deliberate
compatibility overlap. It is not a panic/unwrap exemption.

One test fixture exposed by the full sweep contained one stale JSON brace after
removing a useless `format!`; the fixture was corrected and the real desktop
import rejection test now passes. The production importer was unchanged by
that correction.

`crates/narrivy-history` was the only workspace crate missing
`publish = false`. Adding it aligns the crate with the private workspace policy
and makes cargo-deny's bans and licenses checks pass. It does not select or
ignore a license and does not publish anything.

## Validation

| Command | Exit | Material result |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | Full workspace formatting passes. |
| `git diff --check` | 0 | No whitespace errors. |
| `cargo check --workspace --all-targets --all-features --locked` | 0 | Every workspace target and feature compiles. |
| `cargo test --workspace --all-targets --all-features --locked` | 0 | Full Rust sweep passes; the current-session native portal observation remains ignored. |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | 0 | Configured full workspace lint passes. |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic` | 0 | Command-line-forced policy passes for every workspace target and feature. Historical source-scoped test allowances still exist and are disclosed below. |
| `timeout 10s cargo test -p narrivy-ipc --lib server::ipc_tests::ipc_server_accept_once_handles_single_connection --locked -- --exact --nocapture` | 0 | One real authenticated request/response; test body completed in 0.02 seconds rather than timing out. |
| `cargo test -p narrivy-core worker::tests --locked` | 0 | 19 lifecycle tests pass, including admission/shutdown overlap, external completion join, timeout restart refusal, atomic timeout restoration, bounded concurrent coalescing, and typed panic publication. |
| Desktop `npm test -- --run` | 0 | 17 files and 24 tests pass; existing React `act(...)` warnings remain. |
| Desktop `npx tsc --noEmit` | 0 | No TypeScript diagnostics. |
| `python3 scripts/doctor.py` | 0 | Linux/COSMIC/Wayland development prerequisites are detected; no permission was granted. |
| `python3 scripts/check_action_pins.py` | 0 | Five action `uses` entries are pinned. |
| `python3 scripts/verify-boundaries.py` | 0 | Scanner exits zero, but ADR-0030 records its verified false negative for direct Unix coupling in protected crates. |
| `python3 scripts/verify_dependency_boundaries.py` | 1 | Target-unaware traversal still reports `windows-sys` through `tempfile`; ADR-0030 also records the real transport/journal split required. |
| `cargo deny --offline --locked check` | 1 | Bans, licenses, and sources pass; advisories fail. No advisory is ignored. |

The first online `cargo deny check` attempt emitted no result while refreshing
for more than two minutes and was interrupted. The complete cached/offline
audit above is the recorded result. Its release-blocking advisory set includes
the GTK3 binding family, `glib 0.18.5` unsoundness, `proc-macro-error`,
`quick-xml 0.38.4` CPU/memory denial-of-service vulnerabilities,
`time 0.3.45` stack exhaustion, and unmaintained `unic-*` crates.

Read-only resolution probes found that `plist 1.8.0` constrains `quick-xml` to
`^0.38.0`, while patched `time 0.3.47` requires Rust 1.88 rather than the
workspace's documented 1.85 minimum. No forced update, MSRV change, or advisory
exception was made.

This is not a claim that the source tree contains zero historical lint
allowances. Sixteen explicitly scoped `unwrap`/`expect`/`panic` test allowances
remain outside the three newly enforced manifests and require separate review.
This follow-on added no panic/unwrap exemption; it added the single
compatibility-specific `should_implement_trait` expectation described above.

## Governance and non-mutation

The final fail-closed governance window below is rerun after the implementation
and independent review. Its hashes are recorded only after final report/state
reconciliation; the workspace inventory and Git-diff hashes explicitly exclude
this report to avoid a self-referential digest.

| Command | Exit | Material result |
|---|---:|---|
| `python3 -m unittest tests.governance.test_recovery_validators -v` | 0 | 12 tests passed. |
| `python3 scripts/verify_traceability.py` | 0 | 136 requirements mapped exactly once; all 98 Linux-1.0 requirements remain blocked. |
| `python3 scripts/verify_evidence.py --production` | 1 | 148 schema-v1 records remain historical/quarantined; zero v2 candidates. |
| `python3 scripts/validate_pack.py` | 1 | Expected dirty/unsealed manifest mismatch; validator remained read-only. |
| `python3 scripts/release_gate.py --target linux-1.0` | 1 | Expected fail-closed result; pack validation and all 98 requirements remain blocked. |
| `python3 scripts/run_gate.py --phase P11` | 2 | Legacy gate remains quarantined and wrote nothing. |

The tracked-or-unignored workspace inventory/content and Git diff (both
excluding this report), plus Git status, were identical immediately before and
after exactly those six commands:

- workspace inventory/content:
  `ea9a9a2854d1850646b005b6cf08d711844cf27331a65978ddfdcf26b24923d4`;
- Git diff:
  `95186340c176fa9f71c1d9bccface734f063b3a4f8ff15889feb38b649230222`;
- Git status:
  `7e819d8a85bb4371ae9db1c3e7700b805d2c428bfabbb23a5e3ceadf71309caa`.

Protected files and governance catalogs remain exact:

- `.deb`: `77733e7ce7c0ace6820c928a6827e0056d2d18490e166927217351724da419c0`;
- `BUILD_LOG.md`: `0c52da5778bf9b8f6f3c843ff0cb0db6e9bc6d206a63d1c898ae5608fba85112`;
- traceability: `ffe19846bf4710ee2ce015b6081ceeb87fff4bc1acac68ac281e738579e59d0f`;
- evidence index: `dfb5bd72bfb8f63d88b9f8d4027304d491b737c19a584bc293aff2b97923adc5`.

## Repository hygiene and non-claims

No `narrivyd`, `pw-record`, or `transcribe-cli` process and no Narrivy audio or
transcript temporary payload remained after the final residue check. Exact
agent-created Clippy diagnostic files were removed; the remaining audio files
are pre-existing fixtures below the ignored salvage archive `code/`, not
runtime payloads.
Ordinary Cargo/npm caches are developer state, not release evidence.

The repository still tracks 4,955 files below
`apps/narrivy-desktop/src-tauri/target/`; the root-only `/target/` ignore rule
does not cover that nested directory. This is real repository-hygiene debt.
It was not mass-deleted in a lint follow-on because doing so would create a
large, independent index change that needs its own reviewed cleanup scope.

No microphone, native export, permission prompt, EIS connection, external
input event, schema-v2 evidence, evidence-index entry, requirement promotion,
manifest reseal, package, commit, or push occurred. Phase 2C runtime wiring and
native qualification were not started. All 98 Linux-1.0 requirements and the
release remain blocked.
