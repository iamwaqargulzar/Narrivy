# ADR-0078: The OpenVINO add-on through the Store

Status: Accepted (2026-10-01). Implements the first slices of T2.16 (OpenVINO
speech recognition) and T2.19 (Store) under D21, D23, D25 and D26.

## Context

- D25 makes OpenVINO an add-on the user installs from the Store, not part of
  the base install. D26 asks for every OpenVINO model stage to run in one
  OpenVINO worker.
- The owner asked to "add OpenVINO and have the Store set up" so the
  OpenVINO pipeline can be tried.
- Measured on the reference Core Ultra 9 288V with OpenVINO 2026.4 and
  Whisper large-v3-turbo INT8 on the 11 s JFK clip:
  - CPU: 3.0 s per decode;
  - NPU: 0.45 s per decode, after a one-time compile of 100–106 s;
  - cached NPU loads: 1.5 s in the runtime, 5.3 s end to end in the daemon,
    including pack verification and the self-test.

  OpenVINO's GPU plugin is not usable here yet: Intel's GPU compute runtime
  for this GPU is not in the Pop!_OS archives.
- The owner's earlier Handy work (a C++ NPU worker) found the GenAI C++
  pipeline to be the right native boundary. OpenVINO GenAI 2026.4 also
  ships a C API covering the Whisper pipeline.

## Decision

1. **The daemon never loads OpenVINO.**
   - A separate worker, `narrivy-openvino-worker` (crate
     `engines/openvino`), speaks the same resident protocol as the
     transcribe.cpp worker. The protocol moved to `narrivy-asr-api` as
     `resident_protocol`, so both engines share it, and the parent-side
     supervisor starts either worker
     (`ResidentWorkerEngine::with_worker_binding_until`).
   - The worker is bound by digest like the transcribe.cpp worker.
2. **The worker loads the runtime at run time** from the installed runtime
   pack, through the GenAI C API and `libloading`.
   - That foreign-function code is the crate's only `unsafe`, in
     `src/genai.rs`, with its invariants written beside it.
   - Libraries are loaded by absolute path in dependency order, so nothing
     depends on a library search path.
   - The Level Zero loader is pointed at the pack's NPU driver
     (`ZE_ENABLE_ALT_DRIVERS`).
   - The NPU uses the plugin's own compiler (`NPU_COMPILER_TYPE=PLUGIN`),
     because the bundled driver's compiler is older than the runtime
     accepts.
   - Building the worker needs no OpenVINO SDK.
3. **Packs are multi-file and pinned by their manifest.**
   - A model pack is `model-manifest.json` plus its files; a runtime pack is
     `runtime-manifest.json` plus `lib/` and `licenses/`.
   - The catalog pins each manifest's size and SHA-256, and each manifest
     pins every file.
   - Paths are plain relative names, at most two levels deep, and links are
     refused.
   - Installs go into a private staging directory that is renamed into
     place only when every file has been verified. A retry keeps the files
     already verified.
   - The worker verifies the model pack against the bound manifest again
     before every load.
4. **The runtime pack is assembled, not built.**
   - `scripts/build_openvino_runtime_pack.py` copies the minimal closure
     from the official OpenVINO GenAI 2026.4 archive: core, C API, GenAI,
     the IR frontend, tokenizers, the CPU, GPU and NPU plugins with the NPU
     compiler, and oneTBB.
   - It adds Intel's NPU user-space driver and the Level Zero loader (MIT)
     and the licences, and records each file's origin in the manifest
     (31 files, 292 MB).
   - Publishing it to the Narrivy Hugging Face organisation is the owner's
     step. Until it is published, the catalog entry has no revision and the
     pack installs only from a developer source.
   - Everything is published in one repository, `narrivy/store`, by kind
     and runtime: `models/gguf/`, `models/openvino/`, `runtimes/openvino/`
     and `modules/` (owner request, 2026-10-01). The catalog names each
     pack's folder (`source_path`); download paths are plain names, at most
     four deep.
5. **Developer source.**
   - `NARRIVY_STORE_LOCAL_DIR` names a directory laid out like
     `narrivy/store` (the staging tree that is uploaded).
   - Its files are checked against exactly the same pins as downloads.
     It is set by `scripts/dev-daemon.sh`.
6. **Devices.**
   - `compute` gains `npu`; the worker attests `npu`.
   - `auto` for an OpenVINO model tries the NPU, then the GPU, then the CPU.
     An accelerator is used only after it decodes the known recording
     correctly and faster than real time.
   - A failure on any accelerator drops the engine to the CPU, as in
     ADR-0076.
   - Compiled models are cached per runtime version, beside the models
     directory.
7. **The Store, first slice.**
   - Revised 2026-10-01 (D27): the Store lists accelerators and modules
     only. Vulkan appears as built in, CUDA and the modules as planned;
     OpenVINO model packs are installed and removed on the Models screen.
   - `ListStore` offers the runtime pack, with the daemon's verdict on
     whether it fits this computer:
     - an NPU found;
     - CPU only;
     - needs the runtime first.
   - `DownloadModel` installs any of them, and `RemoveStoreItem` removes
     one; removal is refused while the item is in use.
   - The desktop app has a Store screen. A model that needs the add-on
     links to it, and the NPU device is offered for a model the daemon
     lists as able to run there.

## Consequences

- An OpenVINO model's first NPU load takes about two minutes, while it
  compiles. Later loads take seconds.
  - Warming the cache in the background right after a model pack is
    installed is a follow-up.
  - So is skipping the full re-hash of an unchanged pack, about 2 s of
    each load.
- The OpenVINO GPU path needs Intel's GPU compute runtime. This needs an
  owner decision on how to ship it, as T2.16 item 2 says for drivers
  outside the archives. Until then the worker skips a GPU it cannot use, and
  Vulkan (ADR-0076) remains the GPU path.
- Noise suppression and the shared language model (D26) are later stages in
  the same worker. The runtime pack already carries what the GenAI LLM
  pipeline needs.
- The packaged composition does not include the OpenVINO worker yet; the
  owner deferred packaging during rapid development.
