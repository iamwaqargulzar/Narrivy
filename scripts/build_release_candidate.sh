#!/usr/bin/env bash
# Build the release-candidate .deb from a clean commit (WP3 T3.1).
#
# Usage:
#   scripts/build_release_candidate.sh VERSION
#     VERSION  D3 form: X.Y.Z~rcN or X.Y.Z (for example 1.0.0~rc1)
#
# Steps: refuse a dirty tree; build the front end; build the release binaries
# into a fresh target directory (target/release-candidate, emptied first, so a
# second run on the same commit rebuilds from nothing at the same paths);
# run the reviewed builder
# (scripts/build_deb_installable.py --clean-source); publish to the fixed path
# the recorders read (artifacts/releases/narrivy-linux-amd64.deb, the
# previous version moved to narrivy-previous-amd64.deb); build the
# unqualified supply-chain bundle; write artifacts/candidates/VERSION-build.json.
#
# The package is not qualified by this script: it is the input to the native
# smoke test (T3.3) and the release-gate recorders.

set -euo pipefail
# Build outputs follow the umask, and the builder accepts only 0755 binaries
# (a group-writable worker is refused).
umask 022

die() {
    echo "build_release_candidate: $*" >&2
    exit 1
}

[[ $# -eq 1 ]] || die "usage: scripts/build_release_candidate.sh VERSION (for example 1.0.0~rc1)"
version="$1"
# The same rule as VERSION_RE in build_deb_installable.py (D3).
[[ "$version" =~ ^(0|[1-9][0-9]{0,3})(\.(0|[1-9][0-9]{0,3})){2}(~rc[1-9][0-9]{0,3})?$ ]] \
    || die "version must be X.Y.Z or X.Y.Z~rcN (D3): $version"
# Every package must upgrade the 0.2.3 build already installed on the
# reference machine.
dpkg --compare-versions "$version" gt 0.2.3 || die "version must sort above 0.2.3: $version"

repo="$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"
cd "$repo"
[[ -z "$(git status --porcelain=v1 --untracked-files=all)" ]] \
    || die "the source tree is not clean; commit or stash first"
# One version (F-M12): the app shows its Tauri version (About, the version
# button), the daemon reports its crate version and support reports the
# CLI's. Each must be this package's version in semver form (1.0.0~rc2 is
# 1.0.0-rc.2).
semver="${version/\~rc/-rc.}"
python3 -I - "$semver" <<'EOF_VERSIONS' || die "set every version above to $semver for $version"
import json, re, sys
want = sys.argv[1]
found = {"apps/narrivy-desktop/src-tauri/tauri.conf.json":
         json.load(open("apps/narrivy-desktop/src-tauri/tauri.conf.json"))["version"]}
for manifest in ("services/narrivyd/Cargo.toml", "apps/narrivy-cli/Cargo.toml",
                 "apps/narrivy-desktop/src-tauri/Cargo.toml"):
    match = re.search(r'^version = "([^"]+)"', open(manifest).read(), re.M)
    found[manifest] = match.group(1) if match else "?"
wrong = {path: got for path, got in found.items() if got != want}
for path, got in wrong.items():
    print(f"build_release_candidate: {path} has version {got}", file=sys.stderr)
sys.exit(1 if wrong else 0)
EOF_VERSIONS
source_head="$(git rev-parse --verify HEAD)"
source_date_epoch="$(git log -1 --format=%ct)"
export SOURCE_DATE_EPOCH="$source_date_epoch"
# `const-random` (through the grammar engine's burn-ndarray) bakes a random
# 256-bit value into narrivyd unless it is seeded; the commit seeds it, so
# two builds of one commit are byte-identical. It is a hash seed, not a secret.
export CONST_RANDOM_SEED="$source_head"

# The worker links the Vulkan loader (ADR-0076; the package Depends on
# libvulkan1), so the release worker is always built with Vulkan.
[[ -e /usr/include/vulkan/vulkan.h ]] || die "the Vulkan headers are missing"
# ggml-vulkan builds its integer dot-product shaders (and others) only when
# glslc supports the extension; without them speech recognition on the GPU is
# markedly slower. CMake takes glslc from $VULKAN_SDK/bin first, so a glslc
# too old (shaderc 2023.8 in Ubuntu 24.04) is replaced by an installed Vulkan
# SDK's or, failing that, a Flatpak SDK's.
glslc_supports_int_dot() {
    local probe
    probe="$(mktemp -d)"
    printf '%s\n' '#version 450' '#extension GL_EXT_integer_dot_product : require' \
        'void main() {}' >"$probe/probe.comp"
    local status=0
    "$1" -o "$probe/probe.spv" "$probe/probe.comp" >/dev/null 2>&1 || status=$?
    rm -rf "$probe"
    return "$status"
}
if [[ -z "${VULKAN_SDK:-}" ]] && ! glslc_supports_int_dot "$(command -v glslc || echo glslc)"; then
    export VULKAN_SDK="$repo/scripts/glslc-flatpak-sdk"
fi
glslc_bin="$(command -v glslc || true)"
[[ -n "${VULKAN_SDK:-}" ]] && glslc_bin="$VULKAN_SDK/bin/glslc"
glslc_supports_int_dot "$glslc_bin" ||
    die "no glslc with integer dot-product support (needed for a fast Vulkan worker): set VULKAN_SDK to a Vulkan SDK, or install the Flatpak org.freedesktop.Sdk 25.08 or later"
echo "build_release_candidate: glslc $("$glslc_bin" --version | head -1)"

echo "build_release_candidate: $version from $source_head (epoch $source_date_epoch)"

# The desktop embeds the built front end at compile time.
(cd apps/narrivy-desktop && npm ci --no-audit --no-fund && npm run build)

target="$repo/target/release-candidate"
rm -rf "$target"
export CARGO_TARGET_DIR="$target"
cargo build --release --locked -p narrivyd --features asr-transcribe-resident
cargo build --release --locked -p narrivy-cli
# `tauri build` turns on custom-protocol: the app serves its embedded front
# end, never a development server.
cargo build --release --locked -p narrivy-desktop --features tauri/custom-protocol
cargo build --release --locked -p narrivy-asr-transcribe --features vulkan \
    --bin narrivy-transcribe-worker
# The language-model worker (clean-up and the AI assistant, D48) and the
# OpenVINO speech worker (the OpenVINO add-on's models, D25). Both load the
# OpenVINO runtime pack the Store installs, so they build without the SDK.
cargo build --release --locked -p narrivy-asr-openvino --features worker \
    --bin narrivy-openvino-llm-worker --bin narrivy-openvino-worker
# Voice mode's worker (listening and speaking, D34): sherpa-onnx linked
# statically (its build script downloads the pinned prebuilt archive).
cargo build --release --locked -p narrivy-voice-sherpa --features worker \
    --bin narrivy-voice-worker
# Voice mode's Urdu speaker (GPL-3.0-or-later, D51): its own workspace, so
# eSpeak NG never links into the MIT programs above.
cargo build --release --locked --manifest-path "$repo/engines/voice-urdu/Cargo.toml"
urdu_speaker="$target/release/narrivy-urdu-speaker"
# The writing assistant's browser connector (D26, D48).
cargo build --release --locked -p narrivy-browser-host

releases="$repo/artifacts/releases"
candidates="$repo/artifacts/candidates"
mkdir -p "$releases" "$candidates"
output="$releases/narrivy_${version}_amd64.deb"
rm -f "$output"

python3 scripts/build_deb_installable.py \
    --clean-source --source-root "$repo" \
    --source-head "$source_head" --source-date-epoch "$source_date_epoch" \
    --version "$version" \
    --narrivyd "$target/release/narrivyd" \
    --cli "$target/release/narrivy-cli" \
    --desktop "$target/release/narrivy-desktop" \
    --worker "$target/release/narrivy-transcribe-worker" \
    --llm-worker "$target/release/narrivy-openvino-llm-worker" \
    --openvino-worker "$target/release/narrivy-openvino-worker" \
    --voice-worker "$target/release/narrivy-voice-worker" \
    --voice-detector "$repo/packaging/voice/silero_vad.onnx" \
    --urdu-speaker "$urdu_speaker" \
    --urdu-license "$repo/engines/voice-urdu/COPYING" \
    --browser-host "$target/release/narrivy-browser-host" \
    --browser-extension "$repo/apps/narrivy-browser-extension" \
    --service "$repo/packaging/deb/narrivy-daemon.service" \
    --desktop-entry "$repo/packaging/deb/narrivy.desktop" \
    --postinst "$repo/packaging/deb/postinst" \
    --prerm "$repo/packaging/deb/prerm" \
    --postrm "$repo/packaging/deb/postrm" \
    --icon-32 "$repo/packaging/linux/icons/32x32/narrivy.png" \
    --icon-128 "$repo/packaging/linux/icons/128x128/narrivy.png" \
    --icon-256 "$repo/packaging/linux/icons/256x256/narrivy.png" \
    --icon-512 "$repo/packaging/linux/icons/512x512/narrivy.png" \
    --license-file "$repo/LICENSE" \
    --notices "$repo/THIRD_PARTY_NOTICES.md" \
    --output "$output"

package_sha="$(sha256sum "$output" | cut -d' ' -f1)"
fixed="$releases/narrivy-linux-amd64.deb"
previous="$releases/narrivy-previous-amd64.deb"
# The previous package is the upgrade recorder's PREVIOUS input: an older
# version. A rebuild of the same version replaces the fixed path only.
if [[ -f "$fixed" ]] && [[ "$(dpkg-deb --field "$fixed" Version)" != "$version" ]]; then
    mv -f "$fixed" "$previous"
fi
cp -f "$output" "$fixed"

toolchain="$(rustc --version); $(cargo --version); node $(node --version); npm $(npm --version)"
builder_os="$(. /etc/os-release && echo "$PRETTY_NAME") $(uname -r)"
bundle="$candidates/$version"
rm -rf "$bundle"
# The SBOM reads licences with an offline `cargo metadata --all-features`,
# which needs every locked crate (all targets) in the Cargo cache.
cargo fetch --locked
python3 scripts/build_candidate_supply_chain.py \
    --candidate "$output" --source-root "$repo" --output "$bundle" \
    --expected-sha256 "$package_sha" \
    --builder-os "$builder_os" --builder-image host \
    --builder-image-digest unverified-host-build \
    --toolchain "$toolchain"

record="$candidates/$version-build.json"
python3 -I - "$record" "$version" "$source_head" "$source_date_epoch" "$toolchain" "$output" \
    "$target/release/narrivyd" "$target/release/narrivy-cli" \
    "$target/release/narrivy-desktop" "$target/release/narrivy-transcribe-worker" \
    "$target/release/narrivy-openvino-llm-worker" "$target/release/narrivy-openvino-worker" \
    "$target/release/narrivy-voice-worker" "$urdu_speaker" \
    "$target/release/narrivy-browser-host" <<'EOF'
import hashlib, json, os, sys
record, version, head, epoch, toolchain, output, *inputs = sys.argv[1:]
def sha(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return {"sha256": digest.hexdigest(), "size_bytes": os.path.getsize(path)}
payload = {
    "schema": "narrivy-release-candidate-build/v1",
    "version": version,
    "source_head": head,
    "source_date_epoch": int(epoch),
    "toolchain": toolchain,
    "inputs": {os.path.basename(path): sha(path) for path in inputs},
    "output": {"path": os.path.relpath(output), **sha(output)},
}
with open(record, "w", encoding="utf-8") as handle:
    json.dump(payload, handle, indent=2, sort_keys=True)
    handle.write("\n")
EOF

echo "package:  $output"
echo "sha256:   $package_sha"
echo "fixed:    $fixed"
if [[ -f "$previous" ]]; then
    echo "previous: $previous $(sha256sum "$previous" | cut -d' ' -f1)"
fi
echo "record:   $record"
