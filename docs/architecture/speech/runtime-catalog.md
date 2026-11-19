# Runtime Catalog

| Runtime | Status | Devices | Purpose |
|---|---|---|---|
| native | base | CPU | Built-in/native Rust/C/C++ execution |
| vulkan | base-capable | dGPU/iGPU | General GPU compute path |
| openvino | optional | NPU/dGPU/iGPU/CPU | OpenVINO runtime pack |
| onnxruntime | optional | CPU/GPU/provider-dependent | ONNX Runtime pack |
| transcribe.cpp | reference | CPU/iGPU/dGPU (Vulkan) GGUF | Multi-family GGUF STT runtime (MIT; license verified 2026-08-22) |
| llama.cpp | optional | CPU/GPU backend-dependent | Local LLM runtime |
| wasm | roadmap | CPU | Sandboxed extension runtime |
| isolated-worker | roadmap | device-dependent | Out-of-process extension runtime |
