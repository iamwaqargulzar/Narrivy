# Models & Hardware Contract

## Job
Let normal users choose a reliable quality/resource profile while giving advanced users truthful runtime/device diagnostics.

Default view:
- recommended active model;
- installed models with quality/resource/language summary;
- compute choice `Automatic` plus qualified devices;
- storage use;
- download/remove actions.

Advanced inspector:
- immutable model revision/hash;
- engine/runtime version;
- device/driver identifiers relevant to health;
- Hot/Warm/Cold state;
- qualification/health result and last checked time;
- measured memory/latency when available;
- license/provenance link/metadata.

A merely enumerated NPU is not shown as healthy. Failed accelerator should display CPU fallback and a `Run check` action.

Tests: `UI-MODEL-DOWNLOAD`, `UI-MODEL-INTEGRITY`, `UI-DEVICE-UNHEALTHY`, `UI-CPU-FALLBACK`, `E2E-MODEL-SWITCH`.
