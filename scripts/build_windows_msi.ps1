# Build Narrivy's Windows installer: one per-machine MSI (owner decision
# D47, 2026-10-27). Run on Windows from the repository root, in an ordinary
# (unelevated) PowerShell:
#
#   powershell -ExecutionPolicy Bypass -File scripts\build_windows_msi.ps1
#
# Needs: the MSVC build tools (set $env:CMAKE to their CMake), Rust with the
# x86_64-pc-windows-msvc target, tauri-cli 2 (`cargo install tauri-cli
# --version "^2" --locked`), the Vulkan SDK (`$env:VULKAN_SDK`, with `glslc`),
# and the built frontend in apps\narrivy-desktop\dist. The
# Tauri bundler fetches WiX on first use.
#
# The worker is always built with Vulkan (D25, ADR-0076): it loads the model
# on a discrete or integrated GPU that passes its self-test and otherwise on
# the CPU. It links the system's Vulkan loader (`vulkan-1.dll`, installed
# with every current GPU driver), as the Linux package depends on libvulkan1.
#
# Every binary links the C runtime statically (no VC++ redistributable on
# the target machine) and is built with an explicit --target, which keeps
# the static-CRT flag off build scripts (engines/transcribe-cpp/BUILD-NOTES.md,
# "Windows"). The daemon, CLI, worker and installed-runtime manifest
# are staged beside the app, where the installed daemon verifies them
# (adapters/platform-windows/src/installed_runtime.rs).
#
# The language-model worker (clean-up and the assistant, D48) is staged the
# same way with its own llm-worker-manifest.json. It needs no OpenVINO at
# build time: it loads the OpenVINO runtime pack the user downloads from
# Models (scripts/build_openvino_runtime_pack.py --platform windows-x86_64).
# The OpenVINO speech worker (Whisper on the NPU or GPU) is staged the same
# way with openvino-worker-manifest.json and loads the same runtime pack.
# Voice mode's worker (D34) is staged with voice-worker-manifest.json and the
# Silero detector in voice\; its voice is a Store download.
# Voice mode's Urdu speaker (GPL-3.0-or-later, D51) is built from its own
# workspace (engines\voice-urdu) and staged with urdu-speaker-manifest.json,
# its licence and its source note in licenses\narrivy-urdu-speaker\.

param(
    [string]$TargetDir = (Join-Path (Get-Location) "target\msi"),
    [int]$Jobs = 3
)
$ErrorActionPreference = "Stop"
$repo = Split-Path $PSScriptRoot
$triple = "x86_64-pc-windows-msvc"

function Invoke-Checked([scriptblock]$Command, [string]$What) {
    & $Command
    if ($LASTEXITCODE -ne 0) { throw "build_windows_msi: $What failed ($LASTEXITCODE)" }
}

# The MSI ships no speech model (D32, D40, as the Linux package): the user
# downloads one from the Store on first run.

# MSI versions are numbers only, and upgrades compare the first three:
# 1.0.0-rc.N installs as 0.99.N, so every candidate upgrades the one before
# and 1.0.0 upgrades them all.
$conf = Get-Content (Join-Path $repo "apps\narrivy-desktop\src-tauri\tauri.conf.json") -Raw | ConvertFrom-Json
$version = $conf.version
if ($version -match '^(\d+)\.(\d+)\.(\d+)$') {
    $msiVersion = $version
} elseif ($version -match '^1\.0\.0-rc\.(\d+)$') {
    $msiVersion = "0.99.$($Matches[1])"
} else {
    throw "build_windows_msi: no MSI version for $version"
}

if (-not $env:VULKAN_SDK -or -not (Test-Path (Join-Path $env:VULKAN_SDK "Include\vulkan\vulkan.h"))) {
    throw "build_windows_msi: the Vulkan SDK is missing (set `$env:VULKAN_SDK)"
}
if (-not (Get-Command glslc -ErrorAction SilentlyContinue)) {
    $env:PATH = "$(Join-Path $env:VULKAN_SDK 'Bin');$env:PATH"
    if (-not (Get-Command glslc -ErrorAction SilentlyContinue)) { throw "build_windows_msi: glslc is missing (needed for the Vulkan worker)" }
}

# The frontend is built separately (`npm run build`); refuse one left over
# from another version, which would ship that version's screens. The bundler
# quotes the version with " or with a backquote, depending on its release.
$dist = Join-Path $repo "apps\narrivy-desktop\dist\assets"
$quotedVersion = "[`"``]$([regex]::Escape($version))[`"``]"
if (-not (Test-Path $dist) -or -not (Select-String -Path (Join-Path $dist "*.js") -Pattern $quotedVersion -Quiet)) {
    throw "build_windows_msi: apps\narrivy-desktop\dist is missing or not built for $version (run npm run build)"
}

$env:CARGO_TARGET_DIR = $TargetDir
# The engine's build script asks for `vulkan-1.lib` without its folder.
$env:RUSTFLAGS = "-C target-feature=+crt-static -L native=$(Join-Path $env:VULKAN_SDK 'Lib')"
$env:TRANSCRIBE_CMAKE_ARGS = "-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded"
try {
    Invoke-Checked { cargo build -j $Jobs --release --locked --target $triple --manifest-path "$repo\Cargo.toml" -p narrivyd --features asr-transcribe-resident } "daemon"
    Invoke-Checked { cargo build -j $Jobs --release --locked --target $triple --manifest-path "$repo\Cargo.toml" -p narrivy-cli } "CLI"
    Invoke-Checked { cargo build -j $Jobs --release --locked --target $triple --manifest-path "$repo\Cargo.toml" -p narrivy-asr-transcribe --features vulkan --bin narrivy-transcribe-worker } "worker"
    # The same worker for the processor only, for a computer with no Vulkan
    # loader (vulkan-1.dll comes with the graphics driver; the Vulkan worker
    # cannot start without it). Its own target folder, so the two builds of
    # one binary don't replace each other.
    $cpuTarget = Join-Path $TargetDir "cpu-worker"
    Invoke-Checked { cargo build -j $Jobs --release --locked --target $triple --target-dir $cpuTarget --manifest-path "$repo\Cargo.toml" -p narrivy-asr-transcribe --features resident-worker --bin narrivy-transcribe-worker } "processor-only worker"
    Invoke-Checked { cargo build -j $Jobs --release --locked --target $triple --manifest-path "$repo\Cargo.toml" -p narrivy-asr-openvino --features worker --bin narrivy-openvino-llm-worker } "language-model worker"
    Invoke-Checked { cargo build -j $Jobs --release --locked --target $triple --manifest-path "$repo\Cargo.toml" -p narrivy-asr-openvino --features worker --bin narrivy-openvino-worker } "OpenVINO speech worker"
    Invoke-Checked { cargo build -j $Jobs --release --locked --target $triple --manifest-path "$repo\Cargo.toml" -p narrivy-voice-sherpa --features worker --bin narrivy-voice-worker } "voice worker"
    Invoke-Checked { cargo build -j $Jobs --release --locked --target $triple --manifest-path "$repo\engines\voice-urdu\Cargo.toml" } "Urdu speaker"
    Invoke-Checked { cargo build -j $Jobs --release --locked --target $triple --manifest-path "$repo\Cargo.toml" -p narrivy-browser-host } "browser connector"

    $out = Join-Path $TargetDir "$triple\release"
    $stage = Join-Path $repo "apps\narrivy-desktop\src-tauri\msi-runtime"
    if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
    New-Item -ItemType Directory -Force $stage | Out-Null
    foreach ($name in "narrivyd.exe", "narrivy-cli.exe", "narrivy-transcribe-worker.exe", "narrivy-openvino-llm-worker.exe", "narrivy-openvino-worker.exe", "narrivy-voice-worker.exe", "narrivy-urdu-speaker.exe", "narrivy-browser-host.exe") {
        Copy-Item (Join-Path $out $name) (Join-Path $stage $name)
    }
    Copy-Item (Join-Path $cpuTarget "$triple\release\narrivy-transcribe-worker.exe") (Join-Path $stage "narrivy-transcribe-worker-cpu.exe")
    # The writing assistant's unpacked extension, the same files as the deb's
    # (all but the README), loaded from the install folder.
    $extension = Join-Path $stage "browser-extension"
    Copy-Item -Recurse (Join-Path $repo "apps\narrivy-browser-extension") $extension
    Remove-Item (Join-Path $extension "README.md")
    # Voice mode's Silero detector (MIT), checked by the daemon's digest.
    New-Item -ItemType Directory -Force (Join-Path $stage "voice") | Out-Null
    Copy-Item (Join-Path $repo "packaging\voice\silero_vad.onnx") (Join-Path $stage "voice\silero_vad.onnx")
    # The Urdu speaker's licence and where its source is (GPL-3.0).
    $urduLicenses = Join-Path $stage "licenses\narrivy-urdu-speaker"
    New-Item -ItemType Directory -Force $urduLicenses | Out-Null
    Copy-Item (Join-Path $repo "engines\voice-urdu\COPYING") (Join-Path $urduLicenses "COPYING")
    Copy-Item (Join-Path $repo "engines\voice-urdu\SOURCE") (Join-Path $urduLicenses "SOURCE")
    # The echo canceller's licence (sonora, BSD-3-Clause): binary
    # redistribution keeps its notice, conditions and disclaimer.
    $sonoraLicenses = Join-Path $stage "licenses\sonora"
    New-Item -ItemType Directory -Force $sonoraLicenses | Out-Null
    Copy-Item (Join-Path $repo "packaging\licenses\sonora\LICENSE") (Join-Path $sonoraLicenses "LICENSE")
    # Each worker's manifest names its exact bytes (same schema as Linux).
    foreach ($pair in @(
            @("narrivy-transcribe-worker.exe", "transcribe.cpp", "installed-runtime-manifest.json"),
            @("narrivy-transcribe-worker-cpu.exe", "transcribe.cpp", "installed-runtime-cpu-manifest.json"),
            @("narrivy-openvino-llm-worker.exe", "openvino-llm", "llm-worker-manifest.json"),
            @("narrivy-openvino-worker.exe", "openvino", "openvino-worker-manifest.json"),
            @("narrivy-voice-worker.exe", "voice", "voice-worker-manifest.json"),
            @("narrivy-urdu-speaker.exe", "urdu-speaker", "urdu-speaker-manifest.json"))) {
        $worker = Join-Path $stage $pair[0]
        $manifest = [ordered]@{
            schema_version = 1
            runtime_id = $pair[1]
            worker = [ordered]@{
                sha256 = (Get-FileHash -Algorithm SHA256 $worker).Hash.ToLower()
                size_bytes = (Get-Item $worker).Length
            }
        }
        [System.IO.File]::WriteAllText((Join-Path $stage $pair[2]),
            ($manifest | ConvertTo-Json -Compress), (New-Object System.Text.UTF8Encoding $false))
    }

    # The staged runtime goes beside the app only in the MSI (an ordinary
    # Windows build of the app has no staging folder), with the MSI version.
    $override = Join-Path $TargetDir "msi-config.json"
    [System.IO.File]::WriteAllText($override,
        "{`"bundle`":{`"resources`":{`"msi-runtime`":`".`"},`"windows`":{`"wix`":{`"version`":`"$msiVersion`"}}}}",
        (New-Object System.Text.UTF8Encoding $false))
    Push-Location (Join-Path $repo "apps\narrivy-desktop\src-tauri")
    try {
        Invoke-Checked { cargo tauri build --target $triple --bundles msi --config $override -- -j $Jobs --locked } "app and MSI"
    } finally {
        Pop-Location
    }
} finally {
    Remove-Item Env:RUSTFLAGS, Env:TRANSCRIBE_CMAKE_ARGS -ErrorAction SilentlyContinue
}

# Pick this version's MSI by name: older MSIs stay in the same folder and the
# VM's clock can make one of them look newer than the build just made.
$msiPath = Join-Path $TargetDir "$triple\release\bundle\msi\Narrivy_${version}_x64_en-US.msi"
if (-not (Test-Path $msiPath)) { throw "MSI for $version not found: $msiPath" }
$msi = Get-Item $msiPath
Write-Output "msi:     $($msi.FullName)"
Write-Output "version: $version (MSI $msiVersion)"
Write-Output "sha256:  $((Get-FileHash -Algorithm SHA256 $msi.FullName).Hash.ToLower())"
