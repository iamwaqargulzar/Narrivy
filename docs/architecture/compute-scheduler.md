# Compute Scheduler

Narrivy chooses a qualified pipeline execution plan from capabilities rather than exposing raw runtimes to ordinary users. Advanced users can override Auto with an exact compatible path.

## Candidate preference

Auto explores **NPU → dGPU → iGPU → CPU**. This is a preference order, not a promise that a higher tier wins. A tuple must be compatible, healthy and qualified before it can be selected.

Vulkan is an execution API used by GPU candidates, not a separate hardware tier. OpenVINO or another runtime may expose more than one device class.

## Inputs

- model/engine/runtime/device compatibility and qualification state;
- transcript quality/accuracy constraints;
- latency and real-time-factor targets;
- CPU/dGPU/iGPU/NPU availability and health;
- memory/VRAM/NPU memory;
- battery/AC status and power profile where available;
- thermal/load state where reliable;
- cold/warm compile/load cost;
- model Hot/Warm/Cold state;
- privacy/network policy;
- explicit user/device/model preference.

## User modes

- **Auto / Balanced** — best qualified general-purpose result.
- **Best Accuracy** — quality first within acceptable stability.
- **Lowest Latency** — latency first with quality floor.
- **Lowest Power** — avoid power-hungry devices where another qualified path meets the target.
- **Manual preferred** — prefer an exact user-selected tuple; bounded fallback remains allowed.
- **Manual locked** — do not silently change the requested tuple; report failure/recovery choice if unavailable.

Every automatic decision is visible as a resolved value, for example `Auto → Intel NPU / OpenVINO / model X`.

## CPU safety path

CPU is the mandatory correctness fallback and remains installed/tested even when an accelerator wins. It is last in Auto preference, not optional infrastructure.

## Model residency

- **Hot:** ready to infer.
- **Warm:** compiled/cache-ready, quickly loadable.
- **Cold:** immutable source model only.

Never wake a discrete GPU for trivial work when a qualified lower-power path meets the active policy. Never keep an accelerator merely because it exists if benchmark/health evidence shows a lower tier is better.

## Whole-Pipeline scheduling

Qualification may score an entire Pipeline execution plan, not only isolated model/device tuples. Example candidates can include ASR+postprocessor both on NPU versus ASR on NPU and postprocessor on iGPU/CPU. The preferred hardware tier never overrides measured contention, residency, memory, latency, quality or stability.

