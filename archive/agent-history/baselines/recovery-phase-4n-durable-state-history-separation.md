# Recovery Phase 4N-A — durable recovery state and History separation

Status: controlled implementation complete and independently reviewed PASS for
the scoped phase. Release qualification remains blocked. This is non-release
recovery work only.

## Objective

Repair the accepted persistence/privacy boundary by placing the bounded active
session journal in private durable XDG state, pruning it at startup, preventing
legacy History APIs from reading it, and giving the live desktop an exact
transaction-scoped recovery-result request.

## Initial independently verified defects

- `narrivyd` opens the recovery journal below the volatile runtime directory.
- `GetHistory` and `GetHistoryEntry` read that recovery journal directly.
- the desktop post-delivery path lists every recovery row and chooses the
  newest entry instead of using the event transaction identity.
- retention removes expired delivered/recovered entries only and does not cap
  all records strictly.
- startup does not invoke journal retention.
- the History screen presents recovery rows as user History even though a
  secure optional History store is not composed.

## Guardrails

No microphone operation, evidence/index or requirement change, manifest
reseal, package operation, protected-file edit, commit, push, or Phase 2C work
is authorized by this phase. Optional History remains disabled.

## Implemented boundary

- `narrivyd` now composes its recovery journal below the private durable state
  root instead of the runtime directory.
- the production journal serializes mutation plus retention and enforces the
  15-minute TTL/100-entry cap on startup, access, and mutation; raw adapter
  fixtures retain explicit synthetic-time control.
- new transactions use OS-CSPRNG 128-bit opaque IDs and atomic no-clobber
  creation with bounded collision retries before publishing session state.
- legacy History commands remain decodable but fail closed without reading
  recovery storage; exact current-session recovery uses the additive
  `GetRecoveryEntry { txid }` request.
- the desktop binds transcript-bearing recovery responses to the requested
  transaction and successful terminal state, and reports mismatch without
  rendering payload or identifiers.
- qualification daemon launchers force a private 0700 `XDG_STATE_HOME`, retain
  it only across intentional scenario restarts, and remove it with the
  scenario temporary tree.
- the obsolete P6 runner is globally quarantined: direct execution and imported
  `main()` return 2 without legacy command execution or evidence writes.

## Controlled results so far

- journal API: 9 tests passed;
- Unix journal adapter: 18 tests passed, including cap/TTL without restart,
  concurrent pruning, and atomic duplicate rejection;
- daemon: all focused/unit/integration tests passed, including entropy failure,
  bounded collision retry/exhaustion, no-clobber preservation, startup prune,
  exact recovery lookup, and History non-access;
- focused strict Clippy for journal API/adapter/daemon: passed with warnings
  denied;
- desktop: 17 files / 28 tests passed; TypeScript passed. Existing React
  `act(...)` warnings remain outside this recovery scope;
- qualification Python sources compile in memory with no `__pycache__`.

## Independent review disposition

PASS for the Recovery 4N-A scope. No high-severity defect was found in durable
path resolution, retention locking, no-clobber admission, IPC separation,
desktop response binding, or qualification state isolation.

The review identified older non-strict capture/ASR initialization paths that
can leave a durable Pending record or a registered session slot. Strict
recovery lanes use the corrected terminalization path. This pre-existing,
medium-severity debt is not hidden by 4N-A and requires a separately bounded
phase.

Still intentionally unverified: automatic restart-to-Inbox recovery, migration
or removal of legacy runtime journals, optional History, package/version
compatibility, and native release evidence.

## Validation record

| Check | Exit | Material result |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | workspace formatted |
| `cargo check --workspace --all-targets --all-features --locked` | 0 | no diagnostics |
| `cargo test --workspace --all-targets --all-features --locked` | 0 | all non-ignored workspace tests passed; no microphone/native qualification |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | warnings denied |
| `python3 scripts/verify_dependency_boundaries.py` | 0 | 9 portable/core packages checked |
| desktop Vitest | 0 | 17 files, 28 tests passed; pre-existing React `act(...)` warnings only |
| desktop TypeScript | 0 | no diagnostics |
| governance tests | 0 | 12 passed, including direct/imported P6/P9/P11 quarantine and non-mutation |
| `python3 scripts/verify_traceability.py` | 0 | 136 mapped; all 98 Linux release requirements remain blocked |
| `python3 scripts/verify_evidence.py --production` | 1 | 148 schema-v1 records quarantined; zero v2 candidates |
| `python3 scripts/validate_pack.py` | 1 | dirty/unsealed manifest rejected read-only |
| `python3 scripts/release_gate.py --target linux-1.0` | 1 | pack failure and all 98 requirements blocked |
| `python3 scripts/run_gate.py --phase P11` | 2 | quarantined; no files changed |
| `cargo deny check` | 1 | pre-existing desktop/Tauri advisories remain; bans, licenses, and sources passed; no exception added |

The six-command governance window used `PYTHONDONTWRITEBYTECODE=1`. Its
pre/post hashes matched exactly:

- Git status: `8e1fc9167e197aaf2c5de36a3493777fa0115800dce62e79018a6dc96743d613`;
- Git binary diff: `9b9d3d1dcb460ac08c109c67a8b5d71a39c6e497fe62ba1a95ea1f2b95cad48f`;
- tracked/unignored workspace inventory and content, excluding this
  self-referential baseline:
  `8fe2fbf4fd12939d010e13da67a6dcf49b1d5f4f546d7f609bd1322604634f47`.

Generated Python bytecode directories were inspected and removed after test
runs; none remains outside build output. The pre-existing tracked nested
`target` artifacts remain a separate repository-hygiene blocker and were not
modified by this phase.

Protected and governance hashes remain exact:

- `artifacts/releases/narrivy_0.2.3_amd64.deb`:
  `77733e7ce7c0ace6820c928a6827e0056d2d18490e166927217351724da419c0`;
- `packaging/deb/BUILD_LOG.md`:
  `0c52da5778bf9b8f6f3c843ff0cb0db6e9bc6d206a63d1c898ae5608fba85112`;
- `spec/requirements-traceability.csv`:
  `ffe19846bf4710ee2ce015b6081ceeb87fff4bc1acac68ac281e738579e59d0f`;
- `agent/evidence/INDEX.json`:
  `dfb5bd72bfb8f63d88b9f8d4027304d491b737c19a584bc293aff2b97923adc5`.

HEAD remains `ff6d864012adb1468d02c6b5f80b9ce92258fa04`. No commit,
push, evidence, requirement transition, manifest reseal, package operation, or
native input occurred.
