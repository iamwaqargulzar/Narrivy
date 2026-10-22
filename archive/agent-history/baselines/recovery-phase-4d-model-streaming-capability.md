# Recovery Phase 4D — Native model streaming-capability attestation

**Recorded:** 2026-08-28  
**Base/final HEAD:** `ff6d864012adb1468d02c6b5f80b9ce92258fa04`
(unchanged)  
**Status:** controlled implementation and content-free local observation
complete; non-release; all 98 Linux requirements remain blocked

## Objective and scope

Preserve the model-dependent nature of transcribe.cpp streaming by carrying the
loaded model's authoritative capability across the resident-worker boundary,
without promoting that fact into a product streaming claim.

Scope is limited to the private worker startup handshake, immutable parent
capability state, restart consistency, truthful daemon status, controlled tests,
ADR/build/research/decision records, and this baseline/state update. No stream
begin/feed/finalize protocol, live capture overlap, VAD/endpointer, partial UX,
microphone run, evidence, requirement status, manifest reseal, package, commit,
or push is included.

## Implemented behavior

- Resident protocol v2 requires a one-byte capability payload on `READY`.
- The native worker sets bit 0 directly from the successfully loaded model's
  `supports_streaming`; seven unknown bits and every other payload length fail
  closed.
- The parent stores the attested capability independently of current child
  liveness. A restarted worker must report the identical set or it is
  terminated/reaped and returns `resident_capability_changed`.
- Daemon status differentiates
  `model-streaming=supported|unsupported` from
  `streaming=disabled:not_composed`. Even a capable model cannot promote the
  product capability before the full streaming architecture exists.
- Existing one-shot resident decode behavior, fail-fast admission,
  cancellation, restart, privacy, and final transcript semantics are unchanged.

## Content-free local observation

No microphone, audio request, or transcript was used. The exact current
45,904,544-byte Whisper tiny GGUF loaded and produced a valid protocol-v2 ready
frame in 116.9 ms with `model_supports_streaming=false`. A bounded shutdown
frame produced worker exit 0. This is model-capability corroboration only, not
streaming, latency, accuracy, microphone, package, or release evidence.

## Controlled validation

| Command | Exit | Material result |
| --- | ---: | --- |
| `cargo fmt --all -- --check` / `git diff --check` | 0 | formatting and whitespace pass |
| resident engine tests | 0 | 28 passed |
| resident daemon tests | 0 | 6 + 59 + 62 + 63 passed |
| strict engine and daemon Clippy with `-D warnings` | 0 | no diagnostics |
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
  `40a9b42f475bd1c61711e257b9c3e44b39b1a045164680e34cf7e2b3056dff7b`;
- binary diff:
  `80974a4cf55252a677a7b73782e0d3fb3cf64f31f07c514088deb053d484c325`;
- tracked/unignored inventory/content:
  `bfc59861477722e7c459d19eebb199874e16ea17465a20ab83c153a2098a89d2`.

This final table replacement occurred after the measured validator window and
is intentionally outside its workspace-content hash. The identical status and
binary-diff hashes establish that the validators themselves made no changes.

Protected identities remained exact:

- `.deb`: `77733e7ce7c0ace6820c928a6827e0056d2d18490e166927217351724da419c0`;
- `BUILD_LOG.md`: `0c52da5778bf9b8f6f3c843ff0cb0db6e9bc6d206a63d1c898ae5608fba85112`;
- traceability: `ffe19846bf4710ee2ce015b6081ceeb87fff4bc1acac68ac281e738579e59d0f`;
- empty evidence index:
  `dfb5bd72bfb8f63d88b9f8d4027304d491b737c19a584bc293aff2b97923adc5`.

## Truth boundaries and next work

- Narrivy still has no production streaming operations or partial transcript.
- A streaming-capable model has not been downloaded, integrity-pinned, licensed,
  language/accuracy qualified, or run.
- Capability consistency is not executable/model package integrity.
- ADR-0034 enumerates the contracts that must be decided before streaming
  composition.
- Every supply-chain, native delivery, desktop, packaging, evidence, and
  release blocker remains in force.

## Rollback

Rollback is limited to protocol-v2 capability payload changes, worker/parent
attestation and status code/tests, ADR-0034, related documentation records,
this baseline, and the state update. Accepted Recovery 1–4C behavior and all
protected files must be preserved.
