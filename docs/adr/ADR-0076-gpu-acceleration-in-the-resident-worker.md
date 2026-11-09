# ADR-0076: GPU acceleration in the resident worker, self-tested, with CPU fallback

Status: Accepted (owner direction D25, 2026-09-30; implements the Vulkan part
of the locked pipeline D21 and plan task T2.15).

## Context

- The resident transcribe.cpp worker loaded every model with a strict CPU
  request. The compute planner's choice was advisory: nothing routed
  recognition by it.
- The pinned wrapper (`transcribe-cpp` 0.2.3) can build the ggml Vulkan
  backend into the same library. One binary then serves computers with and
  without a usable GPU.
- Measured on the reference computer (Core Ultra 9 288V, Intel Arc
  integrated GPU, Mesa 26.1.6), release build, the 11 s repository fixture:

  | Model | CPU | Vulkan |
  |---|---:|---:|
  | Whisper tiny.en Q8_0 | 0.42 s | 0.23 s |
  | Parakeet Unified EN 0.6B Q8_0 | 1.05 s | 0.30 s |
  | Whisper large-v3-turbo Q8_0 | 10.1 s | 2.6 s |

  Streaming the same audio to Parakeet in 80 ms chunks took 5.7 s on the
  CPU (slowest single feed 877 ms) and 1.7 s on Vulkan (slowest 387 ms).
  These are development measurements, not release evidence.

## Decision

1. **Vulkan is built into the worker** by the `vulkan` cargo feature of
   `narrivy-asr-transcribe`. It is the default acceleration on Linux, and on
   Windows when that port exists. It links `libvulkan.so.1` at load time, so
   the package depends on `libvulkan1` and recommends `mesa-vulkan-drivers`.
   The library is linked statically into the worker: the worker runs from a
   sealed in-memory copy, so it cannot load backend modules from beside
   itself.
2. **The parent asks; the worker decides and attests.** The parent passes
   `--compute auto` or `--compute cpu`. For `auto`, a worker built with
   Vulkan loads the model on the GPU and then runs a self-test there: it
   decodes the repository's known recording, and the text must contain the
   known phrase and arrive faster than real time. If there is no GPU, the
   load fails, or the self-test fails, the worker releases the GPU copy and
   loads on the CPU. The ready frame attests what was loaded: CPU,
   integrated GPU or discrete GPU. The self-test also warms the device up,
   so the first dictation does not pay for shader compilation.
3. **A GPU worker that fails is replaced by a CPU worker**, and the GPU
   stays withdrawn for the life of the engine:
   - a worker that was allowed a GPU and does not reach readiness (crash,
     hang, error) is followed at once by one strict-CPU start;
   - a one-shot recognition that fails on a GPU worker is recognised again,
     from the same audio, on a CPU worker: the dictation is not lost;
   - a streaming session that fails on a GPU worker withdraws the GPU, so
     the worker started by the next recovery is strict CPU. The stream's own
     audio is not replayed (the engine does not retain it).
4. **Compatibility.** `--compute` is optional and the worker without it is
   strict CPU, with a zero device byte in the ready frame. The wire version
   stays 5, and tools that start the worker directly (the CPU performance
   recorder) keep measuring strict CPU.
5. **Truthful status.** A ready worker's device is reported as
   `asr.transcribe-cpp.device=cpu|igpu|dgpu`. Clients show that device and
   nothing else as "runs on". Nothing is reported while no worker is ready.
6. **Setting.** `NARRIVY_TRANSCRIBE_COMPUTE=auto|cpu` (default `auto`).
7. **CUDA and OpenVINO are Store add-ons** (D25), not part of the base
   install. The NPU → GPU → CPU order of D21 is unchanged.

## Amendment (2026-09-30): a device and a model per pipeline

- `--compute gpu` is a third request: a GPU or nothing. The worker reports a
  backend failure instead of falling back, and the parent does not retry on
  the CPU at start-up. After it has started, a GPU failure still moves the
  next worker to the CPU (decision 3): a running dictation is never lost to
  a device preference.
- Each pipeline stores the model and the device (`auto`, `gpu`, `cpu`) it
  wants (IPC minor 15). One engine is loaded at a time. When a dictation
  starts, the daemon reloads only if the pipeline's model differs from the
  loaded one or its device is not satisfied: automatic is satisfied by an
  automatic load wherever it landed and by anything already on a GPU; GPU by
  an engine that attests a GPU; CPU by one that attests the CPU.
- A reload at start delays listening by the load time (2 to 4 s measured in
  a development build on the reference computer). The overlay shows that a
  model is loading.

## Consequences

- The self-test proves one recording on one device at start-up. It is not
  the full qualification of D21: results are not yet stored per
  model/runtime/driver/device tuple, a slow-but-working GPU is accepted when
  it is faster than real time even if the CPU would be faster, and the
  compute planner's tokens still describe its own, separate plan.
- Start-up is longer by one decode of 11 s of audio on the GPU (0.3 s to
  3 s on the reference computer).
- Release claims about acceleration still need native qualification
  evidence on the reference computer.

## Not yet done (T2.15)

- Stored self-test records and re-testing only after a model or driver
  change; choosing between several GPUs; comparing GPU and CPU speed.
- Replaying a failed streaming session's audio on the CPU.
- A Settings control for the device, wired to the daemon.
- Building the packaged worker with Vulkan in the release pipeline, and its
  qualification evidence.
