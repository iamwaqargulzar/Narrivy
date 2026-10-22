# Performance Benchmark Protocol — Linux 1.0

> **Authority:** legacy planning Part I. No numeric latency target is a release gate unless measured per this protocol.

## Reference hardware classes

| Class | Example | CPU | RAM | NPU | GPU | OS lane |
|---|---|---|---|---|---|---|
| `ref-intel-lunar` | Intel Core Ultra 7 258V (Lunar Lake) | 8-core (4P+4E) | 32 GB | Intel NPU (OpenVINO 2026.4; exact driver recorded at qualification) | Intel Arc 140V (Vulkan 1.3) | Pop!_OS 24.04 COSMIC Wayland |
| `ref-amd-phoenix` | AMD Ryzen 7 8700G (Phoenix) | 8-core Zen4 | 32 GB | XDNA 16 TOPS (amdxdna, Linux 6.14+, XRT 2.18, Ryzen AI 1.8) | Radeon 780M (Vulkan + MIGraphX) | Ubuntu 24.04.2 GNOME Wayland (when hardware arrives) |
| `ref-cpu-only` | Any x86_64, CPU-only control | — | 16 GB min | none | none | Both lanes (validates CPU fallback) |

Each benchmark run records `environment.{cpu,gpu,npu,os_version,kernel,desktop}` plus `builder_identity` from `release-artifacts.json`.

## Fixed release fixture

The Linux 1.0 CPU runtime benchmark uses `tests/fixtures/jfk16.wav`: 11 seconds,
PCM16 mono at 16 kHz, SHA-256
`59dfb9a4acb36fe2a2affc14bacbee2920ff435cb13cc314a08c13f66ba7860e`.
It is the redistributable canonical whisper.cpp sample pinned in
`tests/fixtures/README.md`; no user content is used. Multilingual and long-form
quality fixtures remain separate correctness inputs and must not be listed here
until their files, licenses, expected results and hashes are committed.

Real microphone latency is measured separately with the physical loopback
procedure. The CPU runtime benchmark sends the fixed PCM through the exact
packaged resident-worker protocol and never retains transcript text.

## Build/mode/model state

- Build mode: release (`--release`)
- Linux 1.0 packaged CPU control: registry-pinned
  `whisper-tiny.en-Q8_0`, SHA-256
  `e8c9b73c06344307d8b346e07fbe93dd88d894627854bcff31523f1ce44394fa`,
  run by the package's process-isolated transcribe.cpp worker with its enforced
  CPU backend. Additional model/runtime tuples require their own immutable
  identity and device evidence.
- **Residency states — Hot/Warm/Cold (PERF-007):**
  - `Cold` — first load after reboot, no compiled cache
  - `Warm` — compiled blob cache exists, model not resident
  - `Hot` — model resident in memory/device
  Cache key = `model hash + recipe + quant + runtime version + device/driver + compile opts`; invalidation causes `Cold` recompilation.

## Metrics (separate — do not average into one)

| Metric ID | Start event | End event | Target (p50) | Notes |
|---|---|---|---|---|
| `hotkey_ack` | global shortcut portal event received | daemon ack event emitted | <30 ms | measured via portal→daemon IPC timestamp |
| `capture_ready` | shortcut received | PipeWire capture stream ready (first buffer) | <50 ms warm service | requires warm daemon, not cold boot |
| `listening_feedback` | shortcut received | overlay visible listening state | <100 ms | UI thread timestamp |
| `speech_partial_first` | speech end (VAD) or stream start | first `Partial` event where engine supports streaming | <250 ms | only where `engine.supports_streaming == true` |
| `release_raw` | key-release / toggle second press | `Final` raw transcript available | measured, no fixed target yet | baseline recorded |
| `release_committed` | key-release | committed final text (after pipeline, before delivery) | baseline | |
| `committed_delivered` | committed | delivery confirmed or Inbox fallback | baseline | verified delivery semantics: Delivered/Verified/Probably/Failed/Unknown per Part G |

The legacy `p6-perf-baseline` entrypoint is quarantined and cannot qualify these
metrics. `scripts/record_native_ui_dictation_evidence.py` is the reviewed native
desktop runner for `capture_ready` (PERF-003) and `listening_feedback`
(PERF-004). It measures the installed atomic activation command-to-visible-
overlay boundary and reads the daemon's content-free monotonic stage offsets
through IPC protocol minor 8. The broader AT-SPI UI action duration is retained
as diagnostic UX data but is not substituted for the activation hot path.
The combined native run writes each performance record independently. A target
miss leaves that requirement unqualified while preserving unrelated native UI,
dictation, and restart-recovery records that passed in the same exact-package
run.
PERF-001 and PERF-002 remain blocked until reviewed procedures cover the global
shortcut, streaming partial, finalization and delivery boundaries.

The narrower packaged CPU runtime benchmark is
`scripts/record_cpu_performance_evidence.py`. It records worker-ready and decode
latency with `CLOCK_MONOTONIC`, observes the worker's final-only capability, and
qualifies only the conditional partial behavior and Hot/Warm/Cold residency
claims covered by its reviewed procedures.

## Sample count & statistics

- `Hot`: one resident worker, 5 warmup decodes discarded, then **n=100** measured decodes.
- `Warm`: **n=10** fresh worker processes with the host file cache available;
  the model is not resident in a worker.
- `Cold`: one descriptive first worker/load from a fresh package extraction.
  Cold is defined as a first-load event and therefore is not presented as an
  n=100 repeated-reboot distribution. Cross-boot Cold trends require separate
  runs and keep each boot as its own observation.
- Report `p50`, `p95`, `mean`, `max`, and **failure rate** for repeated metrics.
- Allowed system load: idle desktop, no competing audio inference, CPU governor `performance` or documented alternative, thermal throttling noted if observed.

## Tooling

- `scripts/record_cpu_performance_evidence.py` benchmarks the exact packaged
  worker/model binding without retaining transcript text.
- `scripts/record_native_ui_dictation_evidence.py` measures installed atomic
  activation-to-listening feedback and validates the daemon's completed-session
  timing stages without retaining audio or transcript text.
- Results are emitted as JSON evidence (`evidence.schema.json`) with `metrics: {hotkey_ack: {p50_ms, p95_ms, fail_rate}}` and `artifact_hashes: {fixture: sha256}`.
- P6 establishes baselines; P9 checks regressions (fail if p95 regresses >20% or fail rate >1% on blocking metrics without owner-approved update).

## What is not measured as latency

- Model download time, package install, or first Cold compile are recorded separately as “first-run cost”, not as hot-path latency.

## When a target cannot be measured reproducibly

Rewrite or remove the numeric target before it becomes a gate; do not keep a numeric claim without a protocol. See `docs/maintainers/phases/phase-06-integrated-dictation-beta.md` and `phase-09-hardening.md`.
