# Recovery Phase 4Q-C5B2B2 — daemon-owned offline model provisioning

Status: implementation and required principal orchestration review complete;
independent final review and native installed observation pending. Release
remains blocked.

## Implemented boundary

- Protocol-minor-5 start/cancel commands now route only when installed mode is
  truthfully awaiting its fixed bundled model.
- One daemon lifecycle coordinator owns single-flight admission, 30-second
  cooldown, opaque operation/generation identity, cancellation, monotonic typed
  status, resident publication, and closing.
- The daemon—not the client—owns model identity/digest, no-network cache store,
  destination, ten-minute cooperative deadline, and supervised worker.
- Import, immutable binding, and resident activation share that token/deadline.
  Ready requires a matching generation; stale/closing candidates cannot publish.
- FIFO sources are opened nonblocking and rejected as non-regular. Controlled
  ancestry checks cover immutable model binding. Post-publication confirmation
  uses one bounded full hash and maps interruption to published uncertainty.
- Status and capability projection are taken from the same lifecycle lock.
- Production orchestration tests now cover worker-spawn rejection, shutdown
  during import, shutdown after candidate creation, stale completion,
  panic-guard terminalization, successful ownership transfer, and zero retained
  supervisor/resident children. Test authority is private to `narrivyd` under
  `cfg(test)`; no cross-crate feature can forge a bundled registry, installed
  runtime root, or resident worker in a production feature combination.
- A full-gate run exposed and corrected an independent Unix IPC ordering race:
  accepted Unsubscribe is now a serialized event barrier, including against a
  pre-existing registry snapshot. A deterministic regression test exercises
  the exact interleaving.

## Principal validation

| Command | Exit | Material result |
| --- | ---: | --- |
| `cargo fmt --all -- --check` and `git diff --check` | 0 | Rust formatting and whitespace pass |
| `cargo test -p narrivy-platform-linux model_import --lib --locked` | 0 | 23 tests pass, including FIFO and post-publication control |
| `cargo test -p narrivy-platform-linux --lib --all-features --locked` repeated | 0 | 11 consecutive runs; 2,673 pass and 11 expected native-observation ignores |
| `cargo test -p narrivy-ipc-unix --lib --all-features --locked` plus 20 repeats | 0 | 27 focused tests pass; 540 repeated tests pass |
| `cargo test --workspace --all-targets --all-features --locked` | 0 | complete Rust workspace passes; explicitly gated native diagnostics remain ignored |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | 0 | no diagnostics |
| `cargo check --workspace --all-targets --all-features --locked` | 0 | no diagnostics |
| `python3 scripts/verify_traceability.py` | 0 | 136 mapped; all 98 Linux requirements remain blocked |
| source/dependency boundary validators | 0 | all 9 portable/core packages pass |
| governance and packaging validator suites | 0 | 29 governance and 27 packaging tests pass |
| `cargo deny check` | 1 | existing unsuppressed desktop/Tauri transitive advisories remain; bans, licenses, and sources pass |
| `python3 scripts/verify_evidence.py --production` | 1 | expected fail-closed: 148 historical records, zero v2 candidates |
| `python3 scripts/release_gate.py --target linux-1.0` | 1 | expected fail-closed: stale pack plus all 98 requirements blocked |

The coordinator and daemon integration have deterministic coverage for
single-flight/cooldown, operation generations, idempotent exact cancellation,
monotonic revisions, publication-stage validation, cancellation/terminal
linearization, stale and closing candidate rejection, fail-closed
classifications, ready extraction, worker panic, spawn rejection, and shutdown
races.

## Truth and exclusions

The principal successor review closed the deterministic production-
orchestration coverage requested by the prior baseline and removed cross-crate
test-authority exposure. The prior independent integration reviewer could not
return a verdict because its account allowance was exhausted. No independent
final re-review is claimed.

No real model/cache provisioning, native resident activation, microphone run,
CLI/Desktop UX, evidence/index/traceability change, manifest reseal, packaging,
installation, commit, or push occurred. All 98 Linux requirements and the
release remain blocked.

## Remaining acceptance work

Before accepting C5B2B2, an independent reviewer must inspect the current
uncommitted integration diff and explicitly confirm the authority boundaries,
terminal classifications, shutdown ordering, and child/worker ownership tests.
Then perform only an explicitly authorized, content-free, disposable real-model
import/activation observation. Do not start CLI/Desktop UX or later phases
until that review and observation pass. The stale pack manifest and dependency
advisories remain separate release blockers and were not bypassed or resealed.
