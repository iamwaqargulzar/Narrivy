# Recovery Phase 4C — Bounded resident-ASR transport

**Recorded:** 2026-08-28  
**Base/final HEAD:** `ff6d864012adb1468d02c6b5f80b9ce92258fa04`
(unchanged)  
**Status:** controlled implementation and content-free local observation
complete; non-release; all 98 Linux requirements remain blocked

## Objective and scope

Reduce avoidable peak memory and allocation work in the accepted Recovery 4B
resident-ASR request path without changing ASR semantics, model, privacy,
cancellation, or release claims. Also correct the catalog and documentation so
upstream model-dependent streaming is not confused with Narrivy's final-only
Whisper recovery lane.

Scope is limited to the resident worker's private PCM transport, its controlled
tests, the existing ADR/build notes/catalog truth, research/decision records,
and this baseline/state update. No microphone run, streaming implementation,
VAD, delivery change, evidence, requirement status, manifest reseal, package,
commit, or push is permitted.

## Implemented behavior

- The request header remains unchanged and still declares exact sample and byte
  counts under the 30-second/480,000-sample bound.
- The parent now encodes f32 PCM through one fixed 4,096-sample/16,384-byte
  transfer buffer directly to the owned nonblocking pipe under the existing
  absolute deadline and cancellation signal. It no longer constructs a second
  payload-sized serialized byte vector.
- The worker reads the same fixed-size chunks directly into the one final
  `Vec<f32>` required by the upstream final-decode API. It drains and validates
  the complete request before returning a typed invalid-audio result; it does
  not allocate a second payload-sized byte vector.
- A controlled maximum-size request crosses multiple transfer chunks and proves
  the exact protocol boundary without creating any payload file.
- The public capability remains `streaming=false`. Upstream 0.2.2 streaming is
  available only for models advertising that capability; Narrivy's installed
  Whisper tiny model and current worker protocol remain final-only.

## Allocation boundary

At the maximum accepted request, 480,000 f32 samples occupy 1,920,000 bytes.
Before this phase the parent additionally allocated about 1,920,000 bytes for
serialized PCM (plus the 28-byte header), and the worker additionally allocated
1,920,000 bytes before decoding into its final f32 buffer. After this phase each
side uses one fixed 16,384-byte transfer buffer instead. The canonical parent
`AudioChunk` and worker final f32 input remain payload-sized by design.

This is a construction-level allocation reduction, not an RSS or latency claim:
allocator behavior and native model inference dominate short exploratory timing.

## Content-free local observation

No microphone or transcript content was used or printed. With the existing
45,904,544-byte Whisper tiny GGUF, the optimized worker reached ready in
107.4 ms. Three one-second silent requests completed in 220.4, 271.3, and
196.2 ms, each returning three bytes with SHA-256
`bb0347a468d97e98a9c00e37cebec1ab930f6f1221cae0f1fbb92b07e1900ba2`.
The worker exited 0. These noisy local measurements establish protocol/model
compatibility only; they do not establish a latency improvement, accuracy,
microphone behavior, or release qualification.

## Controlled validation

| Command | Exit | Material result |
| --- | ---: | --- |
| targeted `rustfmt --check` / `git diff --check` | 0 | formatting and whitespace pass |
| `cargo test -p narrivy-asr-transcribe --features resident-worker --locked --offline -q` | 0 | 25 passed, including maximum-size chunk transfer |
| strict engine Clippy with `-D warnings` | 0 | no diagnostics |
| `cargo test -p narrivyd --features asr-transcribe-resident --locked --offline -q` | 0 | 6 + 58 + 61 + 62 passed |
| `cargo check --workspace --all-targets --all-features --locked --offline -q` | 0 | workspace compiles |
| `cargo test --workspace --all-targets --all-features --locked --offline -q` | 0 | complete workspace passes; one attended portal observation remains ignored |
| architecture catalog validator | 0 | 20 feature groups, 136 requirements, 12 pipelines, 26 engines, 66 modules, 8 runtimes |
| governance suite | 0 | 29 passed |
| dependency/source boundary validators | 0 / 0 | 9 protected packages |
| `verify_traceability.py` | 0 | 136 mapped; 98 Linux requirements blocked |
| `verify_evidence.py --production` | 1 | 148 schema-v1 records quarantined; zero v2 candidates |
| `validate_pack.py` | 1 | deliberately dirty/unsealed manifest; read-only |
| `release_gate.py --target linux-1.0` | 1 | pack failure and all 98 requirements blocked |
| `run_gate.py --phase P11` | 2 | quarantined and non-writing |

The governance/boundary/release window was non-mutating. Exact matching
before/after hashes were:

- Git status:
  `d6adc553cf7d22a210974108839003d066dd07ae8286e8d43776b384895643c0`;
- binary diff:
  `7e72c99487b47d49212764c524735a7cf18dadbe5d9c0c4b4d4f81436f96835a`;
- tracked/unignored inventory/content:
  `bc12b32b7227a433437fdbecc158b248af5086b155fc19e0e46de6dd237add69`.

The inventory command encountered the three intentionally deleted original
IPC transport source paths and hashed the same resulting inventory on both
sides. This baseline's final validation-table replacement happened afterward
and is therefore intentionally outside that measured content window; status and
diff identity still prove the validators themselves made no changes.

Protected identities remained exact:

- `.deb`: `77733e7ce7c0ace6820c928a6827e0056d2d18490e166927217351724da419c0`;
- `BUILD_LOG.md`: `0c52da5778bf9b8f6f3c843ff0cb0db6e9bc6d206a63d1c898ae5608fba85112`;
- traceability: `ffe19846bf4710ee2ce015b6081ceeb87fff4bc1acac68ac281e738579e59d0f`;
- empty evidence index:
  `dfb5bd72bfb8f63d88b9f8d4027304d491b737c19a584bc293aff2b97923adc5`.

Final cleanup found no Recovery 4 temporary directory, no untracked audio or
transcript payload candidate, and no Narrivy daemon, ASR worker, recorder, or
legacy transcribe CLI process. The process scan matched only its own command
line. Cargo target output remains an ignored reproducible build cache, not a
release artifact.

## Truth boundaries and next work

- True low-latency partial recognition needs an integrity-pinned
  streaming-capable model plus worker protocol, core orchestration, VAD/
  endpointing, cancellation, duplication, privacy, accuracy, and UI contracts.
- Repeated final-window decoding is not accepted as simulated streaming.
- The current lane remains final-only after Stop and behaviorally unqualified.
- All existing supply-chain, desktop, native-delivery, packaging, evidence, and
  release blockers remain in force.

## Rollback

Rollback is limited to the fixed-size resident protocol transfer changes and
their test, the catalog/build-note/ADR truth corrections, this baseline,
research/decision entries, and the corresponding state update. Accepted
Recovery 1–4B behavior and all protected files must be preserved.
