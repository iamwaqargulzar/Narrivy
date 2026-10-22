# Recovery Phase 4N-B — session worker terminalization ownership

Status: controlled implementation independently reviewed PASS. Release remains
blocked.

## Objective

Prevent every admitted daemon session worker exit—including non-strict capture
and ASR failures or a panic—from leaving a durable Pending transaction or a
dead in-memory session slot.

## Implemented scope

- one RAII cleanup owner spans every strict and non-strict worker return and
  unwind path;
- expected capture, fixture, model, runtime, ASR, and cancellation paths write
  typed terminal state before slot cleanup;
- ASR error events now make durable failure state agree with Failed/Idle
  events;
- live state-machine sequence is synchronized before every stage event, so
  propagated errors keep monotonic terminal sequences;
- cancellation-shaped ASR events require both the caller token and typed
  `cancelled` code;
- a shared caller/worker strict-permit lease keeps admission closed through
  both registered-worker cleanup and rejected-spawn cleanup;
- missing admitted journal records fail closed during finalization;
- the cleanup fallback changes only Pending records and cannot overwrite a
  legitimate recoverable or terminal result.

## Controlled tests

- full `narrivyd` all-feature suite passes: 11 library, 74 daemon, 77 mock
  end-to-end, and 78 extension/integration tests; the one explicitly controlled
  native timing diagnostic remains ignored;
- deterministic new cases cover invalid fixture input, missing model, worker
  unwind, session-slot removal, zero live workers, terminal failure codes, and
  missing-record rejection, monotonic propagated failure, typed ASR
  cancellation, false-terminal-event rejection, and spawn-failure permit
  ordering;
- targeted formatting and `git diff --check` pass.

## Independent review

The first independent review found stale terminal event sequence ownership,
false terminal events after journal failures, incorrect ASR-cancellation
classification, and a strict-permit cleanup race. Those findings were repaired
and covered deterministically. A second review found a remaining spawn-failure
permit-release race; the caller/worker shared lease and its ordering test close
that gap. Final independent rereview disposition: **PASS** for the bounded
Recovery 4N-B scope.

## Explicit residual risk

Legacy non-strict `capture_pipewire` remains a temp-WAV helper without the
strict fileless capture cancellation/reaping design. It is not release
qualified and requires its own replacement/disablement phase. No native input
was used here.

## Guardrails

No evidence/index or requirement change, manifest reseal, package operation,
protected-file edit, commit, push, or native observation is permitted.

## Final validation

- `cargo fmt --all -- --check`: exit 0;
- `cargo check --workspace --all-targets --all-features --locked`: exit 0;
- `cargo test --workspace --all-targets --all-features --locked`: exit 0;
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`:
  exit 0;
- `python3 scripts/verify_dependency_boundaries.py`: exit 0, nine portable/core
  packages accepted;
- `git diff --check`: exit 0;
- `cargo deny check`: exit 1 because existing desktop/Tauri transitive
  advisories remain; bans, licenses, and sources pass. No exception was added.

The final fail-closed governance validator window and protected-file hashes are
recorded after the state/report update. No native input or product-delivery
claim is part of this phase.

## Fail-closed governance and non-mutation

- governance suite: exit 0, 29 tests passed;
- `verify_traceability.py`: exit 0, 136 mapped, all 98 Linux 1.0 requirements
  gated/blocked;
- `verify_evidence.py --production`: exit 1, 148 schema-v1 records remain
  historical/quarantined and there are zero v2 candidates;
- `validate_pack.py`: exit 1 on the intentionally dirty/unsealed manifest;
- `release_gate.py --target linux-1.0`: exit 1 with all 98 requirements blocked;
- `run_gate.py --phase P11`: exit 2, quarantined and non-writing.

The tracked-or-unignored validator measurement was identical before and after:

- Git status hash:
  `e9762ead495b13b61d9001e1bd719d2b12ba02226cd8acf08de6e22e7492004b`;
- Git diff hash:
  `10a1b91fec2116e4da2b0f4c6e087f51f8a278d9e922fc4c72db41167acb93be`;
- workspace inventory/content hash, excluding this self-referential report:
  `53c8904dd8298d270b20001f4ef8241649f4b3847014d2cf3cf89dc12428960a`.

One ignored generated `scripts/__pycache__/boundary_policy.cpython-312.pyc`
was removed after validation; no `__pycache__` directory remains. It was not
part of the tracked-or-unignored workspace measurement.

Protected and governance identities remain exact:

- release `.deb`:
  `77733e7ce7c0ace6820c928a6827e0056d2d18490e166927217351724da419c0`;
- `packaging/deb/BUILD_LOG.md`:
  `0c52da5778bf9b8f6f3c843ff0cb0db6e9bc6d206a63d1c898ae5608fba85112`;
- traceability catalog:
  `ffe19846bf4710ee2ce015b6081ceeb87fff4bc1acac68ac281e738579e59d0f`;
- empty evidence index:
  `dfb5bd72bfb8f63d88b9f8d4027304d491b737c19a584bc293aff2b97923adc5`;
- HEAD: `ff6d864012adb1468d02c6b5f80b9ce92258fa04`.

No evidence/index or requirement state was created or promoted. No manifest
was resealed, no package was built, and no commit or push was made.
