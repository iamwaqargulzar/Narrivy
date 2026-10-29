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
# check the bundled model against its registry pin; run the reviewed builder
# (scripts/build_deb_installable.py --clean-source); publish to the fixed path
# the recorders read (artifacts/releases/narrivy-linux-amd64.deb, the
# previous version moved to narrivy-previous-amd64.deb); build the
# unqualified supply-chain bundle; write artifacts/candidates/VERSION-build.json.
#
# Environment:
#   NARRIVY_MODEL  the pinned whisper-tiny.en-Q8_0.gguf
#                  (default ~/.cache/narrivy/models/whisper-tiny.en-Q8_0.gguf)
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
command -v glslc >/dev/null || die "glslc is missing (needed for the Vulkan worker)"
[[ -e /usr/include/vulkan/vulkan.h ]] || die "the Vulkan headers are missing"

model="${NARRIVY_MODEL:-$HOME/.cache/narrivy/models/whisper-tiny.en-Q8_0.gguf}"
[[ -f "$model" ]] || die "model not found: $model (set NARRIVY_MODEL)"
# Never trust the path alone: the registry pin decides.
python3 -I - "$repo/crates/narrivy-model-manager/seed/registry.json" "$model" <<'EOF' \
    || die "the model does not match its registry pin"
import hashlib, json, os, sys
registry, path = sys.argv[1], sys.argv[2]
with open(registry, encoding="utf-8") as handle:
    data = json.load(handle)
models = data["entries"]
pin = next(m for m in models if m["id"] == "whisper-tiny.en-Q8_0")
digest = hashlib.sha256()
with open(path, "rb") as handle:
    for block in iter(lambda: handle.read(1 << 20), b""):
        digest.update(block)
ok = os.path.getsize(path) == pin["size_bytes"] and digest.hexdigest() == pin["sha256"]
sys.exit(0 if ok else 1)
EOF

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
    --model "$model" \
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
python3 scripts/build_candidate_supply_chain.py \
    --candidate "$output" --source-root "$repo" --output "$bundle" \
    --expected-sha256 "$package_sha" \
    --builder-os "$builder_os" --builder-image host \
    --builder-image-digest unverified-host-build \
    --toolchain "$toolchain"

record="$candidates/$version-build.json"
python3 -I - "$record" "$version" "$source_head" "$source_date_epoch" "$toolchain" "$output" \
    "$target/release/narrivyd" "$target/release/narrivy-cli" \
    "$target/release/narrivy-desktop" "$target/release/narrivy-transcribe-worker" "$model" <<'EOF'
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
