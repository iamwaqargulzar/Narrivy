# Reference Model Catalog Policy

Narrivy's 1.0 support statement is a **qualified catalog**, not “any model the runtime can open”.

Each catalog record references a validated `model-manifest` and qualification report. Required fields include immutable source revision, file hashes/sizes, model format/precision, language/task support, engine/runtime range, eligible devices, measured memory/latency where available, license, redistribution status, and conversion recipe/version.

## 1.0 reference tiers

- **Accuracy:** Whisper large-v3 family candidate.
- **Balanced/resource:** smaller Whisper-family candidate selected by measured latency/quality/resource use.
- **Fallback:** CPU-known-good configuration used for diagnostics and recovery when accelerators fail.

The coding agent may add variants only when they improve a defined user segment and are fully qualified. Model count is not a product metric.

## Catalog states

`candidate → downloadable/importable → verified → qualified on device/runtime → default-eligible`.

A model can remain usable on CPU while unqualified/unhealthy on NPU; do not collapse health to a single global flag.
