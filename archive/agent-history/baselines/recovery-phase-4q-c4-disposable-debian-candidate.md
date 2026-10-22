# Recovery Phase 4Q-C4 — disposable Debian candidate composition

Status: controlled non-release structural implementation complete; principal
Sol review PASS. Release remains blocked.

## Objective and bounded result

C4 establishes deterministic Debian archive composition from explicit release
ELFs without retaining or installing an artifact. It does not modify the
historical package, repair the legacy builder, provision a model, enable a
service, or claim package/release qualification.

`scripts/build_deb_candidate.py` accepts explicit absolute inputs, constructs a
private package root, invokes the C3 runtime stager, builds one deliberately
non-installable archive, reads both tar streams back, compares exact membership,
modes, root ownership, and bytes, hashes the archive, and removes the whole
temporary tree before returning. Its canonical output contains only package
classification, version, archive hash/size, exact data-file count, and the
install-blocked fact.

The candidate has 14 exact data leaves: daemon, CLI, desktop, resident worker,
installed-runtime manifest, reviewed user service, desktop entry, four icons,
license, notices, and a non-release build-information record. Its only
maintainer script is an exact mode-0755 `preinst` that exits 1. No model,
post-install activation, upgrade/removal script, SBOM, signature, or release
provenance is included.

ADR-0054 records the trust boundary. `scripts/build_deb.sh` remains byte-
identical to its index entry and is not invoked.

## Fail-closed service and daemon boundary

The candidate service explicitly selects PipeWire, transcribe.cpp installed
resident mode, Inbox delivery, and the strict `linux-mic-inbox` lane. It scrubs
all raw worker/model identity variables and rejects mock, sine-fixture, null,
optional environment-file, and path-existence substitutions. Restart-on-
failure excludes configuration/model failures 1 and 2 and is bounded by the
unit start limit. The package has no automatic enable/start action.

Before archive construction, the exact supplied daemon runs in isolated mode-
0700 HOME/XDG roots and an empty model cache. It must exit exactly 1 with the
fixed content-free `installed resident runtime failed verification` diagnostic,
empty stdout, no socket, and no surviving process group. A daemon without the
resident installed-runtime feature therefore cannot be packaged by this path.
This is a negative startup probe, not successful installed behavior.

## Controlled tests and actual composition

The C4 suite has 14 passing tests and the combined C3/C4 packaging suite has 27
passing tests covering exact archive composition and
disposal, two-build determinism, release-ELF mode/type/symlink/source-identity
rejection, strict service validation, icon/legal validation, exact negative
daemon-probe behavior, injected `dpkg-deb` failure, protected-file identity,
legacy-builder non-use, and content-free canonical output.

Three independent attack reviews found and caused repair of service section
reordering/override acceptance, noncanonical control and directory acceptance,
duplicate tar members, mutable expected bytes, archive path swapping, probe-to-
package byte races, unbounded probe/tar/archive output, escaped-session
descendants, path-following staging writes, C3-to-C4 root replacement, output
symlink/hardlink overwrite, and restrictive-umask failures. The revised tests
exercise each exploit shape. Staging is dirfd/nofollow anchored, the daemon and
archive snapshots are sealed and digest bound, tar reads are pre-authorized and
stream bounded, and output is exclusively created before `dpkg-deb` receives
only its retained descriptor.

The independent reviewer verified the earlier repairs and isolated the final
two issues: an unowned archive output pathname and restrictive-umask directory
modes. Both were repaired with retained exclusive output descriptors, fixed-FD
`dpkg-deb` output, exact new-directory `fchmod`, C3 root-identity handoff, and
dedicated collision/umask tests. Two attempts to obtain a final independent
re-review were blocked by the reviewer service's automated classifier rather
than a technical result. The principal Sol audit therefore reviewed the final
diff and test results directly and records PASS only for C4's disposable
structural boundary, not installation or release qualification.

One fresh disposable Cargo target built:

```text
cargo build --release --locked -p narrivyd --features asr-transcribe-resident
cargo build --release --locked -p narrivy-cli -p narrivy-desktop
cargo build --release --locked -p narrivy-asr-transcribe \
  --features resident-worker --bin narrivy-transcribe-worker
```

Two candidate compositions from those exact inputs were byte-identical:

- archive bytes: 3,481,352;
- archive SHA-256:
  `722196c4e3da12e3d3e78bd1b06baeab7cb0ef48e9d77d2c15d0430faa5d04a2`;
- exact data leaves: 14;
- classification: `non-release-disposable-candidate`;
- install blocked: true.

Release input identities were:

- daemon: 3,405,608 bytes,
  `84f17e34f33787ec07382b60f78a65036398476fdd7c3f3079cd0c5cbed45980`;
- CLI: 772,856 bytes,
  `ea567d0928b08d75cd31ef2c91af00829548638969af4ffd2bd891cf6f6bb166`;
- desktop: 5,480,168 bytes,
  `daec58a26a5270e292cbc23ef676fd4c96a9fe2979e053a949d06f1b07105997`;
- resident worker: 3,690,000 bytes,
  `486501085511cb725425ca73bd2b5b2f838e856b27e40a31be005c6521215cfc`.

All binary input modes were 0755; canonical assets were tightened to 0644. The
temporary target, package roots, negative-probe
roots, and both archives were removed, and their absence was asserted. No
daemon, worker, recorder, or ASR child remained.

## Explicit exclusions and remaining blockers

- The archive was never retained, installed, executed as a package, enabled,
  started by systemd, or entered into dpkg state.
- The selected model ID is solely a deterministic missing-model negative. The
  live registry is not release authority and currently has no exact-digest
  streaming-capable model suitable for final provisioning.
- Canonical versioning, dependencies, SBOM/signature/provenance, maintainer
  lifecycle, model license/distribution, installation, upgrade/removal,
  rollback, and clean-machine qualification remain open.
- Native microphone, transcript, delivery, latency, accuracy, and streaming
  behavior were not exercised.
- No evidence, requirement transition, manifest reseal, protected-file change,
  commit, push, or Phase 4Q-C5 work occurred.
- All 98 Linux release requirements and the release remain blocked.

## Governance and protected identities

The fail-closed governance window used `PYTHONDONTWRITEBYTECODE=1`; its pre/post
status, binary-diff, and tracked/unignored content hashes matched exactly. The
baseline itself is excluded from the self-referential diff/content hashes.

- Git status: `f512a92c85cdcd6ccf516e8ccd92020cdd9e8d4c81225a654756467691158a73`;
- Git binary diff excluding this baseline:
  `6e8187103d5344ee2a58d5d166ff20c2e763420262dcb7fe07b850e832c70ae2`;
- tracked/unignored inventory/content excluding this baseline:
  `de964908b97de29abd15615489dbbd77dec9a9e1c369254e6988decc5333edef`.

| Command | Exit | Material result |
| --- | ---: | --- |
| `python3 scripts/doctor.py` | 0 | host diagnostics only; no install or permission mutation |
| governance discovery | 0 | 29 tests passed |
| `python3 scripts/verify_traceability.py` | 0 | 136 mapped; all 98 Linux release requirements blocked |
| `python3 scripts/verify_evidence.py --production` | 1 | 148 quarantined v1 records; zero v2 candidates |
| `python3 scripts/validate_pack.py` | 1 | dirty/unsealed manifest rejected read-only |
| `python3 scripts/release_gate.py --target linux-1.0` | 1 | pack failure and all 98 requirements blocked |
| `python3 scripts/run_gate.py --phase P11` | 2 | quarantined; no files changed |

Protected identities remain:

- `.deb`: `77733e7ce7c0ace6820c928a6827e0056d2d18490e166927217351724da419c0`;
- `BUILD_LOG.md`: `0c52da5778bf9b8f6f3c843ff0cb0db6e9bc6d206a63d1c898ae5608fba85112`;
- traceability: `ffe19846bf4710ee2ce015b6081ceeb87fff4bc1acac68ac281e738579e59d0f`;
- empty evidence index:
  `dfb5bd72bfb8f63d88b9f8d4027304d491b737c19a584bc293aff2b97923adc5`.
