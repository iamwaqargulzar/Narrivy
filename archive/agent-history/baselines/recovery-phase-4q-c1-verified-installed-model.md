# Recovery Phase 4Q-C1 — verified installed-model observation

Status: controlled non-release implementation independently reviewed PASS.
Release remains blocked.

## Objective

Remove the model manager's false equivalence between path existence and an
installed model identity. Produce a bounded portable observation that can feed
the already accepted Linux immutable-descriptor binder, without claiming
package, model-pack, SBOM, release-artifact, or native provenance.

## Implemented bounded scope

- Presence is now only `PresentUnverified`. `verify_installed` opens model and
  sidecar leaves once, matches their opened identities to the observed regular
  leaves, reads exact bounded lengths with EOF checks, validates stable
  descriptor metadata, and requires exact registry identity, declared size,
  metadata, and SHA-256.
- `InstalledModelBinding` contains path, canonical digest, size, model ID,
  family, and immutable source revision. Its debug form omits the path. It is a
  portable observation, not an open descriptor or runtime/package proof.
- Acquisition enforces exact transport size and digest, reconciles a regular
  interrupted metadata staging leaf before model publication, creates the new
  sidecar staging leaf without clobber, and returns only the freshly verified
  binding. The completed model directory is still not atomically published.
- Prefetch skips only an exact verified observation and repairs tampered or
  interrupted present-but-unverified state.
- Registry ingestion is capped at 1 MiB and bounds entries, sources, languages,
  and string fields. Unknown fields, unsafe identity/path components,
  malformed hashes, unsupported sizes, malformed HTTPS/file URLs, and mutable
  or malformed Hugging Face identities are rejected. HF revisions require an
  exact 40-hex commit. Digest case is canonicalized.
- Acquisition, observation, cleanup, and budget enforcement perform fail-closed
  walks over the cache root and existing descendants and reject stable symlink
  redirection or special files. Cleanup no longer reports ignored deletion
  failures as success.

ADR-0051 records the portable/platform boundary, truthful limitations, and
deferred transaction/package work.

## Independent review and rework

The first review rejected unbounded pathname-reopened verification, unsafe
descendant cache symlinks, under-bounded registry ingestion, and atomic/install
overclaims. Those were repaired. A fresh review then rejected a non-exact
sidecar-length read, incomplete-install repair blocked by stale
`meta.json.part`, and inconsistent uppercase digest handling. Those were also
repaired. The final focused review passed all prior findings; no remaining C1
defect was reported.

## Verification summary

- Model-manager: 5 unit and 22 integration tests passed; strict all-target and
  all-feature Clippy passed.
- Full workspace all-target/all-feature check and test sweep passed.
- Workspace strict Clippy, formatting, diff hygiene, and both architecture-
  boundary validators passed.
- Cargo-deny bans, licenses, and sources passed. The advisory database refresh
  failed because the remote connection reset; no ignore or exception was
  added, and the existing desktop/Tauri advisory blocker remains open.

## Residual risk and exclusions

- Portable component walks cannot make multiple pathname operations atomic
  against a hostile process already running as the same UID. Linux runtime
  admission must still use ADR-0050's open/no-follow immutable descriptor
  snapshot and worker attestation.
- Model and metadata leaves are not yet published as one completed atomic
  directory. Install locking/reference ownership and durable fsync semantics
  remain incomplete.
- The live version-1 registry is a compatibility catalog, not the normative
  model-manifest, package, SBOM, signature, license-policy, or release authority.
- Existing Debian artifacts contain neither the resident worker nor a GGUF
  model, and the service unit does not compose resident bindings. No package,
  clean-machine, upgrade, removal, native behavior, evidence, or release
  qualification was exercised or claimed.
- All 98 Linux release requirements and the release remain blocked.

## Governance and protected identities

The final fail-closed validator window used `PYTHONDONTWRITEBYTECODE=1`.
Pre/post hashes matched exactly; the inventory/content calculation excludes
this self-referential baseline file:

- Git status:
  `a5105c76bfed81675547a4dc3072a6ce675bcd0e98fcbf8b0893205ecd2e9c3a`;
- Git binary diff:
  `c5c99d01dad897794ef6bbcfbe792a28b8ba764c46c0795c1c45ae13bd20fcb7`;
- tracked/unignored inventory/content:
  `9362dade3dddcac229a8c2ab5c0045e85968ae1f585a1c502183ba0a23d8fa45`.

| Command | Exit | Material result |
| --- | ---: | --- |
| `python3 scripts/doctor.py` | 0 | host tools reported; no install or permission mutation |
| governance unit suite | 0 | 29 passed |
| `python3 scripts/verify_traceability.py` | 0 | 136 mapped; all 98 Linux release requirements blocked |
| `python3 scripts/verify_evidence.py --production` | 1 | 148 historical/quarantined schema-v1 records; zero v2 candidates |
| `python3 scripts/validate_pack.py` | 1 | dirty/unsealed manifest rejected read-only |
| `python3 scripts/release_gate.py --target linux-1.0` | 1 | pack failure and all 98 requirements blocked |
| `python3 scripts/run_gate.py --phase P11` | 2 | quarantined; no files changed |

One ignored `boundary_policy` Python bytecode file created by a validator
subprocess was explicitly removed afterward. No `__pycache__` or `.pyc` remains
outside ignored build/dependency output.

Protected and governance identities remain exact:

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
