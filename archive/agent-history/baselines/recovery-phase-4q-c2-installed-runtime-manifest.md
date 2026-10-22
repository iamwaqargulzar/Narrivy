# Recovery Phase 4Q-C2 — installed runtime manifest binding

Status: controlled non-release implementation independently reviewed PASS.
Release remains blocked.

## Objective

Add one truthful installed-runtime composition without treating a path,
self-authored digest, local manifest, or successful native startup as package
or release provenance. Compose C1's verified model observation with the
existing immutable worker/model descriptor boundary and no network fallback.

## Implemented bounded scope

- The production resolver accepts only
  `/usr/share/narrivy/installed-runtime-manifest.json` and
  `/usr/lib/narrivy/narrivy-transcribe-worker`. The bounded version-1 manifest
  can declare only `transcribe.cpp`, one lowercase worker SHA-256, and one
  positive exact size.
- Manifest and worker are independently resolved beneath freshly validated
  descriptors for the same fixed root with Linux `openat2` beneath,
  no-symlink, and no-magiclink constraints. Root ownership, leaf modes,
  regular/native-ELF identity, exact size/digest, stable descriptor metadata,
  and immutable sealed snapshots are fail-closed.
- C1's `InstalledModelBinding` is accepted by the Linux adapter without a raw
  model-path API and upgraded to a sealed descriptor under its private cache
  root. Cache directories and sensitive model/metadata leaves are tightened to
  0700 and 0600 respectively during acquisition.
- The process and ASR-engine layers expose only generic
  `TrustAnchorDigestBound` state. Only the daemon's private initializer that
  consumes opaque `InstalledRuntimeBinding` and
  `InstalledModelRuntimeBinding` values together may attribute
  installed-manifest and registry-observed state.
- Installed startup requires `NARRIVY_ASR_ENGINE=transcribe-cpp`,
  `NARRIVY_TRANSCRIBE_RUNTIME=installed`, and one bounded registered model ID.
  Raw controlled worker/model path, digest, and size identity is rejected. The
  selected installed model is verified with a no-network transport, worker
  handshake and model load finish before IPC serving, and no mock/sidecar/raw
  fallback is possible.
- Capabilities separately expose local worker-manifest binding,
  registry-observed immutable model binding, unavailable worker/model package
  bindings, unavailable external release provenance, and unqualified native
  behavior.

ADR-0052 and the version-1 JSON schema record this boundary.

## Independent review and rework

The first review rejected a public engine constructor that could relabel any
generic trust-anchor pair as manifest/registry-backed. It also required an
explicit release-provenance negative capability, rejection of raw controlled
worker identity in installed mode, coherent transcribe.cpp selection, and a
newtype composition regression. The engine was reduced to generic provenance;
authority-specific attribution moved to the daemon's private newtype seam;
configuration and capabilities were made fail-closed; and regressions were
added.

The re-review found only an ADR sentence incorrectly claiming one shared root
descriptor. The ADR now truthfully states that the two fixed leaves use
independently opened and validated descriptors. The final independent verdict
was PASS with no remaining C2 blocker.

## Verification summary

- Process adapter: 21 tests passed and one explicit diagnostic was ignored.
- Model manager: 5 unit and 22 integration tests passed.
- Linux installed-runtime tests: 5 passed; the full platform suite passed in
  the final workspace run.
- Transcribe engine: 56 library tests passed, one explicit real-model
  diagnostic ignored, and 2 worker tests passed.
- Daemon focused suites passed after rework. The new main suite count is 85;
  integration inclusions passed with 88 and 89 tests respectively.
- Full workspace all-target/all-feature check and serial test sweep passed.
  An earlier concurrent run exposed one load-sensitive latency benchmark
  failure; its exact isolated rerun and the final serial workspace sweep both
  passed. No threshold was weakened.
- Workspace strict Clippy, formatting, diff hygiene, and both architecture-
  boundary validators passed.
- Desktop Vitest passed 17 files / 28 tests with pre-existing React `act(...)`
  warnings; TypeScript passed.
- Cargo-deny bans, licenses, and sources passed with duplicate-version and one
  unmatched-license-allowance warning. Advisory qualification remains under
  the existing desktop/Tauri supply-chain blocker; no ignore was added.

## Residual risk and exclusions

- The root manifest proves only local manifest-to-worker byte consistency. It
  does not prove dpkg membership, installed package identity, the protected
  Debian artifact's contents, SBOM membership, signature/build provenance, or
  release authorization.
- The model registry observation is independent of the root manifest and is
  not a normative model pack, package, license approval, or release authority.
- The current protected Debian artifact does not ship or compose this runtime.
  No package was built or altered, and no clean-machine install, upgrade,
  rollback, removal, service lifecycle, or artifact reconciliation was run.
- Native microphone, speech-onset, partial stability, accuracy, latency,
  resource, containment, delivery, and UX qualification remain outside C2.
- All 98 Linux release requirements and the release remain blocked.

## Governance and protected identities

The final fail-closed validator window used `PYTHONDONTWRITEBYTECODE=1`.
Pre/post hashes matched exactly; the inventory/content calculation excludes
this self-referential baseline file:

- Git status:
  `76442cffdd13a634068f2a0710f1adeb17a6f25adb791ab8b14e3ae55bd42065`;
- Git binary diff:
  `b8b87821d8b0bdf30ab262f6959af4e0b4720f4e5c548fec66f1baaf9aef45ad`;
- tracked/unignored inventory/content:
  `feea93c903f4bd57d0bcf929dc8597774fb23cdc01603060188f839035cbc9ac`.

| Command | Exit | Material result |
| --- | ---: | --- |
| `python3 scripts/doctor.py` | 0 | host tools reported; no install or permission mutation |
| governance unit suite | 0 | 29 passed |
| `python3 scripts/verify_traceability.py` | 0 | 136 mapped; all 98 Linux release requirements blocked |
| `python3 scripts/verify_evidence.py --production` | 1 | 148 historical/quarantined schema-v1 records; zero v2 candidates |
| `python3 scripts/validate_pack.py` | 1 | dirty/unsealed manifest rejected read-only |
| `python3 scripts/release_gate.py --target linux-1.0` | 1 | pack failure and all 98 requirements blocked |
| `python3 scripts/run_gate.py --phase P11` | 2 | quarantined; no files changed |

Protected and governance identities are expected to remain exact:

- `artifacts/releases/narrivy_0.2.3_amd64.deb`:
  `77733e7ce7c0ace6820c928a6827e0056d2d18490e166927217351724da419c0`;
- `packaging/deb/BUILD_LOG.md`:
  `0c52da5778bf9b8f6f3c843ff0cb0db6e9bc6d206a63d1c898ae5608fba85112`;
- `spec/requirements-traceability.csv`:
  `ffe19846bf4710ee2ce015b6081ceeb87fff4bc1acac68ac281e738579e59d0f`;
- `agent/evidence/INDEX.json`:
  `dfb5bd72bfb8f63d88b9f8d4027304d491b737c19a584bc293aff2b97923adc5`.

No microphone, audio, transcript, ordinary payload log, evidence, requirement
transition, manifest reseal, package build, commit, push, or release claim was
created.

One ignored `boundary_policy` Python bytecode file created by a validator
subprocess was explicitly removed afterward. No `__pycache__` or `.pyc`
remains outside ignored build/dependency output.
