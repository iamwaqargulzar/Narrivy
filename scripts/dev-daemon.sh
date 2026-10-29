#!/usr/bin/env bash
# Run a development narrivyd with real dictation: PipeWire microphone, the
# resident transcribe.cpp engine, and wtype typing with Inbox as the fallback.
#
# Development only. The worker and model are bound by exact size and SHA-256
# (controlled-build identity), which is not release or package provenance.
#
# Usage: scripts/dev-daemon.sh [path/to/model.gguf]
# Set NARRIVY_TRANSCRIBE_COMPUTE=cpu to keep recognition off the GPU.
# Set NARRIVY_OPENVINO_RUNTIME_DIR to a pack from
# scripts/build_openvino_runtime_pack.py --version (for example a nightly) to
# try another OpenVINO build instead of the installed 2026.4 pack.
# Default model: ~/.cache/narrivy/models/whisper-tiny.en-Q8_0.gguf
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
model="${1:-$HOME/.cache/narrivy/models/whisper-tiny.en-Q8_0.gguf}"

if [[ ! -f "$model" ]]; then
    echo "dev-daemon: model not found: $model" >&2
    exit 1
fi

cargo build --locked --manifest-path "$repo/Cargo.toml" \
    -p narrivyd --features asr-transcribe-resident
# Build the worker with the Vulkan backend when this computer can (Vulkan
# headers and the shader compiler are installed). The worker then uses a GPU
# that passes its self-test and the CPU otherwise. NARRIVY_TRANSCRIBE_COMPUTE=cpu
# keeps it on the CPU.
worker_features=resident-worker
if command -v glslc >/dev/null && [[ -e /usr/include/vulkan/vulkan.h ]]; then
    worker_features=vulkan
fi
cargo build --locked --manifest-path "$repo/Cargo.toml" \
    -p narrivy-asr-transcribe --features "$worker_features" --bin narrivy-transcribe-worker

# The OpenVINO add-on's worker (T2.16). It needs no OpenVINO SDK to build:
# it loads the runtime pack the Store installs.
cargo build --locked --manifest-path "$repo/Cargo.toml" \
    -p narrivy-asr-openvino --features worker \
    --bin narrivy-openvino-worker --bin narrivy-openvino-llm-worker

# The daemon refuses a group/other-writable worker, and cargo output follows
# the umask, so run a private 0755 copy.
worker_dir="${XDG_RUNTIME_DIR:?XDG_RUNTIME_DIR is not set}/narrivy-dev"
install -d -m 0700 "$worker_dir"
worker="$worker_dir/narrivy-transcribe-worker"
install -m 0755 "$repo/target/debug/narrivy-transcribe-worker" "$worker"
openvino_worker="$worker_dir/narrivy-openvino-worker"
install -m 0755 "$repo/target/debug/narrivy-openvino-worker" "$openvino_worker"
llm_worker="$worker_dir/narrivy-openvino-llm-worker"
install -m 0755 "$repo/target/debug/narrivy-openvino-llm-worker" "$llm_worker"

# The routed ladder: clipboard paste (Shift+Insert, the user's clipboard put
# back), then typing with wtype, then the Inbox (D29). Clipboard paste reads
# the clipboard to restore it, so it needs the explicit authorization.
export NARRIVY_RECOVERY_MODE=linux-mic-routed-inbox
export NARRIVY_CLIPBOARD_AUTHORIZED=1
export NARRIVY_DEFAULT_PIPELINE=clean
export NARRIVY_ASR_ENGINE=transcribe-cpp
export NARRIVY_AUDIO_SOURCE=pipewire
export NARRIVY_DELIVERY_BACKEND=routed-inbox
export NARRIVY_TRANSCRIBE_RUNTIME=resident
export NARRIVY_TRANSCRIBE_WORKER="$worker"
export NARRIVY_TRANSCRIBE_WORKER_SHA256="$(sha256sum "$worker" | cut -d' ' -f1)"
export NARRIVY_TRANSCRIBE_WORKER_SIZE="$(stat -c %s "$worker")"
export NARRIVY_OPENVINO_WORKER="$openvino_worker"
export NARRIVY_OPENVINO_WORKER_SHA256="$(sha256sum "$openvino_worker" | cut -d' ' -f1)"
export NARRIVY_OPENVINO_WORKER_SIZE="$(stat -c %s "$openvino_worker")"
# Clean-up by a language model (D31) in the Clean pipeline, when a model
# pack built by scripts/build_llm_pack.py is present. NARRIVY_REFINE_MODEL_DIR
# picks another one; NARRIVY_REFINE_COMPUTE=npu|gpu|cpu pins the device.
refine_model="${NARRIVY_REFINE_MODEL_DIR:-$HOME/.cache/narrivy/llm/s1-mini-int8-cw-ov}"
if [[ -f "$refine_model/model-manifest.json" ]]; then
    export NARRIVY_REFINE_MODEL_DIR="$refine_model"
    export NARRIVY_OPENVINO_LLM_WORKER="$llm_worker"
    export NARRIVY_OPENVINO_LLM_WORKER_SHA256="$(sha256sum "$llm_worker" | cut -d' ' -f1)"
    export NARRIVY_OPENVINO_LLM_WORKER_SIZE="$(stat -c %s "$llm_worker")"
fi
# The AI assistant extension (D33), when a chat model pack is present
# (scripts/build_llm_pack.py --existing). NARRIVY_ASSISTANT_COMPUTE picks the
# device; by default the GPU, else the CPU.
assistant_model="${NARRIVY_ASSISTANT_MODEL_DIR:-$HOME/.cache/narrivy/llm/qwen3.5-4b-int4-ov}"
if [[ -f "$assistant_model/model-manifest.json" ]]; then
    export NARRIVY_ASSISTANT_MODEL_DIR="$assistant_model"
    export NARRIVY_OPENVINO_LLM_WORKER="$llm_worker"
    export NARRIVY_OPENVINO_LLM_WORKER_SHA256="$(sha256sum "$llm_worker" | cut -d' ' -f1)"
    export NARRIVY_OPENVINO_LLM_WORKER_SIZE="$(stat -c %s "$llm_worker")"
fi
# Meeting transcription (F22): scripts/meeting.py, run by the Python that has
# sherpa-onnx and openvino-genai (the model-conversion environment).
meeting_python="${NARRIVY_MEETING_PYTHON:-$HOME/.cache/narrivy/convert-venv/bin/python}"
if [[ -x "$meeting_python" ]]; then
    export NARRIVY_MEETING_PYTHON="$meeting_python"
    export NARRIVY_MEETING_SCRIPT="$repo/scripts/meeting.py"
    export NARRIVY_VOICE_SCRIPT="$repo/scripts/voice_filter.py"
    # The assistant's voice mode (scripts/voice_mode.py) uses the same Python.
    export NARRIVY_VOICE_MODE_SCRIPT="$repo/scripts/voice_mode.py"
    # Screens in meetings (scripts/screen_capture.py) use the system Python,
    # which has PyGObject and GStreamer.
    export NARRIVY_SCREEN_SCRIPT="$repo/scripts/screen_capture.py"
fi
# Store items not yet published install from the local Store tree (the
# same tree that is uploaded), checked against the same pins.
staging="${NARRIVY_STORE_LOCAL_DIR:-$HOME/.cache/narrivy/hf-staging/store}"
if [[ -d "$staging" ]]; then
    export NARRIVY_STORE_LOCAL_DIR="$staging"
fi
export NARRIVY_TRANSCRIBE_MODEL="$model"
export NARRIVY_TRANSCRIBE_MODEL_SHA256="$(sha256sum "$model" | cut -d' ' -f1)"
export NARRIVY_TRANSCRIBE_MODEL_SIZE="$(stat -c %s "$model")"

exec "$repo/target/debug/narrivyd"
