#!/usr/bin/env bash
# Longer property-test runs over the untrusted-input boundaries: IPC frames,
# envelopes and commands; the Unix socket server; pack manifests and model
# downloads; model import sources; and the dictation language tags.
#
# The same tests run in `cargo test` with small case counts. This raises the
# count (default 20000 per property) for a slower, deeper run. It is not
# wired into CI.
#
#   scripts/fuzz-untrusted-input.sh            # 20000 cases per property
#   scripts/fuzz-untrusted-input.sh 200000     # more
set -euo pipefail

cases="${1:-20000}"
cd "$(dirname "$0")/.."

export PROPTEST_CASES="$cases"
# No regression files are written; a failure prints its minimal input.
export PROPTEST_MAX_SHRINK_ITERS="${PROPTEST_MAX_SHRINK_ITERS:-4096}"

cargo test --release -p narrivy-ipc --test untrusted_input
cargo test --release -p narrivy-ipc-unix untrusted_peer
cargo test --release -p narrivy-asr-openvino -- tests::
cargo test --release -p narrivyd --bin narrivyd -- text_options download
cargo test --release -p narrivy-platform-linux model_import
