# Recovery Phase 4Q-C5B1 — degraded installed-model lifecycle

Status: bounded implementation and independent review PASS. Release remains
blocked.

## Objective and bounded result

C5B1 lets an installed-mode daemon with a verified fixed-root resident runtime
serve authenticated status while its selected bundled model is explicitly
absent. The daemon retains a typed, content-free `AwaitingInstalledModel` state
and starts no resident child or fallback engine in that condition.

The model manager now distinguishes a completely absent registry-derived
digest directory from an existing installation that fails verification.
Installed startup remains fatal for invalid configuration, unknown bundled ID,
unsafe cache-root policy, incomplete/corrupt/unverifiable model state, model
I/O/JSON failures, immutable model-binding failure, missing/invalid installed
runtime, and resident-engine initialization failure. Only the exact typed
`InstalledModelMissing` observation enters degraded service.

Resident ownership is one mutex-protected lifecycle state, so initialization,
snapshot, and shutdown removal cannot expose mixed engine/identity/model states.
`GetStatus.installed_model` provides additive typed `awaiting_model`/`ready`
truth with forward-compatible unknown-tag preservation; capability strings are
only diagnostic detail. `ListPipelines` disables
strict recovery lanes because live resident readiness is false. `StartSession`
and `SelectPipeline` use the same fail-closed admission decision and reject
before journal, session, or worker effects. No model/cache mutation API was
added.

## Controlled coverage

- Model-manager tests distinguish missing, incomplete, and unknown bundled
  installations in private temporary roots.
- Daemon capability tests cover the missing-model degraded cause, preserve the
  verified installed-runtime binding fact, deny ASR readiness, and disable the
  strict pipeline.
- Lifecycle tests pin typed awaiting/ready/absent status mapping and atomic
  transition to closing before any attempted reinitialization artifact work.
- Resident admission tests reject the missing-model state with typed
  `DaemonUnavailable` outcomes.
- Existing installed-runtime/model binding tests and the typed initializer seam
  continue to cover the valid preinstalled composition path. No fixed-root
  system installation was created for this phase.
- Existing IPC endpoint lease tests cover identity-checked socket cleanup. A
  real systemd user-unit degraded lifecycle was not run because installation
  and package mutation were explicitly outside scope.

## Validation

| Command | Exit | Material result |
| --- | ---: | --- |
| `cargo test -p narrivy-model-manager --test p5_model_cache --locked` | 0 | 23 tests passed |
| `cargo test -p narrivy-ipc --locked` | 0 | 26 tests passed, including legacy absence and unknown typed lifecycle tags |
| `cargo test -p narrivy-ipc-unix --locked` | 0 | 26 tests passed, including same-user endpoint lifecycle coverage |
| `cargo test -p narrivyd --all-features --locked` | 0 | 11 library, 87 daemon, 90 mock E2E, and 91 extension tests passed; one controlled native diagnostic ignored |
| `cargo check -p narrivyd --all-targets --all-features --locked` | 0 | no diagnostics |
| focused `rustfmt --check` | 0 | changed Rust files formatted |
| `cargo clippy -p narrivyd --all-targets --all-features --locked -- -D warnings` | 0 | no diagnostics |
| `git diff --check` | 0 | no whitespace errors |

No governance/evidence/package/release command is represented as having run in
this phase. The repository remains dirty and unsealed; all 98 Linux release
requirements remain blocked.

## Changed scope

- `crates/narrivy-model-manager/src/{error,store}.rs` add only the typed absent
  observation at the existing bundled verification boundary;
- `crates/narrivy-model-manager/tests/p5_model_cache.rs` pins missing versus
  incomplete behavior;
- `crates/narrivy-ipc/src/{lib,protocol}.rs`, the Unix IPC controlled fixtures,
  and `spec/protocols/ipc.md` add the compatible typed status field;
- `services/narrivyd/src/{main,daemon/mod,daemon/policy,daemon/session}.rs` own
  degraded startup state, status/capability truth, admission, and focused tests;
- ADR-0056, this baseline, `agent/STATE.json`, `agent/DECISIONS.md`, and
  `agent/RESEARCH_LOG.md` record the bounded decision and remaining blockers.

## Explicit exclusions and remaining blockers

- No IPC mutation/import command, CLI, desktop UI, package/unit change,
  model/cache mutation, model download, evidence/index/traceability change,
  manifest reseal, commit, push, or Phase C5B2 work occurred.
- No valid fixed-root installed runtime/model pair was installed or launched.
  Native degraded systemd lifecycle, socket cleanup, upgrade/removal, and clean-
  machine behavior remain unverified.
- Package/SBOM/release provenance, licensing approval, and all native model,
  microphone, transcription, delivery, performance, and release qualification
  remain blocked.
- All 98 Linux release requirements and the release remain blocked.
