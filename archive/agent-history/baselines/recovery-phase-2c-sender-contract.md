# Recovery Phase 2C controlled sender contract — non-composed

**Recorded:** 2026-08-27  
**HEAD:** `ff6d864012adb1468d02c6b5f80b9ce92258fa04` (unchanged)  
**Status:** controlled sender and Inbox-recovery contracts implemented; real
sender, production composition, native qualification, and release remain
blocked

## Result

The Linux adapter and daemon now define a controlled, actor-owned contract for
one bounded portal/EIS text frame. It redacts payload diagnostics, distinguishes
cancellation before the first positive payload-bearing write from uncertainty
after it, carries one caller-owned absolute deadline, invalidates on unconfirmed
cleanup, and reconciles submission-time revocation into daemon status.

A controlled-only recovery adapter maps the direct attempt into exactly one
mandatory Inbox recovery attempt. Portable core accepts only the exact
`portal-eis-text` plus `inbox` report shape before journaling
`InboxRecovered`; it never promotes the direct attempt to `Delivered`.

The existing direct-zbus production permission driver inherits
`SenderUnavailable`. The recovery adapter has only a test constructor and no
actor implementation. No production pipeline, IPC route, CLI, desktop path, or
capability can select this contract.

## Dependency decision

No sender dependency was added. A source audit rejected direct production use
of `reis` 0.7.1 because its debug behavior, handshake breadth,
writable-readiness, flush, cancellation, and close ownership do not meet the
repository's privacy and bounded-lifecycle rules. A reviewed narrow fork or
native libei adapter remains future work.

## Validation

| Command | Exit | Material result |
|---|---:|---|
| Targeted `rustfmt --check` and `git diff --check` | 0 | Touched Rust and repository diff pass. |
| `cargo test -p narrivy-platform-linux --lib portal_eis --locked` | 0 | 39 passed; one read-only native observation remains ignored. |
| `cargo test -p narrivy-platform-linux --lib recovery --locked` | 0 | 28 passed. |
| `cargo test -p narrivy-core --lib recovery --locked` | 0 | 16 passed. |
| `cargo test -p narrivyd portal --locked` | 0 | 24 selected tests passed in each of the three compiled daemon targets. |
| `cargo check --workspace --all-targets --all-features --locked` | 0 | Pass. |
| `cargo test --workspace --all-targets --all-features --locked -q` | 0 | Full Rust workspace passed; the slow IPC accept-once test completed in 30.49 seconds. |
| `cargo fmt --all -- --check` | 0 | Pass. |
| Touched platform/daemon production Clippy with `--no-deps -D warnings` | 0 | Pass. |
| Full workspace Clippy with `-D warnings` | 101 | Existing out-of-scope test lints plus existing core worker lints remain; no gate was weakened. |
| Adapter `cargo deny --offline ... advisories licenses sources` | 0 | Pass with four pre-existing unmatched-license-allowance warnings. |
| `python3 scripts/verify-boundaries.py` and `verify_architecture_map.py` | 0 | Source boundaries pass; architecture map reports 20 feature groups, 136 requirements, 12 pipelines, 26 engines, 66 modules, and 8 runtimes. |
| `python3 scripts/verify_dependency_boundaries.py` | 1 | Existing transitive `windows-sys` violations remain for core, IPC, and pipeline. |
| Desktop Vitest and TypeScript | 0 | 17 files / 24 tests passed; existing React `act(...)` warnings; TypeScript passed. |

Independent review first returned REWORK for cleanup truth, submit-derived
revocation propagation, deadline ownership, and stale documentation. After
deterministic fixes and exact-deadline forwarding coverage, the code contract
passed; the final documentation audit required this exact result table and a
Phase 2C module-description correction. After both were applied, independent
re-review returned PASS.

These controlled tests are not platform, native-target, requirement, or release
evidence.

## Fail-closed governance window

| Command | Exit | Material result |
|---|---:|---|
| `python3 -m unittest tests.governance.test_recovery_validators -v` | 0 | 12 passed. |
| `python3 scripts/verify_traceability.py` | 0 | 136 mapped exactly once; 98 Linux-1.0 gated and blocked; 38 roadmap. |
| `python3 scripts/verify_evidence.py --production` | 1 | 148 historical/quarantined schema-v1 records; zero v2 candidates. |
| `python3 scripts/validate_pack.py` | 1 | Expected dirty/unsealed manifest mismatches; read-only. |
| `python3 scripts/release_gate.py --target linux-1.0` | 1 | Expected fail-closed result; pack failure, all 98 requirements blocked, legacy evidence rejected. |
| `python3 scripts/run_gate.py --phase P11` | 2 | Quarantined; explicitly reported no files changed. |

The tracked-or-unignored workspace inventory/content, Git diff, and Git status
hashes were measured immediately before the governance commands and after the
two read-only summary reruns. Each pair was identical:

- workspace inventory/content:
  `9e3fd5e885bb0698271a762dd42a27e5f5f7a7bfec7b3cd19040eac67fa18deb`;
- Git diff:
  `c4cd2e262a647c0a61ccf49fbe0cd50f7136beaa18420a7944a68d56b6c4ab93`;
- Git status:
  `91b450ef87855e764d68bc6cb967a6e0ccf5eaad829686646df8acc14155bc1d`.

The protected files and governance catalogs remained exact throughout:

- `.deb`: `77733e7ce7c0ace6820c928a6827e0056d2d18490e166927217351724da419c0`;
- `BUILD_LOG.md`: `0c52da5778bf9b8f6f3c843ff0cb0db6e9bc6d206a63d1c898ae5608fba85112`;
- traceability: `ffe19846bf4710ee2ce015b6081ceeb87fff4bc1acac68ac281e738579e59d0f`;
- empty evidence index: `dfb5bd72bfb8f63d88b9f8d4027304d491b737c19a584bc293aff2b97923adc5`.

## Explicit non-claims

No permission prompt, EIS connection, input event, microphone run, target
window, transcript, evidence record/index, requirement-state change, manifest
reseal, packaging, commit, or push occurred. All 98 Linux-1.0 requirements and
the release remain blocked.

## Subsequent implementation increment

ADR-0026 and `recovery-phase-2c-real-sender.md` subsequently add a real,
non-composed EI wire sender using a constrained Reis low-level codec. This
historical contract result remains accurate for its measurement window; its
old `SenderUnavailable` and no-dependency statements are superseded only by
that later record. Neither increment is native or release qualification.
