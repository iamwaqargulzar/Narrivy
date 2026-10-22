# Reference Model Tooling

P5 adds reproducible scripts/configuration for:
- resolving immutable upstream model revisions;
- downloading/exporting/converting to the selected runtime format;
- producing file hashes/sizes and manifest metadata;
- verifying model licenses/redistribution notes;
- running fixed qualification audio through CPU/GPU/NPU candidates;
- recording conversion recipe/tool/runtime versions.

Conversion output is never trusted solely because a command exited zero; load/inference health qualification is required before publishing it to the model catalog.

`spec/examples/model-reference.example.json` is a schema demonstration containing deliberate placeholders and is **not** a real model entry.

## Model packs for the narrivy Hugging Face organisation

`packs.json` lists the packs Narrivy publishes: each one's pinned upstream
source (repository and immutable revision), licence, quantization, and the
SHA-256 and size of every file. `build_model_packs.py` finds the files on
this computer, verifies every byte against those pins, and stages one
directory per pack with a schema-valid `model-manifest.json` and a model
card that states the quantization up front. The manifests are also written
to `manifests/` and validated by `scripts/validate_pack.py`.

```
python3 tools/reference-models/build_model_packs.py --check   # verify only
python3 tools/reference-models/build_model_packs.py           # stage
```

Everything is published in one repository, `narrivy/store`, laid out by
kind and runtime. Each pack's folder is its `store_path` in `packs.json`:

```text
models/gguf/<pack>/          transcribe.cpp models (CPU or Vulkan GPU)
models/openvino/<pack>/      OpenVINO models (NPU, GPU or CPU)
runtimes/openvino/<pack>/    the OpenVINO add-on (scripts/build_openvino_runtime_pack.py)
modules/                     add-on modules, as they are published
```

The tool uses no network and uploads nothing. It stages the tree in
`~/.cache/narrivy/hf-staging/store` with an index README, and writes
`upload.sh` beside it. Publishing is the owner's step: `upload.sh` creates
`narrivy/store` and uploads the tree in one commit with the `hf` CLI.

After the upload, record the commit in `published.json`, pin it in the
catalog seeds, and run `python3 tools/reference-models/verify_published.py`.
It reads the public file listing (no model download) and confirms every
file is present at that revision with the pinned size and SHA-256. Clients must fetch a pack
at the revision recorded there, never from a moving branch.

These packs are redistributions of upstream files, byte-identical to the
pinned revisions. Being published makes a pack available, not qualified: a
device is used only after the exact model, runtime, driver and device pass
qualification on the user's computer.

| Folder in `narrivy/store` | Quantization | Engine | Source |
|---|---|---|---|
| `models/gguf/whisper-large-v3-turbo-q8-0` | Q8_0 (8-bit) | transcribe.cpp | `handy-computer/whisper-large-v3-turbo-gguf` |
| `models/gguf/whisper-large-v3-q5-k-m` | Q5_K_M (5-bit, mixed) | transcribe.cpp | `handy-computer/whisper-large-v3-gguf` |
| `models/gguf/parakeet-unified-en-0.6b-q8-0` | Q8_0 (8-bit), streaming | transcribe.cpp | `handy-computer/parakeet-unified-en-0.6b-gguf` |
| `models/openvino/whisper-large-v3-turbo-int8` | INT8 (8-bit) | OpenVINO | `OpenVINO/whisper-large-v3-turbo-int8-ov` |
| `models/openvino/whisper-large-v3-int8` | INT8 (8-bit) | OpenVINO | `OpenVINO/whisper-large-v3-int8-ov` |
