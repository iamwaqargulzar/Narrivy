# Recovery Phase 4Q-C5A — complete-directory offline model import

Status: bounded implementation complete; independent review PASS. Release
remains blocked.

## Objective and bounded result

C5A supplies one Linux-only local transaction that can import exact model bytes
into the existing registry-addressed cache without a network transport and
without exposing a model-manager command through the CLI. It exists solely to
precede a later, separately authorized installed-package test.

`platform-linux::model_import::import_bundled_model` accepts a model ID only
through `BundledModelRegistry`, whose contents are compiled into the exact
build and cannot be replaced by caller JSON. Linux path, ownership, mode,
publication, durability, and cleanup policy stays in the Linux adapter. The
portable model manager supplies only opaque bundled authority, canonical
metadata/digest verification, and an opaque `BundledInstalledModelBinding`.
The adapter copies an absolute safe local regular file into two exclusive
mode-0600 leaves below an atomically created mode-0700 staging directory,
verifies exact registry size and SHA-256, detects source identity or metadata
change, syncs leaves, created directory parents, staging directory, staging
parent, and destination parent, and publishes the complete directory with
descriptor-relative no-replace rename.

Exact re-import is idempotent and concurrent imports converge. An incomplete or
divergent destination is preserved and fails typed. Failure cleanup is anchored
to retained staging/parent descriptors, removes only `model.gguf`, `meta.json`,
and the owned empty staging directory, and never recursively removes a
replaceable pathname. Import-specific diagnostics are content- and path-free.
A deliberately raced nonempty staging-path replacement may remain for later
reconciliation because cleanup never recursively traverses a pathname it no
longer owns. Ordinary failures and concurrent losers remove their empty owned
staging directory. No non-Linux import API or weaker fallback exists.

No real model or ordinary cache was imported. All positive and negative
imports used generated 8-KiB fixture bytes in private disposable directories.
Installed daemon startup and the Linux immutable binder now require the opaque
bundled observation; a generic caller-registry `InstalledModelBinding` remains
explicitly non-authoritative for that composition seam. No CLI, daemon policy,
network acquisition, product behavior, package, installation, evidence,
requirement, or release state was added or enabled.

## Registry and license correction

The public import authority is deliberately narrower than the existing public
`ModelRegistry`/`RegistryEntry` acquisition scaffolds. Arbitrary validated
entries cannot enter the new public import operation. The legacy network
`acquire` transaction remains unchanged and still does not publish a complete
directory atomically.

The three bundled OpenAI Whisper-derived entries and the matching ordinary
Whisper family entries in the transcribe.cpp catalog now declare
`Apache-2.0`, matching the previously independently reviewed upstream and
conversion model cards. Distil-Whisper, Moonshine, Parakeet, and Vosk
declarations are unchanged. Repository-wide historical notices were not
rewritten; any remaining redistribution/notice conflict requires explicit
owner/legal review and remains release-blocking.

ADR-0055 records the authority, transaction, portability, cleanup, and trust
boundaries.

## Controlled coverage

The model-manager suite has 27 passing tests: 5 library tests and 22 integration
tests. Eight focused Linux adapter import tests cover:

- complete two-leaf publication with exact 0700/0600 modes;
- two-writer convergence followed by exact verification, required parent sync,
  and no staging residue;
- digest mismatch with no publication or staging residue;
- rejection of an unsafe existing cache root without changing its mode;
- descriptor-anchored destination publication despite an ancestor-path swap;
- typed destination-parent durability failure;
- descriptor-anchored cleanup that preserves an adversarial nonempty
  replacement while deleting only the original fixed leaves; and
- bundled-registry authority with content/path-safe unknown-ID rejection before
  source I/O.

The offline transport double panics if invoked; every import test completed
without invoking it. The public binder signature and compiler enforce the
bundled wrapper; five focused installed-runtime tests validate the immutable
binding mechanics, and focused daemon suites prove the installed startup
composition still compiles and passes its controlled tests.

Independent re-review verified that all six earlier blockers are closed: OS
mechanics are isolated in the Linux adapter, path traversal is descriptor-
relative and no-follow, existing roots are not mutated, publication durability
includes both rename parents, cleanup is fixed-leaf and non-recursive, bundled
authority is type-enforced through daemon composition, and import diagnostics
are content- and path-free. The result is PASS for this bounded non-release
contract only.

## Validation and truthful failures

| Command | Exit | Material result |
| --- | ---: | --- |
| `cargo test -p narrivy-model-manager --locked` | 0 | 5 library + 22 integration tests passed |
| `cargo test -p narrivy-platform-linux --lib --locked model_import::tests` | 0 | 8 import transaction tests passed |
| `cargo test -p narrivy-platform-linux --lib --locked installed_runtime::tests` | 0 | 5 immutable installed-runtime tests passed |
| `cargo check -p narrivy-model-manager -p narrivy-platform-linux -p narrivyd --all-targets --all-features --locked` | 0 | corrected portable/adapter/service boundary compiled |
| `cargo test -p narrivyd --all-features --locked` | 0 | 11 + 85 + 88 + 89 tests passed; one explicit non-release diagnostic ignored |
| `cargo clippy -p narrivy-model-manager -p narrivy-platform-linux --all-targets --locked -- -D warnings` | 0 | no diagnostics |
| `cargo test -p narrivy-asr-transcribe --lib --locked catalog::tests` | 0 | 5 catalog tests passed |
| `git diff --check` | 0 | no whitespace errors |
| last full `cargo test --workspace --all-targets --all-features --locked` before this boundary rework | 101 | stopped in `narrivy-platform-linux`: 217 passed, 2 failed, 1 ignored |
| prior C5A governance discovery (not rerun for boundary rework) | 0 | 29 tests passed |
| prior `python3 scripts/verify_traceability.py` | 0 | 136 mapped; all 98 Linux release requirements blocked |
| prior `python3 scripts/verify_evidence.py --production` | 1 | 148 quarantined v1 records; zero v2 candidates |
| prior `python3 scripts/validate_pack.py` | 1 | dirty/unsealed manifest rejected read-only |
| prior `python3 scripts/release_gate.py --target linux-1.0` | 1 | pack failure and all 98 requirements blocked |
| prior `python3 scripts/run_gate.py --phase P11` | 2 | quarantined; no files changed |

The full workspace sweep failures are outside the C5A files and are not hidden
or promoted to success:

- `pipewire_monitor::tests::latency_histogram_p99_under_load_stays_under_50us`
  reported p50 11 µs, p95 14 µs, p99 18 µs, max 22 µs, then failed its stricter
  p50 assertion; and
- `recovery::tests::bounded_stream_emits_contiguous_chunks_then_exact_safety_summary`
  returned the content-free `pipewire_spawn_failed` fixture error.

C5A does not modify either test or its Linux capture/latency implementation.
They remain truthful workspace-quality failures for subsequent triage.

## Changed scope

- `crates/narrivy-model-manager/src/{error,lib,registry,store}.rs` add the safe
  unknown-ID outcome, opaque bundled identity/installed observation, and
  portable exact verification contracts; this crate has no Linux filesystem
  import policy or new OS dependency;
- `adapters/platform-linux/src/{lib,model_import,installed_runtime}.rs` own the
  Linux import transaction and require the bundled observation at the immutable
  installed-runtime seam;
- `adapters/platform-linux/Cargo.toml` and `Cargo.lock` use already-present
  Linux descriptor primitives in the Linux adapter;
- `services/narrivyd/src/main.rs` resolves installed startup through the
  bundled registry and opaque bundled observation;
- `crates/narrivy-model-manager/tests/p5_model_cache.rs` pins the corrected
  bundled Whisper license;
- `crates/narrivy-model-manager/seed/registry.json` and
  `engines/transcribe-cpp/src/catalog.rs` correct only the directly conflicting
  ordinary Whisper license declarations;
- ADR-0055, this baseline, `agent/STATE.json`, `agent/DECISIONS.md`, and
  `agent/RESEARCH_LOG.md` record the narrow result and open blockers.

## Governance and protected identities

Governance was not rerun for the architecture-boundary rework, by principal
direction; the preceding C5A read-only window remains recorded above and no
evidence, traceability, pack, or gate file was changed in this rework. Those
prior hashes are not reused as if they described the post-rework workspace.

Protected identities remain exact:

- `.deb`: `77733e7ce7c0ace6820c928a6827e0056d2d18490e166927217351724da419c0`;
- `BUILD_LOG.md`: `0c52da5778bf9b8f6f3c843ff0cb0db6e9bc6d206a63d1c898ae5608fba85112`;
- traceability: `ffe19846bf4710ee2ce015b6081ceeb87fff4bc1acac68ac281e738579e59d0f`;
- empty evidence index:
  `dfb5bd72bfb8f63d88b9f8d4027304d491b737c19a584bc293aff2b97923adc5`.

## Explicit exclusions and remaining blockers

- No network access, model download, real cache import, package build/install,
  service lifecycle action, evidence/index write, requirement transition,
  manifest reseal, protected-file change, commit, push, or release claim
  occurred.
- No source signature or external provenance authority exists in this slice.
- Model redistribution, complete notices, SBOM/package membership, canonical
  release model selection, and download/update policy require owner/legal and
  architectural decisions.
- Standard bundled Whisper is final-only in the current catalog. It does not
  satisfy the product's streaming-model path or native qualification needs.
- Installed package, upgrade/removal/rollback, clean-machine, native model,
  microphone, transcript, delivery, latency, accuracy, RSS, thermal, and
  failure-recovery behavior remain unverified.
- All 98 Linux release requirements and the release remain blocked.
