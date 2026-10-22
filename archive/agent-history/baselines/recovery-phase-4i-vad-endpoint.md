# Recovery Phase 4I — Conservative VAD and endpoint origin

**Recorded:** 2026-08-28  
**Base/final HEAD:** `ff6d864012adb1468d02c6b5f80b9ce92258fa04`
(unchanged)  
**Status:** controlled VAD/endpointer composition implemented and independently
reviewed; native acoustic/latency and release qualification blocked; all 98
Linux requirements remain blocked

## Objective and scope

Reduce the architectural dependence on the fixed capture safety window for the
strict streaming recovery lane without adding a neural dependency, trimming
audio, confusing automatic endpointing with cancellation, or duplicating the
downstream transaction.

Scope is portable VAD/endpointer contracts, strict daemon composition, truthful
finalization origin and capability reporting, controlled tests,
architecture/ADR/state records, upstream research, and read-only validation.
No microphone, native acoustic observation, evidence, requirement status,
manifest reseal, package, commit, push, or release action occurred.

## Implemented boundaries

- `VoiceActivityDetector` and `SpeechEndpointer` are independent portable,
  replaceable contracts in `narrivy-core`; they own no device, ASR, timer,
  journal, delivery, UI, file, or network side effect.
- `NativeEnergyVad` is an allocation-free orchestration-worker candidate with
  activation/release RMS and peak hysteresis. It rejects malformed/non-finite
  audio and intentionally makes no neural-VAD or native-quality claim.
- `DictationEndpointer` is sample-count based. Controlled defaults require
  240 ms of contiguous classified speech followed by 560 ms of trailing
  silence. Initial silence and separated sub-minimum bursts cannot end a
  session; resumed speech resets the hangover.
- Endpointing observes every validated chunk before the ASR feed. The chunk is
  then pushed and its ASR events drained before graceful finalization is
  requested. It therefore does not trim the endpoint-triggering audio, and
  already-buffered chunks remain ordered and are also fed to ASR.
- `SessionFinalization` stores the first graceful origin atomically as
  `UserRequested` or `EndpointDetected`; cancellation remains a separate token.
  Linux capture returns typed `CaptureEnd::EndpointDetected` only when that
  origin won. Core requires the typed summary for an endpoint-owned stop.
- VAD/endpointer failure reuses the existing one-absolute-deadline cleanup and
  attempts both capture and ASR teardown. There is no mock, batch, alternate
  VAD, or delivery substitution.
- Only strict, initialized, model-attested streaming lanes advertise
  `audio.native-energy-vad` and `speech.endpoint` as
  `composed:controlled_unqualified`; all other cases remain explicitly
  disabled.

## Research basis and non-adoption

The architecture pattern was checked against primary upstream whisper.cpp VAD
documentation/example and the official Silero VAD C++ state machine. They
support separating activity classification from minimum-speech/silence endpoint
policy and using threshold state/hysteresis. Narrivy did not import their model,
runtime, or acoustic thresholds. The sources and conclusions are recorded in
`agent/RESEARCH_LOG.md`; ADR-0039 owns the local decision.

## Controlled validation

New coverage includes VAD configuration and malformed audio, hysteresis,
initial silence, separated noise, speech resumption, exact threshold behavior,
chunk-partition invariance, first-origin preservation, typed Linux endpoint
completion with child reaping, all-audio forwarding, shared-tail delivery, and
both-owner cleanup on VAD failure. The controlled composition counts ten ASR
pushes for its three-speech/seven-silence fixture, including the endpoint
triggering chunk.

| Command | Exit | Material result |
| --- | ---: | --- |
| targeted endpoint/core/Linux tests | 0 | endpoint suite 6; core 87; Linux 208 passed + 1 ignored attended observation |
| `cargo check --workspace --all-targets --all-features --locked` | 0 | pass |
| `cargo test --workspace --all-targets --all-features --locked` | 0 | complete workspace pass; one attended portal observation ignored |
| workspace all-target/all-feature Clippy with `-D warnings` | 0 | no diagnostics |
| `cargo fmt --all -- --check` / `git diff --check` | 0 / 0 | pass |
| desktop Vitest / TypeScript | 0 / 0 | 17 files, 24 tests; existing React `act(...)` warnings |
| doctor | 0 | current Linux/COSMIC/PipeWire toolchain observed; no permission granted |
| independent read-only review | PASS | no blocking correctness, race, clipping, cleanup, or capability-truth finding |
| governance suite | 0 | 29 passed |
| source/dependency boundaries | 0 / 0 | 9 protected packages |
| action pins / traceability | 0 / 0 | 5 pins; 136 mapped; all 98 Linux requirements blocked |
| production evidence | 1 | 148 schema-v1 records quarantined; zero v2 candidates |
| pack / release | 1 / 1 | dirty/unsealed; all requirements blocked; read-only |
| P11 gate | 2 | quarantined and non-writing |

`cargo deny` was not rerun because 4I adds or changes no dependency. The last
accepted 4H result remains the relevant repository result: 20 pre-existing
Tauri/GTK transitive advisory or unmaintained findings, no exception added,
and the release blocker retained.

The governance window was non-mutating. Exact matching before/after hashes:

- Git status:
  `9a58a8f7ac2ea6bb02b3f11edda6e780b0c06b920bf6ca2df5f1764b32057be6`;
- binary diff:
  `4096cefd86dd45910b1b5bf1a0204d7b2cead7fc6e7d00314f1ac6dd3347272a`;
- existing tracked/unignored inventory/content:
  `fb8eea98f0517027903221a0a86808760b2c55bd3debb57943ce019cf37cf504`.

The status and binary-diff hashes also cover the three intentionally deleted
legacy IPC source paths; the content command reports them as missing and hashes
the existing inventory. This baseline was written after the validator window.

Protected identities remained exact:

- `.deb`: `77733e7ce7c0ace6820c928a6827e0056d2d18490e166927217351724da419c0`;
- `BUILD_LOG.md`: `0c52da5778bf9b8f6f3c843ff0cb0db6e9bc6d206a63d1c898ae5608fba85112`;
- traceability: `ffe19846bf4710ee2ce015b6081ceeb87fff4bc1acac68ac281e738579e59d0f`;
- empty evidence index:
  `dfb5bd72bfb8f63d88b9f8d4027304d491b737c19a584bc293aff2b97923adc5`.

## Independent review

The reviewer returned PASS after checking endpoint determinism, feed-before-
finalize ordering, queued-audio preservation, atomic first-origin semantics,
cancellation rechecks, shared-deadline teardown, and strict capability gating.
Its only non-blocking note was to add this baseline and update `STATE.json`,
which are now complete.

## Truth boundary and remaining work

This phase proves a controlled executable policy, not that the default energy
thresholds work across real microphones, voices, languages, noise, playback,
or rooms. It does not prove reduced native latency, first/last-word retention,
false-start/false-end rate, accuracy, accessibility, or PERF-005.

Remaining delivery-path work includes native calibration and bounded
content-free measurement, pre-roll admission, maximum-utterance and continuous
segmentation policy, partial IPC and accessible desktop presentation, device
routing/hotplug recovery, native real-time queue/loss/duplication/cancellation
and RSS/thermal qualification, core-dump/swap hardening, model-pack lifecycle,
dependency advisory resolution, clean-machine packaging, evidence, and release
acceptance.

## Rollback and cleanup

Rollback is limited to the endpoint module/export, streaming-slice endpoint
composition, finalization-origin type/API, Linux typed capture end, strict
daemon composition/capability strings/tests, ADR-0039/current documentation,
decision/research/state records, and this baseline. Accepted Phase 1–4H work
and protected files must remain. The verified 4F user-cache model is
intentional, not trash. No temporary audio/transcript file, native worker,
daemon, recorder, evidence record, or package was created by 4I.

Final cleanup removed only regenerated Python `__pycache__` trees and three
stale, positively identified Narrivy controlled-capture test directories
(`/tmp/.tmpNP0rqA`, `/tmp/.tmpaFR18s`, `/tmp/.tmprgPOBw`) containing only a
test recorder script and stale PID marker. No broad `/tmp` cleanup was used;
unrelated temporary directories were left untouched. Final inspection found no
live `narrivyd`, `pw-record`, `transcribe-cli`, or resident-worker process and
no unignored `.pyc`, transcript, partial, or temporary payload file.
