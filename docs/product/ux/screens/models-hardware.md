# Models & Hardware Contract

## Job
Let normal users choose a reliable quality/resource profile while giving advanced users truthful runtime/device diagnostics.

## In 1.0

The Models screen (`apps/narrivy-desktop/src/app/screens/Models.tsx`) shows
the service's model list in four groups: on this computer, available to
download, not published yet, and needs an engine. Each model shows its
languages, live or final-only text, devices, whether it is qualified, and its
source, revision, licence and engine. The screen can download (with progress,
cancel and resume), remove, and install the bundled model. Search, filters and
sort narrow the list. The model and device for a pipeline are chosen in
Pipelines → Transcribe, not here.

Items marked *(roadmap)* below are not in 1.0.

## Target

Default view:
- recommended active model; *(1.0: "In use" marks the active model; there is no recommendation.)*
- installed models with quality/resource/language summary;
- compute choice `Automatic` plus qualified devices; *(1.0: in Pipelines → Transcribe.)*
- storage use; *(roadmap)*
- download/remove actions.

Advanced inspector *(roadmap as a separate inspector; 1.0 shows the items noted below on each model row)*:
- immutable model revision/hash; *(1.0: revision only; hash is roadmap.)*
- engine/runtime version; *(1.0: engine name only.)*
- device/driver identifiers relevant to health; *(roadmap)*
- Hot/Warm/Cold state; *(roadmap)*
- qualification/health result and last checked time; *(1.0: qualified or not; last checked time is roadmap.)*
- measured memory/latency when available; *(roadmap)*
- license/provenance link/metadata.

A merely enumerated NPU is not shown as healthy. Failed accelerator should display CPU fallback and a `Run check` action. *(Roadmap: the `Run check` action.)*

Tests: `UI-MODEL-DOWNLOAD`, `UI-MODEL-INTEGRITY`, `UI-DEVICE-UNHEALTHY`, `UI-CPU-FALLBACK`, `E2E-MODEL-SWITCH`.
