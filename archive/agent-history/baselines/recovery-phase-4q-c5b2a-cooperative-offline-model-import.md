# Recovery Phase 4Q-C5B2A — cooperative offline-model import

Status: bounded implementation and independent final review PASS. Release
remains blocked.

## Objective and bounded result

C5B2A strengthens the existing Linux-only bundled offline-model import with a
caller-owned cancellation token, one absolute cooperative deadline, explicit
pre-publication cleanup confirmation, and truthful post-publication outcomes.
It deliberately adds no daemon IPC command, CLI/UI surface, product lifecycle
transition, network transport, or package behavior.

Copy and staged verification use fixed 256 KiB chunks and control checks around
I/O. Partial writes are handled explicitly. The portable model-manager binding
pass uses the same cooperative budget and bounded hash checks. The completed staged model is
reread and rehashed after its leaf sync. Descriptor-relative
`RENAME_NOREPLACE` remains the irreversible publication point, preceded by the
last cancellation/deadline check.

Cancellation or expiry before publication returns a clean typed outcome only
after fixed-leaf stage removal and staging-parent sync succeed. Cleanup failure
is distinct. After publication, cancellation and the deadline are not observed;
the destination is retained and failures in parent durability or exact
verification are reported as committed-but-unconfirmed typed failures. The
documented deadline is cooperative because a blocking filesystem syscall cannot
be preempted by this synchronous primitive.

## Controlled coverage

- already-cancelled and already-expired imports leave no stage or destination;
- cancellation during copy, staged reread, and the final pre-publication
  checkpoint removes and syncs the owned stage and never publishes; expiry
  during copy exercises the same confirmed-cleanup path;
- cancellation immediately after publication returns success after durability
  and verification, retaining the exact destination;
- a forced cleanup-parent sync failure cannot be mislabeled clean cancellation;
- cancellation deterministically wins when cancellation and expiry coincide;
- a forced destination-parent sync failure reports post-publication durability
  failure while retaining the published directory;
- forced post-publication digest failure remains a verification failure and
  retains the destination even when cancellation is concurrently requested;
- existing private-mode, source/destination race, digest mismatch, concurrent
  import, bounded metadata, exact-directory, and redacted-error coverage remains
  in place.

All fixtures use private temporary roots and synthetic bytes. No real model or
user cache was read, imported, mutated, or retained.

## Validation

| Command | Exit | Material result |
| --- | ---: | --- |
| `cargo fmt --all -- --check` | 0 | workspace Rust formatting passes |
| `git diff --check` | 0 | no whitespace errors |
| `cargo test -p narrivy-platform-linux --lib --locked model_import::tests -- --test-threads=1` | 0 | 21 focused tests pass |
| `cargo check -p narrivy-platform-linux --all-targets --locked` | 0 | no diagnostics |
| `cargo clippy -p narrivy-platform-linux -p narrivy-model-manager --all-targets --locked -- -D warnings` | 0 | no diagnostics |
| `cargo test -p narrivy-model-manager --locked -- --test-threads=1` | 0 | 7 unit and 23 integration tests pass |
| `cargo test -p narrivy-platform-linux --lib --locked -- --test-threads=1` | 0 | 240 passed; one read-only native portal observation ignored |
| `cargo check --workspace --all-targets --all-features --locked` | 0 | no diagnostics |
| `python3 scripts/verify_dependency_boundaries.py` | 0 | dependency boundaries pass for 9 portable/core packages |

The first independent review returned REWORK for an uncontrolled second binding
hash and incomplete cleanup reporting. Those findings were corrected. Final
independent re-review returned PASS after repeating the model-import suite three
times. No governance/evidence/package/release command is represented as having
run in this phase.

## Changed scope

- `adapters/platform-linux/src/model_import.rs` contains the Linux-only control,
  chunked copy/reverification, commit-aware result, cleanup, and deterministic
  tests;
- `crates/narrivy-model-manager` adds typed cooperative installed-verification
  control and bounded-hash coverage used by the idempotent/concurrent-loser
  binding pass;
- ADR-0057, this baseline, `agent/STATE.json`, `agent/DECISIONS.md`, and
  `agent/RESEARCH_LOG.md` record the bounded design and remaining work.

## Explicit exclusions and remaining blockers

- No authenticated import IPC, single-flight admission, daemon worker,
  awaiting-to-ready transition, resident initialization, CLI, desktop UI,
  service/package lifecycle, or C5B2B work occurred.
- No hard wall-clock syscall deadline is claimed.
- No model/cache mutation outside isolated test directories, native model run,
  evidence/index/traceability change, manifest reseal, package, installation,
  commit, or push occurred.
- Installed systemd lifecycle, upgrades/removal, clean-machine validation,
  licensing/provenance/SBOM approval, and all release qualification remain
  blocked. All 98 Linux release requirements remain blocked.
