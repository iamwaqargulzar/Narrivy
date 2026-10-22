# Recovery Phase 4Q-C3 — isolated installed-runtime package staging

Status: controlled non-release implementation independently reviewed PASS.
Release remains blocked.

## Objective and bounded result

C3 establishes one deterministic, fail-closed way to stage the C2 resident
worker and installed-runtime manifest into a new isolated package-root tree. It
does not build a Debian archive, invoke root, modify either packaging tree, or
claim package membership, installation, provenance, or release readiness.

`scripts/stage_installed_runtime.py` requires an explicit absolute worker and
an absolute nonexistent output root. It creates exactly:

- `usr/lib/narrivy/narrivy-transcribe-worker`, mode 0755;
- `usr/share/narrivy/installed-runtime-manifest.json`, mode 0644.

The output root is mode 0700 and package directories are mode 0755. The source
must be a current-user-owned, exact-mode-0755 regular non-symlink file, bounded
to 1..256 MiB and a complete ELF64 little-endian x86-64 executable/shared
object with a valid bounded program table and executable load segment. Copying
uses an `O_NOFOLLOW` descriptor, exclusive destination creation, SHA-256 and
byte-count verification, stable source metadata, canonical JSON, and leaf-to-
root directory fsync.

Output-root creation, all writes, final publication checks, and failure cleanup
are anchored to stable directory descriptors. The stager tracks whether it
created the root, preserves concurrent collisions, rejects root/parent rename
or replacement, never recursively follows a raced pathname, and removes only
its fixed leaves and directories when identity remains established.

ADR-0053 records the trust boundary. The legacy `scripts/build_deb.sh` and both
Debian packaging trees are unchanged.

## Adversarial review and tests

Independent review found and caused repair of four pre-release defects:

1. pathname cleanup could follow a swapped parent and delete another tree;
2. an output collision between precheck and `mkdir` could be removed by cleanup;
3. a restrictive `umask` could prevent acquiring the new root for repair;
4. a renamed/replaced published root could be reported as successful, and
   intermediate directory entries were not all fsynced.

Stable parent/root handles, explicit creation ownership, `O_PATH` acquisition,
identity checks immediately before success, fixed non-recursive cleanup, and
complete directory fsync ordering close those cases. The final independent
verdict was PASS.

The packaging suite has 13 passing tests covering exact layout/schema/digest/
modes, deterministic staging, restrictive `umask`, symlink/type/mode/size/ELF
rejection, source mutation, injected write/fsync failures, collision
preservation, parent/root replacement, safe cleanup, and the exact seven-
directory fsync order.

## Actual release-worker staging observation

A fresh disposable target built the actual worker with:

```text
cargo build --release --locked -p narrivy-asr-transcribe \
  --features resident-worker --bin narrivy-transcribe-worker
```

The worker was staged into two separate roots. Both trees contained exactly two
leaves and were byte-identical:

- worker bytes: 3,690,000;
- worker SHA-256:
  `486501085511cb725425ca73bd2b5b2f838e856b27e40a31be005c6521215cfc`;
- manifest bytes: 159;
- root/worker/manifest modes: 0700/0755/0644.

The canonical manifest's digest and size matched the copied worker. The entire
temporary build and staging tree was removed and its absence asserted. This is
builder-user staging observation only, not package or installed behavior.

## Quality verification

- `python3 -m unittest tests.packaging.test_stage_installed_runtime -v`: 13
  passed.
- Workspace all-target/all-feature `cargo check`: passed.
- Strict workspace Clippy, Rust formatting, diff hygiene, and both architecture
  boundary validators: passed.
- Desktop Vitest: 17 files / 28 tests passed with existing React `act(...)`
  warnings; TypeScript passed.
- Cargo-deny bans, licenses, and sources passed; advisory qualification remains
  separately blocked.
- One full serial workspace test sweep reached the existing load-sensitive
  PipeWire microbenchmark and failed its p50 assertion at 16 microseconds while
  p99 was 20 microseconds. No threshold was changed. Ten exact isolated reruns
  passed (p50 5–6 microseconds, p99 5–17 microseconds). C3 changes no Rust or
  runtime behavior, and the independent C3 review remains PASS; the flaky
  benchmark is recorded rather than presented as a clean full-sweep pass.

## Explicit exclusions and remaining blockers

- No `.deb` construction, package metadata, dpkg membership, installation,
  upgrade/removal, systemd activation, model provisioning, root ownership,
  SBOM, signing, reproducibility, or clean-machine qualification.
- No microphone, transcript, delivery, latency/accuracy, native behavior, or
  release evidence was created.
- The protected historical `.deb` still lacks this runtime; its embedded
  version/build identity is inconsistent and its service composition defaults
  are not release-safe.
- The legacy builder still accepts missing/debug inputs and stub supply-chain
  material. It must be replaced or quarantined before disposable package
  composition.
- Model-pack license/provisioning, service selection, dependency advisories,
  tracked generated outputs, package provenance, and clean-machine validation
  remain open.
- All 98 Linux release requirements and the release remain blocked.

## Governance and identities

The final validator hashes and exact command results are recorded after the
fail-closed validation window below. The workspace-content hash excludes this
self-referential baseline file.

The final fail-closed validator window used
`PYTHONDONTWRITEBYTECODE=1`. Pre/post hashes matched exactly:

- Git status:
  `7101eb5db227752151c6bb81aa6ab0c5158702b65a5437ddecdd767e0b1879a0`;
- Git binary diff, excluding this baseline:
  `f44139ce736f8d6993c9d7ae50906f8cf03b0acee3d8b141536a74d3ded2f103`;
- tracked/unignored inventory/content, excluding this baseline and retaining
  explicit path markers for the three intentionally deleted portable IPC
  implementation files:
  `d0cceb4c70f20ddca471414df0c63eee7ed0ae3df201e158402189eb85ce8b7e`.

| Command | Exit | Material result |
| --- | ---: | --- |
| `python3 scripts/doctor.py` | 0 | host diagnostics only; no install or permission mutation |
| full governance discovery | 0 | 29 tests passed |
| `python3 scripts/verify_traceability.py` | 0 | 136 mapped; all 98 Linux release requirements blocked |
| `python3 scripts/verify_evidence.py --production` | 1 | 148 historical/quarantined v1 records; zero v2 candidates |
| `python3 scripts/validate_pack.py` | 1 | dirty/unsealed manifest rejected read-only |
| `python3 scripts/release_gate.py --target linux-1.0` | 1 | pack failure and all 98 requirements blocked |
| `python3 scripts/run_gate.py --phase P11` | 2 | quarantined; no files changed |

Protected identities must remain:

- `.deb`: `77733e7ce7c0ace6820c928a6827e0056d2d18490e166927217351724da419c0`;
- `BUILD_LOG.md`: `0c52da5778bf9b8f6f3c843ff0cb0db6e9bc6d206a63d1c898ae5608fba85112`;
- traceability: `ffe19846bf4710ee2ce015b6081ceeb87fff4bc1acac68ac281e738579e59d0f`;
- empty evidence index:
  `dfb5bd72bfb8f63d88b9f8d4027304d491b737c19a584bc293aff2b97923adc5`.

No evidence, requirement transition, manifest reseal, package, commit, push, or
release claim was created.
