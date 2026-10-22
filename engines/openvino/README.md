# narrivy-asr-openvino — the OpenVINO add-on engine

Speech recognition through OpenVINO (D25, T2.16, ADR-0078). The daemon never
loads OpenVINO. It starts `narrivy-openvino-worker`, which speaks the shared
resident protocol (`narrivy_asr_api::resident_protocol`) and loads the
OpenVINO runtime from the runtime pack the user installed from the Store.

## What is here

| Part | Feature | Native code |
|---|---|---|
| Pack manifests, verification, runtime layout (`src/lib.rs`) | default | none |
| GenAI C API through `libloading` (`src/genai.rs`) | `worker` | the crate's only `unsafe` |
| The worker binary (`src/bin/narrivy-openvino-worker.rs`) | `worker` | through `genai` |
| `examples/openvino_probe.rs`: list devices and time a model pack | `worker` | through `genai` |

Building needs no OpenVINO SDK. The runtime is OpenVINO GenAI 2026.4,
assembled by `scripts/build_openvino_runtime_pack.py`.

## Devices

- `--compute npu` uses the NPU or nothing. `auto` tries the NPU, then the
  GPU, then the CPU.
- An accelerator is used only after it decodes `tests/fixtures/jfk16.wav`
  to the known phrase, faster than real time.
- NPU compilation is cached in the `--cache-dir` the daemon passes.

## Licence

The crate is the workspace licence. The runtime pack carries OpenVINO and
OpenVINO GenAI (Apache-2.0), oneTBB (Apache-2.0), and the Level Zero loader
and Intel NPU driver (MIT), with their licence files.
