# Recovery Phase 4Q-A — digest-bound resident worker

Status: controlled non-release implementation independently reviewed PASS.
Release remains blocked.

## Objective

Replace arbitrary resident-worker pathname execution and implicit daemon
cleanup with one immutable controlled executable binding, contained worker
lifecycle, truthful live capability reporting, and explicit fail-closed daemon
shutdown. Model and package provenance are deliberately deferred.

## Implemented bounded scope

- Added a controlled executable-artifact contract to `adapters/process-linux`:
  exact SHA-256/size, current-user ownership, safe mode, native ELF structural
  validation, sealed executable memfd snapshot, positional rehash, empty
  environment, no-new-privileges, parent-death, process group, cwd `/`, umask
  0077, descriptor closure and bounded descendant cleanup.
- Resident startup requires the explicit worker path, digest and size; it binds
  once and reuses one sealed handle across every recovery generation. Raw-path
  worker injection exists only inside the engine's unit-test build.
- Admission closure is serialized with recovery. Explicit shutdown is bounded,
  idempotent, rejects future recovery/recognition/streaming, sends the worker
  shutdown frame, and force-cleans the process group under the same deadline.
- Ready and Active states poll contained child liveness. Worker exit clears any
  active stream reservation and becomes RestartRequired; lifecycle uncertainty
  fails closed.
- Daemon startup binds/handshakes before serving IPC. Status, ListPipelines,
  StartSession and SelectPipeline use one stored runtime snapshot and agree for
  Ready, Active, Warming, RestartRequired, FailedClosed, missing and
  non-controlled binding states.
- Capability output distinguishes `ready:controlled-digest-bound` from
  `worker-package-binding=unavailable:not-composed`. No live claim reopens or
  rehashes the source path.
- Recovery scheduling reports whether a worker was actually spawned or state
  changed before scheduling; it never falsely says recovery started.
- Daemon shutdown removes runtime lookup, shuts supervised tasks, always shuts
  the resident engine, retries typed timeouts, then drains the event pump.
  Non-timeout errors and failed final attempts remain sticky process failures.
- The ignored Recovery 4M timing diagnostic now compiles against the same
  production path/digest/size binding instead of a production raw-path bypass.

## Verification summary

- Process adapter: 16 active tests passed; one helper entrypoint intentionally
  ignored. Strict Clippy passed.
- Resident engine: 48 resident-feature tests passed. Strict Clippy passed.
- Narrivyd resident-feature suite: 11 library, 83 binary, 86 mock-IPC and 87
  extension tests passed; the explicit real-resident timing diagnostic remained
  intentionally ignored. No-default-feature warnings check passed.
- Workspace check, strict Clippy, formatting, diff hygiene and both portable
  architecture-boundary validators passed.
- Full workspace all-target/all-feature test sweep passed in this run.
- Desktop Vitest passed 17 files / 28 tests; TypeScript passed. Existing React
  `act(...)` warnings remain visible and were not suppressed.
- `cargo deny check` remains failed on unsuppressed desktop/Tauri transitive
  advisories. Bans, licenses and sources pass. No exception was added.

## Independent review

Reviews required and verified corrections for exact memfd mode plus
no-new-privileges, full ELF validation, positional concurrent rehashing,
graceful-then-forced shutdown, Active liveness polling, API capability
consistency, truthful recovery scheduling, resident timeout retry, and sticky
shutdown-error aggregation. Final review found no remaining 4Q-A correctness
issue.

## Residual risk and exclusions

- The model is still reopened by pathname and validated only by the existing
  model contract. Immutable descriptor/digest binding is Phase 4Q-B.
- Controlled build identity is not package/SBOM/release provenance. Installed
  packaging and clean-machine qualification are Phase 4Q-C.
- Process groups do not contain an adversarial `setsid` escape; cgroup/systemd
  abrupt-death qualification remains open.
- Legacy sidecar and other production helper families remain outside this
  phase.
- No native microphone, transcript, target insertion, accuracy, latency,
  power, package, evidence or release behavior was exercised or claimed.
- All 98 Linux requirements and the release remain blocked.

## Governance and protected identities

Final governance command results and non-mutation hashes are recorded below
after the fail-closed validator window. Protected `.deb`, `BUILD_LOG.md`,
traceability and empty evidence-index identities must remain exact.

| Command | Exit | Material result |
| --- | ---: | --- |
| governance unit suite | 0 | 12 passed |
| `python3 scripts/verify_traceability.py` | 0 | 136 mapped; all 98 Linux release requirements blocked |
| `python3 scripts/verify_evidence.py --production` | 1 | 148 historical/quarantined schema-v1 records; zero v2 candidates |
| `python3 scripts/validate_pack.py` | 1 | dirty/unsealed manifest rejected read-only |
| `python3 scripts/release_gate.py --target linux-1.0` | 1 | pack failure and all 98 requirements blocked |
| `python3 scripts/run_gate.py --phase P11` | 2 | quarantined; no files changed |

The validator window used `PYTHONDONTWRITEBYTECODE=1`. The pre/post hashes
matched exactly; the inventory and binary-diff calculations exclude this
self-referential baseline file:

- Git status: `432198ef686c9aeda2290615b8c220ff92751f3c5efb635567bf368e0a952c4d`;
- Git binary diff: `ab7708189c3877869d6b934e1d2a3f345098a88cfb95e33796e02aa28ffc57c6`;
- tracked/unignored inventory/content:
  `751e52644af23fe618d999ea4fb6a68ebbbf57227c137a6d4897f9d41b05abf1`.

Two ignored `boundary_policy` Python bytecode files left by earlier technical
validation were confirmed untracked and removed explicitly afterward; no
`__pycache__` or `.pyc` remains outside ignored build/dependency output.

Protected and governance identities remain exact:

- `artifacts/releases/narrivy_0.2.3_amd64.deb`:
  `77733e7ce7c0ace6820c928a6827e0056d2d18490e166927217351724da419c0`;
- `packaging/deb/BUILD_LOG.md`:
  `0c52da5778bf9b8f6f3c843ff0cb0db6e9bc6d206a63d1c898ae5608fba85112`;
- `spec/requirements-traceability.csv`:
  `ffe19846bf4710ee2ce015b6081ceeb87fff4bc1acac68ac281e738579e59d0f`;
- `agent/evidence/INDEX.json`:
  `dfb5bd72bfb8f63d88b9f8d4027304d491b737c19a584bc293aff2b97923adc5`.
