# Narrivy's copy of transcribe-cpp-sys 0.4.1

This is the published crate `transcribe-cpp-sys` 0.4.1 (MIT, upstream commit
4d7c1ccefd39e5a9ea36cca5f8fefc6dcc14b99b), used through `[patch.crates-io]`
in the workspace `Cargo.toml`. It differs from the published crate in two ways.

## Removed: ggml backends Narrivy never builds

`ggml/src/` no longer has the CANN, CUDA, ET, HIP, MUSA, SYCL, WebGPU, zDNN,
ZenDNN, OpenCL, Hexagon, OpenVINO and VirtGPU backends. Each is only built
when its `GGML_<NAME>` option is on, and none is. CPU, Vulkan, Metal, BLAS and
RPC are kept.

## Changed: the Whisper encoder's attention (marked `[Narrivy patch]`)

Measured on an Intel Arc 140V (Lunar Lake) with whisper-large-v3-turbo Q8_0:
ggml-vulkan runs flash attention on its scalar shader there, because the
device does not offer the matrix-core shape its fast path needs. That one
operation took about two thirds of each 30-second window's encoder time.

- `src/arch/whisper/encoder.cpp`: the manual (non-flash) attention casts K and
  V^T to F16, as flash attention reads them, so the GPU uses its F16
  matrix-core matmul kernels.
- `src/arch/whisper/model.cpp`, `whisper.h`: on a GPU, with neither
  `TRANSCRIBE_NO_FLASH` nor `TRANSCRIBE_FORCE_FLASH` set, the first window a
  model encodes is encoded flash, manual, flash, and the faster of the manual
  run and the second flash run is kept for that model. Every run gives the
  same encoder output, so the window's result stands. CPU sessions keep flash.

On the Arc 140V the encoder window went from about 1.6 s to about 1.05 s
(about 0.9 s with a glslc new enough for ggml's integer dot-product shaders;
see `scripts/build_release_candidate.sh`), with byte-identical transcripts on
15 recordings of 1 to 87 seconds.

## Changed: ggml-vulkan's softmax for 1025- to 2048-wide rows on Intel (marked `[Narrivy patch]`)

The manual attention above softmaxes 20 x 1500 rows of 1500 scores per
encoder layer. ggml-vulkan gave each such row 512 threads and two 9-barrier
shared-memory reductions, and reached about 45 GB/s on the Arc 140V.

- `vulkan-shaders/soft_max.comp`: with `SUBGROUP_REDUCE`, the row maximum and
  sum are a subgroup reduction plus one shared step across subgroups.
- `vulkan-shaders/vulkan-shaders-gen.cpp`: builds that variant as
  `soft_max_f32_sg`.
- `ggml-vulkan.cpp`, `ggml-vulkan-types.h`: on Intel devices with subgroup
  arithmetic, F32 softmax rows of 1025 to 2048 use it with 128 threads, so
  every value stays in registers. `GGML_VK_DISABLE_SOFT_MAX_SG128` turns it
  off. Other vendors keep ggml's choice.

On the Arc 140V the encoder's softmax went from 245 ms to 119 ms per window
(about 97 GB/s), and 16 recordings of 1 to 87 seconds took 13% less time in
all (18.0 s to 15.6 s), with byte-identical transcripts.

To update: copy the new published crate here, delete the backends above,
and re-apply the lines marked `[Narrivy patch]`.
