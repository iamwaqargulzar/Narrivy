# Recovery Phase 2C portal composition seam — startup disabled

**Recorded:** 2026-08-27  
**HEAD:** `ff6d864012adb1468d02c6b5f80b9ce92258fa04` (unchanged)  
**Status:** controlled composition seam independently reviewed; default startup,
native permission/target behavior, every requirement, and release remain blocked

## Result

The daemon now has an explicitly injected seam joining the authenticated IPC
permission contract, daemon-owned portal actor/service, real EI wire sender,
and the exact portal-attempt-plus-Inbox recovery adapter. The shipped daemon
`main` still calls `build_handler` without a portal service. It therefore cannot
request permission, open EIS, or submit portal text.

The seam adds no pipeline ID, environment switch, CLI command, desktop command,
capability promotion, evidence, or native/release claim. A future startup owner
must first provide a safely exported Tauri/Wry main-window parent handle and an
attended permission protocol under a separate ADR.

## Security, truth, and lifecycle properties

- Kernel-derived same-user authorization occurs before request parsing or
  service dispatch. The self-declared handshake role is not authorization.
- Parent identifiers are bounded, reparsed at the daemon adapter boundary, and
  never copied into errors, logs, persistence, or TypeScript.
- Permission request and revoke use absolute 25-second and two-second bounds.
- Submission is admitted only while the service is accepting, non-revoking,
  Active, has no pending permission operation, and owns a connected transport.
- Revoke closes submission admission under the service-state lock before actor
  close. A deterministic barrier test proves a later submit never reaches the
  driver.
- A post-dispatch reply timeout remains `DispatchUncertain`; the actor closes
  its authority and records a generation-bound terminal event. A deterministic
  service test proves status reconciles from Active to Idle with typed timeout
  and confirmed cleanup rather than remaining stale.
- Direct handoff remains only `SubmittedUnverified`. Every non-clean-cancel
  recovery outcome contains exactly one portal attempt and one Inbox attempt;
  core validation cannot reinterpret it as verified delivery.
- The event pump has an explicit owner and join requirement. Failed publication
  does not create an unbounded retry queue; `GetStatus` is the reconnect truth.
- `sender_available` and `delivery_qualified` remain false.

## Independent review

Independent re-review returned **PASS** after the two lifecycle races above
were corrected. It verified default startup remains disabled, same-user checks
precede parsing/dispatch, fixed errors and debug output are content-free,
submission/revoke linearization is deterministic, reply-timeout authority loss
is reconciled, and exact direct-plus-Inbox recovery cannot become `Delivered`.
The reviewer made no edits and performed no native interaction.

## Validation

| Command | Exit | Material result |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | Workspace formatting passes. |
| `git diff --check` | 0 | No whitespace errors. |
| Daemon portal tests | 0 | 27 passed in each of three compiled targets. |
| Daemon session/IPC routing tests | 0 | 14 passed in each target during independent review. |
| Linux portal/sender tests | 0 | 56 passed; one read-only native observation remains ignored. |
| Core recovery tests | 0 | 16 passed during independent review. |
| IPC tests | 0 | 39 passed during independent review. |
| `cargo check --workspace --all-targets --all-features --locked` | 0 | Every target and feature compiles. |
| `cargo test --workspace --all-targets --all-features --locked` | 0 | Full Rust workspace sweep passed. |
| `cargo clippy -p narrivyd --lib --bins --no-deps --locked -- -D warnings` | 0 | Touched daemon production code is warning-free. |
| `cargo clippy -p narrivy-platform-linux --lib --no-deps --locked -- -D warnings` | 0 | Touched Linux production code is warning-free. |
| Desktop Vitest / TypeScript | 0 / 0 | 17 files, 24 tests passed; existing React `act(...)` warnings remain; TypeScript emitted no diagnostics. |
| `python3 scripts/verify-boundaries.py` | 0 | Source boundaries pass. |
| `python3 scripts/verify_dependency_boundaries.py` | 1 | Existing forbidden transitive `windows-sys` edges from core, IPC, and pipeline remain. |
| Full workspace Clippy with warnings denied | 101 | Existing repository-wide production and test lint failures remain; the first production failures include core worker and history code outside this seam. |
| Adapter `cargo deny` bans/licenses/sources | 0 | Policy passes with existing duplicate and unmatched-allowance warnings. |
| Adapter `cargo deny --offline` advisories/licenses/sources | 0 | Cached advisory, license, and source policy passes with four existing unmatched license-allowance warnings. An online refresh attempt failed separately because the network connection reset; no online-refresh pass is claimed. |

## Fail-closed governance and non-mutation

| Command | Exit | Material result |
|---|---:|---|
| `python3 -m unittest tests.governance.test_recovery_validators -v` | 0 | 12 passed. |
| `python3 scripts/verify_traceability.py` | 0 | 136 mapped exactly once; all 98 Linux-1.0 requirements remain blocked. |
| `python3 scripts/verify_evidence.py --production` | 1 | 148 historical/quarantined schema-v1 records; zero v2 candidates. |
| `python3 scripts/validate_pack.py` | 1 | Expected dirty/unsealed manifest mismatch; read-only. |
| `python3 scripts/release_gate.py --target linux-1.0` | 1 | Expected fail-closed result with all 98 requirements blocked. |
| `python3 scripts/run_gate.py --phase P11` | 2 | Quarantined and explicitly non-mutating. |

The tracked-or-unignored workspace inventory/content, Git diff, and Git status
hashes were measured immediately before and after exactly those six governance
commands. Each pair was identical:

- workspace inventory/content:
  `c3ece06f60657b43cdd4cdf2f83d04953fbb878b736c566b57005bc550668174`;
- Git diff:
  `7f2a1c505fec5b5ac2b6f2ef88c5642c6a4fd95afbbbdff8d946c6d32c2a0764`;
- Git status:
  `cd544e88f6a30670f87fc2b6d24ec8b4c043f970fad0d769072cbc6cc3d24612`.

Protected files and governance catalogs remained exact:

- `.deb`: `77733e7ce7c0ace6820c928a6827e0056d2d18490e166927217351724da419c0`;
- `BUILD_LOG.md`: `0c52da5778bf9b8f6f3c843ff0cb0db6e9bc6d206a63d1c898ae5608fba85112`;
- traceability: `ffe19846bf4710ee2ce015b6081ceeb87fff4bc1acac68ac281e738579e59d0f`;
- evidence index: `dfb5bd72bfb8f63d88b9f8d4027304d491b737c19a584bc293aff2b97923adc5`.

## Cleanup and non-claims

No `narrivyd`, `pw-record`, or `transcribe-cli` process remained after the
checks, and no top-level `/tmp/narrivy-*` or `/tmp/narrivy_*` entry remained.
No audio, transcript, payload, portal session, or test-output file was created
by this seam. Cargo build output and caches were not deleted because they are
normal developer state, not attributable payload residue.

No portal prompt, native EIS connection, external input event, microphone run,
target window, evidence record/index, requirement state, manifest reseal,
package, commit, or push occurred. All 98 Linux-1.0 requirements and the release
remain blocked.
