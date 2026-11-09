# Windows counterpart of scripts/dev-daemon.sh (PORT-007): run the developer
# daemon with the real resident speech engine, WASAPI capture and SendInput
# delivery.
#
# Run it in the logged-in desktop session, never over SSH or as a service
# (session 0 cannot reach the microphone, input or global shortcuts), and
# unelevated: the daemon's journal, worker and model checks require files
# owned by the user, and an elevated client is refused by the pipe.
#
#   powershell -ExecutionPolicy Bypass -File scripts\dev-daemon.ps1 [-Model <gguf>]
#
# Needs: the VS Build Tools CMake (set $env:CMAKE), and the model, by default
# %LOCALAPPDATA%\Narrivy\dev\whisper-tiny.en-Q8_0.gguf (see
# engines/transcribe-cpp/BUILD-NOTES.md for the pinned file).

param(
    [string]$Model = (Join-Path $env:LOCALAPPDATA "Narrivy\dev\whisper-tiny.en-Q8_0.gguf"),
    [string]$TargetDir = (Join-Path (Split-Path $PSScriptRoot) "target")
)
$ErrorActionPreference = "Stop"
$repo = Split-Path $PSScriptRoot
if (-not (Test-Path $Model)) { throw "dev-daemon: model not found: $Model" }

$env:CARGO_TARGET_DIR = $TargetDir
cargo build --locked --manifest-path "$repo\Cargo.toml" -p narrivyd --features asr-transcribe-resident
cargo build --locked --manifest-path "$repo\Cargo.toml" -p narrivy-cli
# A self-contained worker (static CRT). `--target` keeps the static-CRT flag
# off the build scripts (see BUILD-NOTES.md, "Windows").
$worker_target = Join-Path $TargetDir "worker"
$env:CARGO_TARGET_DIR = $worker_target
$env:RUSTFLAGS = "-C target-feature=+crt-static"
$env:TRANSCRIBE_CMAKE_ARGS = "-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded"
cargo build --release --locked --target x86_64-pc-windows-msvc --manifest-path "$repo\Cargo.toml" `
    -p narrivy-asr-transcribe --features resident-worker --bin narrivy-transcribe-worker
Remove-Item Env:RUSTFLAGS, Env:TRANSCRIBE_CMAKE_ARGS

# The worker must be a user-owned file nobody else can write: copy it into
# the per-user folder (owner-only by inheritance).
$dev = Join-Path $env:LOCALAPPDATA "Narrivy\dev"
New-Item -ItemType Directory -Force $dev | Out-Null
$worker = Join-Path $dev "narrivy-transcribe-worker.exe"
Copy-Item (Join-Path $worker_target "x86_64-pc-windows-msvc\release\narrivy-transcribe-worker.exe") $worker -Force

$env:NARRIVY_RECOVERY_MODE = "linux-mic-routed-inbox"
$env:NARRIVY_DEFAULT_PIPELINE = "clean"
$env:NARRIVY_ASR_ENGINE = "transcribe-cpp"
$env:NARRIVY_AUDIO_SOURCE = "wasapi"
$env:NARRIVY_DELIVERY_BACKEND = "routed-inbox"
$env:NARRIVY_TRANSCRIBE_RUNTIME = "resident"
$env:NARRIVY_TRANSCRIBE_WORKER = $worker
$env:NARRIVY_TRANSCRIBE_WORKER_SHA256 = (Get-FileHash -Algorithm SHA256 $worker).Hash.ToLower()
$env:NARRIVY_TRANSCRIBE_WORKER_SIZE = (Get-Item $worker).Length
$env:NARRIVY_TRANSCRIBE_MODEL = $Model
$env:NARRIVY_TRANSCRIBE_MODEL_SHA256 = (Get-FileHash -Algorithm SHA256 $Model).Hash.ToLower()
$env:NARRIVY_TRANSCRIBE_MODEL_SIZE = (Get-Item $Model).Length

& (Join-Path $TargetDir "debug\narrivyd.exe")
