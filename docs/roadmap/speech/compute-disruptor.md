# Compute Disruptor — compute planning that is measured, not guessed

> **Status: roadmap / not in 1.0.** This describes a planned design, not current behaviour. See [the roadmap](../README.md).

Each claim below is checkable against the tree.

**Per-tuple qualification, no assumed pass.** `QualificationTuple`
(`crates/narrivy-compute/src/qualify.rs:29-40`) is keyed by engine, runtime
backend, device class, model, quant, OS and narrator version. A record is
written only from a measured run: `qualify_decode` sets
`pass = fragment_match`, where the match is `stdout.contains(required_fragment)`
on a real decode (`qualify.rs:180,198`). Decode wall time is normalized to an
11 s fixture (`QualificationMetrics::decode_ms_per_11s`, `qualify.rs:18,184`),
and RTF is `audio_seconds / wall_seconds`. The host path uses
`.cache/narrivy/fixtures/jfk16.wav` with the required fragment
`ask not what your country` (`crates/narrivy-compute/tests/p5_qualify_host.rs:23-26`).

**The hierarchy is filtered, not trusted.** `hierarchy()` is
`NPU → dGPU → iGPU → CPU` (`planner.rs:303-310`). `try_class` skips a class
that is not available on the host, then skips an available class with no
**passing** tuple, and records a content-free reason string for each skip
(`planner.rs:313-340`). An enumerated-but-runtime-less accelerator is
therefore never selected.

**Per-tuple invalidation.** `InvalidationKey` is
`{ model_sha256, runtime, backend, tool_version }`
(`crates/narrivy-model-manager/src/residency.rs:32-37`). Any weight, runtime,
backend or tool change changes the key, so the stored residency is demoted
rather than trusted. Health is persisted per key and survives restarts; there
is no global stale flag.

**Budget-aware cleanup.** `CacheBudget { max_bytes, fallback_id }`
(`crates/narrivy-model-manager/src/manager.rs:35-48`) is enforced by
`enforce_cache_budget` (`manager.rs:179-250`): total bytes under `models/` are
summed, and while over budget the oldest-**mtime** unpinned variant is removed,
never the `fallback_id` and never a registry-pinned entry. Eviction is
oldest-mtime-first, not usage-recency-first. `crate::cleanup::remove_unreferenced`
(`crates/narrivy-model-manager/src/cleanup.rs:32`) separately removes staging
leftovers and unreferenced unpinned variants while preserving the selected
source, the CPU fallback and pinned entries, and is idempotent because it
re-derives truth from the directory tree on every run.

**Explainable decisions — with two explicit stubs.** `ResolvedDecision` carries
the human-readable reason chain the frontend renders as `Auto → …`
(`planner.rs:70-74`), plus `estimated_memory_mb` and
`estimated_battery_impact` populated by `finish` (`planner.rs:360-374`). Both
estimates are declared **stubs in the source itself** — a fixed table
(`planner.rs:123-139`: memory 1500/1200/800/0, battery
high/high/medium/low for NPU/dGPU/iGPU/CPU), and `Planner::explain` is
exercised only by an in-crate test (`planner.rs:661`). They are not measured
and must not be presented as measurements.

**OpenVINO discovery returns a suggestion, not an engine.** `ldconfig -p`
output is inspected for `libopenvino.so`; when it is absent,
`probe_openvino_install` returns a `RuntimePackSuggestion` carrying the
runtime name and an install hint
(`crates/narrivy-compute/src/discovery.rs:188-210`,
`crates/narrivy-model-manager/src/manager.rs:23-27,170`). Silent absence would
be a false claim, so absence is reported. There is still no OpenVINO inference
engine in this tree.
