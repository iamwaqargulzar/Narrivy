# Recovery Phase 4E — Bounded resident native-stream protocol

**Recorded:** 2026-08-28  
**Base/final HEAD:** `ff6d864012adb1468d02c6b5f80b9ce92258fa04`
(unchanged)  
**Status:** controlled engine/worker implementation complete; non-release;
all 98 Linux requirements remain blocked

## Objective and scope

Implement the real transcribe.cpp stream lifecycle across the isolated worker
boundary for a model that natively attests streaming support, without
misrepresenting it as a composed microphone/daemon/desktop feature.

Scope includes the scaffold ASR cancellation handle, private protocol v3,
worker native stream lifecycle, parent stream adapter, controlled lifecycle and
malformed-frame tests, ADR/build/research/decision records, and this baseline/
state update. Live capture, VAD/endpointer, daemon partial routing, desktop UX,
streaming-model acquisition, evidence, requirement status, manifest reseal,
packaging, commit, and push are excluded.

## Implemented behavior

- Model-gated begin reserves the single resident worker for one opaque stream.
- Non-empty finite 16 kHz mono feed chunks cross fixed 16 KiB buffers. Total
  input remains bounded to 30 seconds; no background queue exists.
- Native full/committed/tentative snapshots and revision/update metadata use a
  length-checked, UTF-8-checked payload capped at 1 MiB.
- Parent validation covers identity, response status, finality, revision
  ordering, reserved fields, lengths, and text encoding. Changed non-empty
  `committed + tentative` becomes a volatile Partial; duplicates are suppressed.
  Final uses native full text and a later Narrivy sequence.
- One caller-owned token covers bounded stream admission and every synchronous
  operation. Any uncertain begin or cancelled feed kills/reaps the isolated
  worker before a capability-consistent restart.
- Finalize releases admission. Drop performs bounded reset or terminates;
  `finish(timeout)` uses the caller's deadline. Capability-consistent restart
  remains mandatory.
- No Partial enters logs, disk, journal, policy, correction, delivery, IPC,
  desktop state, or evidence.

## Content-free native final-only observation

No microphone, audio request, or transcript was used. The current
45,904,544-byte Whisper tiny model produced a valid protocol-v3 ready frame in
123.2 ms with `model_supports_streaming=false`. A stream-begin request returned
status 17 (`STREAMING_UNSUPPORTED`) with zero payload bytes, then bounded
shutdown produced worker exit 0. This corroborates truthful model gating only;
it is not streaming or release qualification.

## Controlled validation

| Command | Exit | Material result |
| --- | ---: | --- |
| `cargo fmt --all -- --check` / `git diff --check` | 0 | formatting and whitespace pass |
| ASR API/mock/transcribe all-feature tests | 0 | 0 + 2 + 44 passed; 17 focused resident tests |
| resident daemon tests | 0 | 6 + 59 + 62 + 63 passed |
| strict engine and daemon Clippy with `-D warnings` | 0 | no diagnostics |
| `cargo check --workspace --all-targets --all-features --locked --offline -q` | 0 | workspace compiles |
| `cargo test --workspace --all-targets --all-features --locked --offline -q` | 0 | complete workspace passes; one attended portal observation remains ignored |
| architecture catalog validator | 0 | 20 feature groups, 136 requirements, 12 pipelines, 26 engines, 66 modules, 8 runtimes |
| governance suite | 0 | 29 passed |
| dependency/source boundary validators | 0 / 0 | 9 protected packages |
| `cargo deny check licenses bans sources` | 0 | policy checks pass; duplicate and unmatched-license warnings remain non-fatal |
| `cargo deny check advisories` | 1 | 20 pre-existing RustSec findings; release blocker retained |
| `verify_traceability.py` | 0 | 136 mapped; 98 Linux requirements blocked |
| `verify_evidence.py --production` | 1 | 148 schema-v1 records quarantined; zero v2 candidates |
| `validate_pack.py` | 1 | deliberately dirty/unsealed manifest; read-only |
| `release_gate.py --target linux-1.0` | 1 | pack failure and all 98 requirements blocked |
| `run_gate.py --phase P11` | 2 | quarantined and non-writing |

The governance/boundary/release validator window was non-mutating. Exact
matching before/after hashes were:

- Git status:
  `3b07a9f9bd8aa35ec6e07e686b366e140836e68c287861fa6c11a77c4f6d7bee`;
- binary diff:
  `a0dc69eb7663e7db8185b79980cb1bb5b2682a6cb00ab9e150b05a352142910d`;
- tracked/unignored inventory/content:
  `49f5e1f3059efb5181d7f1c8d70e29e387d40ee77f0a8876a72e8a678f452581`.

This table replacement occurred after that measurement and is intentionally
outside the workspace-content hash. Identical status and binary-diff hashes
establish that the validators themselves made no changes.

Protected identities remained exact:

- `.deb`: `77733e7ce7c0ace6820c928a6827e0056d2d18490e166927217351724da419c0`;
- `BUILD_LOG.md`: `0c52da5778bf9b8f6f3c843ff0cb0db6e9bc6d206a63d1c898ae5608fba85112`;
- traceability: `ffe19846bf4710ee2ce015b6081ceeb87fff4bc1acac68ac281e738579e59d0f`;
- empty evidence index:
  `dfb5bd72bfb8f63d88b9f8d4027304d491b737c19a584bc293aff2b97923adc5`.

Frontend tests were not rerun because Recovery 4E changes no frontend or
desktop bridge file; the last accepted frontend baseline remains the relevant
result and is not promoted to fresh evidence here. Final cleanup found no
Recovery temporary directory, live Narrivy/ASR/recorder process, unignored
audio/transcript-like file, or changed tracked payload file. Cargo output is an
ignored reproducible build cache, not a release artifact, and was retained.
An independent read-only re-review returned PASS on begin invalidation,
admission cancellation/bounds, checked deadline construction, deterministic
old-PID reaping, and narrowed crash-persistence wording.

## Truth boundaries and next work

- Controlled Python workers prove Narrivy's protocol/ownership behavior, not
  native transcribe.cpp streaming accuracy or timing.
- No streaming-capable model is installed or qualified.
- Product status remains `streaming=disabled:not_composed` until live capture,
  VAD/endpointer, daemon/client events, and accessible desktop UX exist.
- Existing supply-chain, native-delivery, packaging, evidence, and release
  blockers remain unchanged.
- Narrivy creates no payload files/log entries in this slice, but OS core-dump
  and swap containment remains unimplemented and blocks a stronger privacy
  claim.

## Rollback

Rollback is limited to the ASR scaffold cancellation-token migration,
protocol-v3 streaming operations/framing, worker/parent stream implementation
and tests, ADR-0035, current documentation records, this baseline, and state.
Accepted Recovery 1–4D behavior and all protected files must be preserved.
