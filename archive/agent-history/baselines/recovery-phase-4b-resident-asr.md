# Recovery Phase 4B — Process-isolated resident ASR

**Recorded:** 2026-08-28  
**Base/final HEAD:** `ff6d864012adb1468d02c6b5f80b9ce92258fa04`
(unchanged)  
**Status:** controlled implementation and content-free local observation
complete; non-release; all 98 Linux requirements remain blocked

## Objective and scope

Remove per-utterance model/process startup from the strict recovery path while
preserving bounded cancellation and crash isolation. The selected design is a
daemon-owned resident ASR worker, specified by ADR-0033. It loads one explicit
GGUF model once, accepts bounded in-memory mono f32 requests over a private
binary protocol, and can be killed and reaped if native code stops responding.

Scope was limited to the transcribe.cpp engine/worker, explicit daemon
composition and readiness, dependency/license records, focused CI coverage,
controlled tests, and this ADR/baseline/state record. No microphone run,
streaming/partial transcript, VAD, desktop behavior, evidence, requirement
status, manifest reseal, package, commit, or push was permitted.

## Implemented behavior

- `narrivy-transcribe-worker` links an exact-pinned transcribe.cpp Rust/native
  pair and owns the native model/session outside `narrivyd`.
- The worker verifies upstream version/ABI, actual CPU backend, 16 kHz sample
  rate, and cancellation support before reporting ready. Native logging and
  worker stderr are disabled; audio and transcript payload files are never
  created.
- A fixed, versioned 28-byte binary header carries opaque request identity,
  operation/status, payload length, and sample count. Limits are 30 seconds of
  finite mono f32 and 1 MiB of UTF-8 output. Bad magic/version/reserved fields,
  identity mismatch, truncation, non-finite samples, and oversize fail closed.
- The parent owns nonblocking stdin/stdout and the child. One absolute 120-second
  decode deadline covers request/response; cancellation, timeout, protocol, or
  I/O failure kills and reaps within two seconds. A cleanup failure supersedes
  a clean cancellation claim.
- One worker admits one request; contention is an immediate typed `asr_busy`
  failure, not an unbounded queue. An unhealthy/cancelled worker is replaced on
  the next request.
- `NARRIVY_TRANSCRIBE_RUNTIME=resident` is explicit. It requires the
  `asr-transcribe-resident` daemon feature and a non-symlink executable at
  `NARRIVY_TRANSCRIBE_WORKER`. Startup loads/handshakes the model before IPC is
  accepted. There is no mock or sidecar fallback.
- Status truthfully reports `asr.transcribe-cpp.runtime=resident-worker` while
  retaining `recovery.behavioral_qualification=unqualified`. The old one-shot
  sidecar remains only as an explicit rollback runtime.

## Supply-chain result

The initially evaluated published wrapper 0.2.1 was rejected because its
bundled native library reported 0.2.2 and the upstream pre-1.0 compatibility
gate correctly failed. That gate was not weakened. The implementation uses
exact `transcribe-cpp = 0.2.2` and `transcribe-cpp-sys = 0.2.2` lockfile entries:

- wrapper checksum:
  `ecd41195b6132c83911a05344fdc0fd58807a86ccd796176828d4bdaf9943128`;
- sys checksum:
  `67f209e316800d400b02f6a07e0dc5d96e52dae8ad849d4df080d6c2ce5e9516`.

`cargo deny check licenses bans sources` passes. `cargo deny check advisories`
remains exit 1 with 20 pre-existing desktop/Tauri transitive findings: eleven
GTK/glib findings (including RUSTSEC-2024-0429 unsoundness),
`proc-macro-error`, three XML/stack-exhaustion vulnerabilities, and five
unmaintained `unic-*` crates. No finding named the new transcribe.cpp packages;
the repository advisory gate remains blocked and no exception was added.

## Content-free local observations

No microphone or transcript content was used or printed.

1. Direct real-model worker startup reached ready in 98.5 ms. One isolated
   one-second silence request returned status 1 with the matching opaque ID,
   three output bytes, SHA-256
   `bb0347a468d97e98a9c00e37cebec1ab930f6f1221cae0f1fbb92b07e1900ba2`,
   and a 216.3 ms decode. The worker exited 0.
2. One real worker reused the same loaded model for three requests: startup
   90.5 ms; decodes 189.5, 199.8, and 243.5 ms. Every response had status 1,
   matching identity, the same three-byte count/hash, and the worker exited 0.
3. An isolated daemon started with the explicit resident feature/runtime,
   model, and worker. Strict status reported the runtime available, recovery
   ready, and behavioral qualification unqualified. No session was started.
   Controlled daemon shutdown closed the worker pipe and left no worker.

These are exploratory local measurements, not latency, accuracy, microphone,
native-platform, package, or release evidence.

## Controlled validation

| Command | Exit | Material result |
| --- | ---: | --- |
| `cargo fmt --all -- --check` / `git diff --check` | 0 | formatting and whitespace pass |
| `cargo test -p narrivy-asr-transcribe --features resident-worker --locked --offline -q` | 0 | 24 passed |
| `cargo test -p narrivyd --features asr-transcribe-resident --locked --offline -q` | 0 | 6 + 58 + 61 + 62 passed |
| strict Clippy for touched engine/daemon targets/features, `-D warnings` | 0 | no diagnostics |
| `cargo check --workspace --all-targets --all-features --locked --offline -q` | 0 | workspace compiles |
| `cargo test --workspace --all-targets --all-features --locked --offline -q` | 0 | complete workspace passes; attended portal observation remains ignored |
| governance suite | 0 | 29 passed |
| dependency/source boundary validators | 0 / 0 | 9 protected packages |
| `cargo deny check licenses bans sources` | 0 | passes; duplicate/unmatched-license warnings remain non-fatal |
| `cargo deny check advisories` | 1 | 20 pre-existing findings; release blocker retained |
| `verify_traceability.py` | 0 | 136 mapped; 98 Linux requirements blocked |
| `verify_evidence.py --production` | 1 | 148 v1 quarantined; zero v2 candidates |
| `validate_pack.py` | 1 | deliberately dirty/unsealed manifest; read-only |
| `release_gate.py --target linux-1.0` | 1 | pack failure and all 98 requirements blocked |
| `run_gate.py --phase P11` | 2 | quarantined and non-writing |

The governance/boundary/release validator window was non-mutating. Exact
matching before/after hashes for that window are recorded below. Replacing the
three placeholder labels in this untracked baseline happened afterward and is
therefore intentionally outside the workspace-content measurement boundary:

- Git status:
  `d47a3e87204d00376215aba5f46c09d487d867134c5869b89c78a57bc219e852`;
- binary diff:
  `24fcb52983df1db65ac9ee9130baec7e4637722472b753eb799403b3f047b79b`;
- tracked/unignored inventory/content:
  `207f49b878013799f1fe2f9d0fef6635280ba3baf05ad689e24129acee13e062`.

Protected identities remained exact:

- `.deb`: `77733e7ce7c0ace6820c928a6827e0056d2d18490e166927217351724da419c0`;
- `BUILD_LOG.md`: `0c52da5778bf9b8f6f3c843ff0cb0db6e9bc6d206a63d1c898ae5608fba85112`;
- traceability: `ffe19846bf4710ee2ce015b6081ceeb87fff4bc1acac68ac281e738579e59d0f`;
- empty evidence index:
  `dfb5bd72bfb8f63d88b9f8d4027304d491b737c19a584bc293aff2b97923adc5`.

The isolated `/tmp/narrivy-r4b.nykZ1N` daemon directory was inspected,
explicitly removed, and verified absent. No Narrivy daemon, worker, recorder,
or legacy transcribe CLI remained. Cargo output is an ignored reproducible
build cache, not a tracked release artifact, and was retained.

## Truth boundaries and next work

- ASR remains final-only after Stop. There is no streaming partial transcript,
  endpointer/VAD, microphone observation, end-to-end latency qualification, or
  accuracy claim.
- The worker executable is explicit and symlinks are rejected, but production
  package ownership/integrity binding and clean-machine installation are not
  implemented.
- The process owns statically linked native C/C++ code; package SBOM, license
  payload, supported-CPU behavior, upgrades/rollback, systemd ownership, and
  crash-loop/backoff require separate work and qualification.
- The single worker deliberately fails concurrent work as busy. A bounded pool
  is deferred until real concurrent-session requirements justify it.
- Existing supply-chain advisories, desktop truth/UX, privileged IPC
  authorization, durable state boundaries, native delivery, packaging,
  evidence, and all release requirements remain blocked.

## Rollback

Rollback is limited to the resident-worker feature/protocol/engine/binary,
daemon resident composition/readiness changes, exact transcribe dependency
lock and notices, focused CI additions, ADR-0033, this baseline, and the
corresponding state update. Accepted Recovery 1–4A behavior and all protected
files must be preserved.
