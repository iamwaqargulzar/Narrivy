# Coverage Boost — Docs & Tests

> Additive supplement to `docs/architecture/`. Does not replace any existing architecture doc.
> Tracks rustdoc + test coverage improvements for public APIs and pipeline validation.

> **Status 2026-09-28: this plan was never executed as written.** No
> `tests/coverage/` directory exists (`tests/` holds `fixtures/`,
> `governance/`, `hardening/`, `packaging/`, `schema/` plus `README.md` and
> `__init__.py`). The property-test harness in §2 and the coverage artifacts in
> §3 do not exist, and no `proptest`/`quickcheck` dependency is in any
> manifest. `crates/narrivy-pipeline/tests/` contains exactly one file,
> `contracts.rs`. The rustdoc pass in §1 was partially done, and
> `crates/narrivy-core/src/` has since gained `endpoint.rs` and `recovery.rs`,
> which the ordered list in §1 omits. The Acceptance checklist below is
> therefore entirely unticked and is kept only as a record of intent.

## Scope

- Owned: `docs/architecture/*.md` (additive only) + a planned `tests/coverage/**` that was never created
- Goals:
  - Every `pub fn` / `pub struct` / `pub enum` in `crates/narrivy-core` has a `///` doc comment.
  - Property tests for pipeline validation (`crates/narrivy-pipeline`) — DAG, stream-kind, trigger conflicts, version bumps.
  - No `unsafe_code` in portable crates; background tasks have bounded shutdown.

## Baseline (2026-08-24)

- Portable core crates: `crates/narrivy-core` (7 modules), `crates/narrivy-pipeline`, `crates/narrivy-types`, `crates/narrivy-ipc`, `crates/narrivy-policy`, `crates/narrivy-compute`, `crates/narrivy-diagnostics`, `crates/narrivy-model-manager`
- Existing coverage: `cargo test` for session, journal, transcript, modules, worker, persistence (see `crates/narrivy-core/src/*.rs` inline tests)
- Gaps at start:
  - `crates/narrivy-core`: `NarrivyCore::new`, `transcribe_and_deliver`, `Journal::open/dir/read/mark_state`, `TranscriptStore::new/set_*/variants/len/is_empty`, `ModuleRegistry::new/register/deregister/set_health/get/len/is_empty`, `CancelFlag::cancel/is_cancelled`, `WorkerSupervisor::new/live_worker_count/started_at_unix_ms`, `WorkerHandle::cancel/stop`, `Migration::with_hash`, `sha256_hex`, `Store::open`, `ConfigStore::new/set_json/get_json`, `SessionMachine::new/with_session/session_id/state` missing or incomplete rustdoc.
  - Pipeline property tests: no `proptest`/`quickcheck`-style coverage for DAG invariants; only deterministic unit/contract tests in `crates/narrivy-pipeline/tests/`.

## Plan

### 1. Rustdoc pass — `crates/narrivy-core`

Add `///` for every public item without breaking signatures. Order:
1. `lib.rs` — crate doc, `CoreError`, `NarrivyCore`
2. `session.rs` — `SessionMachine::*`
3. `journal.rs` — `Journal::*`, `JournalEntry::new_pending`
4. `transcript.rs` — `TranscriptStore::*`
5. `modules.rs` — `ModuleRegistry::*`, `ModuleDescriptor` fields
6. `worker.rs` — `CancelFlag`, `WorkerSupervisor::*`, `WorkerHandle::*`
7. `persistence.rs` — `Migration`, `Store`, `ConfigStore`, helpers

Verify: `cargo doc -p narrivy-core --no-deps` and `cargo clippy -p narrivy-core -- -D missing_docs` (if enabled).

### 2. Property tests — pipeline validation

Location (never created): `tests/coverage/pipeline_properties.rs` + `crates/narrivy-pipeline/tests/` augmentation. Only `contracts.rs` exists today.

Properties:
- P1: DAG is acyclic (no self-loops, no cycles).
- P2: Every edge satisfies `slot_io(from).1 == slot_io(to).0` (stream-kind compatibility).
- P3: Singleton slots appear at most once per pipeline.
- P4: Trigger bindings are unique across registry; `validate_registry` rejects conflicts.
- P5: `version` bump-on-save enforced; downgrade rejected.
- P6: `id` matches `^[a-z0-9][a-z0-9-]{2,63}$` and `schema_version == PIPELINE_SCHEMA_VERSION`.
- P7: `optional`/`enabled` axes independent; optional node failure skips without breaking chain (planner property).
- Fuzz: malformed JSON, unknown slots, path traversal in ids (negative cases).

Harness: `proptest` or deterministic `splitmix64` seed (no extra deps if needed) — prefer `proptest` if already in workspace, else pure std.

### 3. Coverage artifacts (none created)

- `tests/coverage/README.md` — how to run coverage. **Does not exist.**
- `tests/coverage/pipeline_properties.rs` — property suite. **Does not exist.**
- Optional: `cargo llvm-cov` / `cargo tarpaulin` report at `target/coverage/`. No coverage tooling is configured.

## Acceptance

- [ ] `cargo doc` succeeds with no warnings for `narrivy-core`.
- [ ] `cargo test -p narrivy-core -p narrivy-pipeline` passes including new property tests.
- [ ] Every `pub fn` in `crates/narrivy-core/src/*.rs` preceded by `///`.
- [ ] `cargo clippy -- -D missing_docs` (where configured) is green or documented allow.

## Log

| Date | Item | Evidence |
|------|------|----------|
| 2026-08-24 | Skeleton created | this file |
| 2026-09-28 | Plan confirmed not executed; status note added | `tests/coverage/` absent; `crates/narrivy-pipeline/tests/contracts.rs` is the only pipeline integration test; no `proptest` in any manifest |

## References

- `docs/architecture/api-stability.md` — scaffold vs stable
- `docs/architecture/pipeline-engine.md` — DAG/chainable vs singleton, planner
- `docs/architecture/component-boundaries.md` — portable-core boundary
- `spec/catalogs/pipelines.json` / `modules.json` — slot vocabulary
- `crates/narrivy-pipeline/src/lib.rs` — validation logic under test
