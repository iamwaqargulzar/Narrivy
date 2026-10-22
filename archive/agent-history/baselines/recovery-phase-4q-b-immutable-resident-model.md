# Recovery Phase 4Q-B — immutable resident model descriptor

Status: controlled non-release implementation independently reviewed PASS.
Release remains blocked.

## Objective

Remove the resident worker's dependency on a mutable model pathname. Bind one
explicit model identity to immutable bytes before IPC admission, reuse that
identity across worker generations, and make every strict readiness and
admission decision require both worker and model bindings. Package and model-
manager provenance are deliberately deferred to Recovery 4Q-C.

## Implemented bounded scope

- `adapters/process-linux` now binds an absolute, current-user-owned regular
  data artifact by exact SHA-256 and byte size. It rejects symlinks, unsafe
  set-ID or group/world-write modes, size/digest drift, identity changes and
  artifacts larger than 4 GiB.
- The adapter copies verified bytes once into a mode-0400 memfd sealed against
  write, grow, shrink, future write, execute-mode change and seal changes. One
  narrowly selected high-numbered descriptor may cross the otherwise close-
  on-exec worker boundary; duplicate data admission fails closed.
- Resident private protocol version 4 requires an exact digest, size and
  `/proc/self/fd/<n>` model argument. The worker rejects ordinary paths and
  independently checks descriptor type, exact mode, complete seal set, size,
  SHA-256 and GGUF identity before strict CPU model load.
- The resident engine owns one digest-bound model snapshot and reuses it for
  every recovery generation. Model-binding failures are typed and never fall
  back to a raw path, sidecar, alternate model or mock engine.
- `narrivyd` parses explicit model path/digest/size, binds and handshakes before
  accepting IPC, and retains worker and model binding state separately.
  Status, pipeline listing, selection, session admission and recovery use that
  stored snapshot. Strict readiness requires both bindings to be controlled.
- Capability output distinguishes controlled model identity from unavailable
  installed-package/model-manager provenance. It does not promote the current
  existence-only model-manager `Installed` state.

ADR-0050 records ownership, data flow, performance cost, rejected alternatives
and the 4Q-C boundary.

## Controlled real-model diagnostic

The final diagnostic rebuilt the resident worker and copied it into an
isolated mode-0700 directory because the ordinary Cargo output was mode 0775
and was correctly rejected as group-writable by the controlled-artifact
policy. The controlled worker identity was:

- SHA-256:
  `502be41c39e1326d93fb37b143ed6a951a130c59d1bc4a87071686291dc469ed`;
- size: 14,864,336 bytes;
- mode: 0700.

The explicit Moonshine streaming canary identity was:

- SHA-256:
  `930e4622ad3a24158b91406c30c977fa6a26b34cb32d6ac3e57cfb23383a869e`;
- size: 50,462,816 bytes;
- source mode: 0600.

The ignored exact diagnostic loaded the native CPU model through the inherited
sealed descriptor, removed only its isolated source copy, killed the first
worker, observed `RestartRequired`, recovered a new worker from the same
retained snapshot, observed `Ready`, and completed bounded shutdown. It exited
0. The controlled worker file/directory were then explicitly removed; no
Narrivy worker, daemon, recorder or transcribe CLI process and no
`/tmp/narrivy-4qb-*` directory remained.

This was content-free model-load/lifecycle observation. It exercised no
microphone, audio decode, transcript, delivery, UI, evidence or release path.

## Verification summary

- Process adapter: 19 active tests passed; one explicit helper diagnostic
  remained ignored.
- Resident engine with resident-worker feature: 48 library tests passed and
  one controlled real-model diagnostic remained ignored during the ordinary
  suite; two worker parser/binding tests passed.
- Narrivyd resident-feature suites passed: 11 library, 83 binary, 86 mock-IPC
  and 87 extension-command tests; the explicit timing diagnostic remained
  ignored.
- Full workspace all-target/all-feature check and test sweep passed.
- Workspace strict Clippy, formatting, diff hygiene and both architecture-
  boundary validators passed.
- Desktop Vitest passed 17 files / 28 tests; TypeScript passed. Existing React
  `act(...)` warnings remain visible and were not suppressed.
- `cargo deny check` exited 1. Bans, licenses and sources pass, but advisories
  fail on the existing GTK3/Tauri dependency graph, including unmaintained GTK3
  bindings, GLib unsoundness, quick-xml denial-of-service vulnerabilities,
  `time` stack exhaustion, and unmaintained transitive crates. No ignore or
  exception was added.

## Independent review

The independent review checked that no production resident worker receives a raw model
path, the worker requires the exact descriptor argument order, only one data
descriptor crosses the close-on-exec boundary, model and executable bindings
are independently represented, and all strict API surfaces require both.
Negative tests cover malformed digest/size, symlink and unsafe modes, source
mutation/removal, duplicate inheritance, seal/mode drift, ordinary pathname
arguments, unavailable model binding, inherited-descriptor leakage and worker
restart. The initially missed worker-only live-readiness check was corrected to
require `bindings_ready()` before final validation. A final post-review
regression also proved that an intentionally inheritable unrelated parent
descriptor remains closed while exactly the selected immutable model
descriptor crosses the worker boundary. No remaining 4Q-B defect was found.

## Residual risk and exclusions

- Controlled digest binding is not installed package, model pack, SBOM or
  release-artifact provenance. Recovery 4Q-C must compose a reviewed manifest
  resolver without weakening this descriptor contract.
- The sealed copy consumes page cache/memory proportional to model size. Large-
  model memory/disk/residency budgets and clean-machine behavior are unqualified.
- Process groups do not contain adversarial `setsid`; cgroup/systemd abrupt-
  death qualification remains open. OS core-dump/swap hardening remains open.
- Native microphone, accuracy, partial stability, latency, power, packaging,
  upgrade/uninstall and delivery behavior were not exercised or claimed.
- All 98 Linux requirements and the release remain blocked.

## Governance and protected identities

The final fail-closed validator window used `PYTHONDONTWRITEBYTECODE=1`.
Pre/post hashes matched exactly; the inventory and binary-diff calculations
exclude this self-referential baseline file:

- Git status:
  `4883afce93a46fc81523763bec0099e15e2315eab33bd9d9f72f0b1cc6e520ab`;
- Git binary diff:
  `55d467d2994d2d6e618cadc7d3fcfa69a7ba116fccc9531f8689dddeafdef028`;
- tracked/unignored inventory/content:
  `ee653f3075488c97e78a940cbc51e9043b943d4b839c186eb4ce0f710861475b`.

| Command | Exit | Material result |
| --- | ---: | --- |
| `python3 scripts/doctor.py` | 0 | required host tools reported; no installation or permission mutation |
| governance unit suite | 0 | 29 passed |
| `python3 scripts/verify_traceability.py` | 0 | 136 mapped; all 98 Linux release requirements blocked |
| `python3 scripts/verify_evidence.py --production` | 1 | 148 historical/quarantined schema-v1 records; zero v2 candidates |
| `python3 scripts/validate_pack.py` | 1 | dirty/unsealed manifest rejected read-only |
| `python3 scripts/release_gate.py --target linux-1.0` | 1 | pack failure and all 98 requirements blocked |
| `python3 scripts/run_gate.py --phase P11` | 2 | quarantined; no files changed |

One ignored `boundary_policy` Python bytecode file found after the window was
explicitly removed. No `__pycache__` or `.pyc` remains outside ignored build or
dependency output.

Protected and governance identities remain exact:

- `artifacts/releases/narrivy_0.2.3_amd64.deb`:
  `77733e7ce7c0ace6820c928a6827e0056d2d18490e166927217351724da419c0`;
- `packaging/deb/BUILD_LOG.md`:
  `0c52da5778bf9b8f6f3c843ff0cb0db6e9bc6d206a63d1c898ae5608fba85112`;
- `spec/requirements-traceability.csv`:
  `ffe19846bf4710ee2ce015b6081ceeb87fff4bc1acac68ac281e738579e59d0f`;
- `agent/evidence/INDEX.json`:
  `dfb5bd72bfb8f63d88b9f8d4027304d491b737c19a584bc293aff2b97923adc5`.
