# Recovery Phase 4F — Native streaming canary

**Recorded:** 2026-08-28  
**Base/final HEAD:** `ff6d864012adb1468d02c6b5f80b9ce92258fa04`
(unchanged)  
**Status:** candidate acquisition and native content-free observation complete;
non-release; all 98 Linux requirements remain blocked

## Objective and scope

Replace assumption-driven streaming work with the smallest integrity-pinned
model that exact transcribe.cpp 0.2.2 supports natively, then exercise the
accepted resident protocol without a microphone or transcript disclosure.

Scope is authoritative model research, secure user-cache acquisition, native
worker observations, stale documentation repair, candidate catalog metadata,
ADR/research/decision/state records, and controlled validation. Live capture,
VAD/endpointer, daemon/UI composition, default selection, release model pack,
evidence, requirements, manifest, package, commit, and push are excluded.

## Candidate identity and acquisition

- model: Moonshine Streaming Tiny Q8_0, English-only, 34M parameters, MIT;
- immutable HF revision: `85ddff612fa3a2cf40b2f745abcfa90ef82f293b`;
- exact size: `50,462,816` bytes;
- SHA-256:
  `930e4622ad3a24158b91406c30c977fa6a26b34cb32d6ac3e57cfb23383a869e`;
- installed cache path:
  `~/.cache/narrivy/models/moonshine-streaming-tiny-Q8_0.gguf`;
- cache directory/file modes: 0700/0600; and
- staging `.part` was removed by atomic rename after verification.

The retained model is intentional user-cache state, not repository content or
release evidence. The engine still performs no implicit network acquisition.

## Native observations without transcript disclosure

Synthetic silence: ready in 105.3 ms, native streaming capability true,
begin status 13, four 80-ms feeds returned valid empty update frames, finalize
returned a valid empty final, shutdown exit 0.

Public upstream JFK fixture (SHA-256
`59dfb9a4acb36fe2a2affc14bacbee2920ff435cb13cc314a08c13f66ba7860e`):

- 11,000 ms / 176,000 frames / 16 kHz mono;
- worker ready in 63.6 ms;
- 138 × 80-ms in-memory feeds; 31 changed native snapshots;
- first change after chunk 9 (720 ms of supplied audio); accelerated-feed
  compute elapsed 180.0 ms, not microphone/end-to-end latency;
- feed-call compute p50 15.54 ms, p95 165.42 ms, max 279.04 ms;
- finalize 145.01 ms, revision 32, final full payload 108 bytes; and
- worker exit 0.

Only byte counts, flags, revisions, timing, and SHA-256 values were printed.
Transcript text was neither printed nor written. This proves a native model
can traverse protocol v3 and emit changing snapshots; it does not prove
accuracy, live latency, delivery, or release behavior.

## Controlled validation

| Command | Exit | Material result |
| --- | ---: | --- |
| format / whitespace / JSON checks | 0 | pass |
| transcribe.cpp all-feature tests | 0 | 44 passed |
| strict ASR/daemon Clippy with `-D warnings` | 0 | no diagnostics |
| full workspace check and test, locked/offline/all features | 0 | pass; one attended portal observation ignored |
| architecture map | 0 | 20 groups, 136 requirements, 12 pipelines, 26 engines, 66 modules, 8 runtimes |
| governance suite | 0 | 29 passed |
| dependency/source boundaries | 0 / 0 | 9 protected packages |
| traceability | 0 | 136 mapped; all 98 Linux requirements blocked |
| production evidence | 1 | 148 schema-v1 records quarantined; zero v2 candidates |
| pack / release | 1 / 1 | dirty/unsealed and all requirements blocked; read-only |
| P11 gate | 2 | quarantined and non-writing |

The final governance/boundary/release window was non-mutating. Matching
before/after hashes were:

- Git status:
  `db825779d1b595813453c3d11e5a50a0c4e5f40565105fd7155be84841d3ea35`;
- binary diff:
  `8126eee2b096ec8a754616e38ff819067cfefb861f8f177a6c5899164fc1bbf4`;
- tracked/unignored inventory/content:
  `429abe63631e4825eef7692c9de6ab8538cdc9c571422aedbfa6e40f0d7f74a1`.

This baseline update occurs after that measured window. Identical status and
binary-diff hashes prove the validators themselves made no changes.

## Remaining blockers

- live PipeWire producer and bounded backpressure;
- VAD/endpointer and explicit Stop/finalize policy;
- daemon/client partial routing and accessible desktop UX;
- 30-second segmentation/continuity and duplicate control;
- RSS/thermal/accuracy/noise/cancellation/native real-time qualification;
- model-pack acquisition, removal, integrity registry, packaging, and upgrade;
- crash-dump/swap containment; and
- every existing evidence, dependency-advisory, package, and release blocker.

## Rollback and cleanup

Repository rollback is limited to ADR-0036, corrected streaming documentation,
candidate catalog metadata, research/decision/version/state records, and this
baseline. The verified user-cache canary may be removed separately only by an
explicit model-cache cleanup action; it is not trash. No temporary directory,
`.part`, audio/transcript output, worker, daemon, recorder, or ASR child remains.
