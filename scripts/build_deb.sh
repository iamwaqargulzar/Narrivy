#!/usr/bin/env bash
# Build the Narrivy .deb (single Linux 1.0 artifact — owner decision 2026-08-22)
# P10 reproducible build: dpkg-deb --root-owner-group, SOURCE_DATE_EPOCH from git log,
# version from Cargo/tauri, build-info embedded, permissions 0755/0644.
#
# Usage:
#   scripts/build_deb.sh [VERSION]
#     VERSION package version, e.g. 0.1.0 (default: from tauri.conf.json or Cargo.toml or git tag)
#
# Prerequisites: cargo build --release -p narrivyd -p narrivy-cli (and narrivy-desktop if Tauri binary present)
# Output:        artifacts/releases/narrivy_${VERSION}_amd64.deb (+ sha256 on stdout)
#
# Reproducibility:
#   SOURCE_DATE_EPOCH is set from `git log -1 --format=%ct` when not provided externally.
#   All staged file mtimes are clamped to SOURCE_DATE_EPOCH for deterministic archives.
#   dpkg-deb --root-owner-group ensures owner/group 0:0.
#   changelog.gz uses gzip -n; control perms 0644, maintainer scripts 0755.
# See docs/maintainers/operations/reproducible-builder.md for immutable builder baseline recording,
#     docs/maintainers/operations/release-packaging.md for XDG/data lifecycle contract.

set -euo pipefail

# Recovery quarantine (ADR-0062): this historical builder is not an authority
# for any package or release artifact. It permits missing/debug substitutions,
# stub provenance, and protected-output mutation. Keep the implementation
# below for forensic comparison only; fail before resolving inputs or touching
# the filesystem until a reviewed replacement owns the release path.
echo "error: legacy Debian builder is quarantined; no release package was created" >&2
echo "use scripts/build_deb_candidate.py only for disposable, install-blocked composition checks" >&2
exit 2

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel 2>/dev/null || true)"
if [[ -z "$ROOT" ]]; then
  ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
fi

# --- version resolution ---
if [[ $# -ge 1 && -n "${1:-}" ]]; then
  VERSION="$1"
elif [[ "${GITHUB_REF_TYPE:-}" == "tag" && -n "${GITHUB_REF_NAME:-}" ]]; then
  VERSION="${GITHUB_REF_NAME#v}"
else
  # Try tauri.conf.json, then Cargo.toml workspace version, then crate version
  VERSION="$(python3 -c 'import json,pathlib;print(json.load(open("apps/narrivy-desktop/src-tauri/tauri.conf.json"))["version"])' 2>/dev/null || true)"
  if [[ -z "$VERSION" || "$VERSION" == "null" ]]; then
    VERSION="$(python3 -c '
import pathlib, re
for p in ["Cargo.toml", "apps/narrivy-desktop/src-tauri/Cargo.toml", "services/narrivyd/Cargo.toml"]:
    try:
        txt=pathlib.Path(p).read_text()
        m=re.search(r"^version\s*=\s*"([^"]+)"", txt, re.M)
        if m and m.group(1)!="0.0.0":
            print(m.group(1)); raise SystemExit(0)
    except: pass
' 2>/dev/null || true)"
  fi
  if [[ -z "$VERSION" || "$VERSION" == "null" ]]; then
    VERSION="$(grep -m1 '^version' "$ROOT/Cargo.toml" 2>/dev/null | sed -E 's/.*"([^"]+)".*/\1/' || true)"
  fi
  # fallback to 0.1.0 skeleton version
  [[ -n "$VERSION" ]] || VERSION="0.1.0"
fi
if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+([+~.-][0-9A-Za-z.+-]*)?$ ]]; then
  echo "error: invalid version '$VERSION'" >&2
  exit 1
fi

PKG_DIR="$ROOT/packaging/linux"
DEB_CTRL_DIR="$ROOT/packaging/deb"
BIN_DIR="$ROOT/target/release"
STAGE="$(mktemp -d /tmp/narrivy-deb.XXXXXX)"
trap 'rm -rf "$STAGE"' EXIT

echo "[build_deb] version=$VERSION root=$ROOT stage=$STAGE" >&2

# --- reproducible epoch: SOURCE_DATE_EPOCH from git log when not set ---
if [[ -z "${SOURCE_DATE_EPOCH:-}" || "${SOURCE_DATE_EPOCH:-0}" == "0" ]]; then
  GIT_EPOCH="$(git -C "$ROOT" log -1 --format=%ct 2>/dev/null || echo 0)"
  if [[ "$GIT_EPOCH" != "0" && -n "$GIT_EPOCH" ]]; then
    export SOURCE_DATE_EPOCH="$GIT_EPOCH"
    echo "[build_deb] SOURCE_DATE_EPOCH set from git log: $SOURCE_DATE_EPOCH" >&2
  else
    export SOURCE_DATE_EPOCH="0"
    echo "[build_deb] SOURCE_DATE_EPOCH fallback 0 (no git history)" >&2
  fi
else
  echo "[build_deb] SOURCE_DATE_EPOCH inherited: $SOURCE_DATE_EPOCH" >&2
fi

# --- verify inputs (skeleton allows missing binaries with warning; real gate requires them) ---
MISSING=0
for bin in narrivyd narrivy-cli narrivy-desktop; do
  if [[ ! -x "$BIN_DIR/$bin" ]]; then
    echo "warn: $BIN_DIR/$bin missing — run: cargo build --release -p narrivyd -p narrivy-cli -p narrivy-desktop" >&2
    MISSING=1
  fi
done
if [[ $MISSING -ne 0 ]]; then
  echo "warn: skeleton mode — creating outer .deb structure without binaries (real build will fail gate)" >&2
fi

# --- stage tree in /tmp ---
install -d "$STAGE/DEBIAN"
install -d "$STAGE/usr/bin"
install -d "$STAGE/usr/lib/systemd/user"
install -d "$STAGE/usr/share/applications"
install -d "$STAGE/usr/share/doc/narrivy"
install -d "$STAGE/usr/share/icons/hicolor/32x32/apps"
install -d "$STAGE/usr/share/icons/hicolor/128x128/apps"
install -d "$STAGE/usr/share/icons/hicolor/256x256/apps"
install -d "$STAGE/usr/share/icons/hicolor/512x512/apps"
install -d "$STAGE/usr/share/narrivy"

# Binaries — only if present (skeleton-tolerant), ensure 0755
if [[ -x "$BIN_DIR/narrivyd" ]]; then
  install -Dm755 "$BIN_DIR/narrivyd" "$STAGE/usr/bin/narrivyd"
  chmod 0755 "$STAGE/usr/bin/narrivyd"
fi
if [[ -x "$BIN_DIR/narrivy-cli" ]]; then
  install -Dm755 "$BIN_DIR/narrivy-cli" "$STAGE/usr/bin/narrivy-cli"
  chmod 0755 "$STAGE/usr/bin/narrivy-cli"
fi
if [[ -x "$BIN_DIR/narrivy-desktop" ]]; then
  install -Dm755 "$BIN_DIR/narrivy-desktop" "$STAGE/usr/bin/narrivy-desktop"
  chmod 0755 "$STAGE/usr/bin/narrivy-desktop"
fi
# Fallback: if release binaries missing but debug exist, use debug with warning
if [[ ! -x "$STAGE/usr/bin/narrivyd" && -x "$ROOT/target/debug/narrivyd" ]]; then
  echo "warn: using debug narrivyd as fallback" >&2
  install -Dm755 "$ROOT/target/debug/narrivyd" "$STAGE/usr/bin/narrivyd"
  chmod 0755 "$STAGE/usr/bin/narrivyd"
fi
if [[ ! -x "$STAGE/usr/bin/narrivy-cli" && -x "$ROOT/target/debug/narrivy-cli" ]]; then
  echo "warn: using debug narrivy-cli as fallback" >&2
  install -Dm755 "$ROOT/target/debug/narrivy-cli" "$STAGE/usr/bin/narrivy-cli"
  chmod 0755 "$STAGE/usr/bin/narrivy-cli"
fi
if [[ ! -x "$STAGE/usr/bin/narrivy-desktop" && -x "$ROOT/target/debug/narrivy-desktop" ]]; then
  echo "warn: using debug narrivy-desktop as fallback" >&2
  install -Dm755 "$ROOT/target/debug/narrivy-desktop" "$STAGE/usr/bin/narrivy-desktop"
  chmod 0755 "$STAGE/usr/bin/narrivy-desktop"
fi

# Systemd user service — the .deb uses systemd --user, not system daemon (ADR-0012), 0644
if [[ -f "$PKG_DIR/deb/narrivy-daemon.service" ]]; then
  install -Dm644 "$PKG_DIR/deb/narrivy-daemon.service" "$STAGE/usr/lib/systemd/user/narrivy-daemon.service"
elif [[ -f "$DEB_CTRL_DIR/narrivy-daemon.service" ]]; then
  install -Dm644 "$DEB_CTRL_DIR/narrivy-daemon.service" "$STAGE/usr/lib/systemd/user/narrivy-daemon.service"
else
  echo "error: narrivy-daemon.service not found" >&2
  exit 1
fi
chmod 0644 "$STAGE/usr/lib/systemd/user/narrivy-daemon.service"

# Desktop entry 0644
if [[ -f "$PKG_DIR/desktop/narrivy.desktop" ]]; then
  install -Dm644 "$PKG_DIR/desktop/narrivy.desktop" "$STAGE/usr/share/applications/narrivy.desktop"
elif [[ -f "$DEB_CTRL_DIR/narrivy.desktop" ]]; then
  install -Dm644 "$DEB_CTRL_DIR/narrivy.desktop" "$STAGE/usr/share/applications/narrivy.desktop"
elif [[ -f "$PKG_DIR/deb/narrivy.desktop" ]]; then
  install -Dm644 "$PKG_DIR/deb/narrivy.desktop" "$STAGE/usr/share/applications/narrivy.desktop"
else
  echo "error: narrivy.desktop not found" >&2
  exit 1
fi
chmod 0644 "$STAGE/usr/share/applications/narrivy.desktop"

# Icons 0644
for size in 32x32 128x128 256x256; do
  src="$PKG_DIR/icons/$size/narrivy.png"
  [[ -f "$src" ]] && install -Dm644 "$src" "$STAGE/usr/share/icons/hicolor/$size/apps/narrivy.png" || true
  [[ -f "$STAGE/usr/share/icons/hicolor/$size/apps/narrivy.png" ]] && chmod 0644 "$STAGE/usr/share/icons/hicolor/$size/apps/narrivy.png" || true
done
# 512x512 fallback from root icon if present
if [[ -f "$PKG_DIR/icons/narrivy.png" ]]; then
  install -Dm644 "$PKG_DIR/icons/narrivy.png" "$STAGE/usr/share/icons/hicolor/512x512/apps/narrivy.png" 2>/dev/null || true
  chmod 0644 "$STAGE/usr/share/icons/hicolor/512x512/apps/narrivy.png" 2>/dev/null || true
fi
if [[ -f "$PKG_DIR/icons/512x512/narrivy.png" ]]; then
  install -Dm644 "$PKG_DIR/icons/512x512/narrivy.png" "$STAGE/usr/share/icons/hicolor/512x512/apps/narrivy.png" 2>/dev/null || true
  chmod 0644 "$STAGE/usr/share/icons/hicolor/512x512/apps/narrivy.png" 2>/dev/null || true
fi

# SBOM / build-info / notices (generated by scripts/generate_sbom.sh)
# Ensure build-info exists with git commit + rustc version
if [[ ! -f "$ROOT/artifacts/sbom/build-info.json" ]]; then
  echo "[build_deb] generating missing build-info.json via generate_sbom.sh" >&2
  "$ROOT/scripts/generate_sbom.sh" "$VERSION" 2>/dev/null || true
fi
if [[ -f "$ROOT/artifacts/sbom/build-info.json" ]]; then
  install -Dm644 "$ROOT/artifacts/sbom/build-info.json" "$STAGE/usr/share/narrivy/build-info.json"
  chmod 0644 "$STAGE/usr/share/narrivy/build-info.json"
else
  # Fallback minimal build-info with git + rustc
  GIT_COMMIT="$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo unknown)"
  RUSTC_VER="$(rustc --version 2>/dev/null || echo 'rustc unknown')"
  CARGO_VER="$(cargo --version 2>/dev/null || echo 'cargo unknown')"
  LOCK_HASH="$(sha256sum "$ROOT/Cargo.lock" 2>/dev/null | awk '{print $1}' || echo unknown)"
  cat > "$STAGE/usr/share/narrivy/build-info.json" <<JSON
{
  "version": "$VERSION",
  "commit": "$GIT_COMMIT",
  "commit_short": "${GIT_COMMIT:0:7}",
  "build_id": "$VERSION-${GIT_COMMIT:0:7}",
  "rustc_version": "$RUSTC_VER",
  "cargo_version": "$CARGO_VER",
  "cargo_lock_hash": "$LOCK_HASH",
  "source_date_epoch": "$SOURCE_DATE_EPOCH",
  "builder": {
    "os": "${BUILDER_OS:-Ubuntu 22.04.5 LTS}",
    "image": "${BUILDER_IMAGE:-ghcr.io/narrivy/builder:stub}",
    "image_digest": "${BUILDER_DIGEST:-sha256:0000000000000000000000000000000000000000000000000000000000000000000000}",
    "toolchain": "$RUSTC_VER + $CARGO_VER"
  }
}
JSON
  chmod 0644 "$STAGE/usr/share/narrivy/build-info.json"
fi
# Ensure build-info is valid JSON and contains required fields
if ! python3 -c "import json,pathlib; d=json.loads(pathlib.Path('$STAGE/usr/share/narrivy/build-info.json').read_text()); assert 'commit' in d and 'version' in d" 2>/dev/null; then
  echo "error: build-info.json invalid" >&2
  exit 1
fi

# THIRD_PARTY_NOTICES.md
if [[ -f "$ROOT/THIRD_PARTY_NOTICES.md" ]]; then
  install -Dm644 "$ROOT/THIRD_PARTY_NOTICES.md" "$STAGE/usr/share/narrivy/THIRD_PARTY_NOTICES.md" 2>/dev/null || true
  chmod 0644 "$STAGE/usr/share/narrivy/THIRD_PARTY_NOTICES.md" 2>/dev/null || true
else
  echo "# Third-Party Notices — stub (generated by cargo-deny / SBOM step)" > "$STAGE/usr/share/narrivy/THIRD_PARTY_NOTICES.md"
  chmod 0644 "$STAGE/usr/share/narrivy/THIRD_PARTY_NOTICES.md"
fi
if [[ -f "$ROOT/artifacts/sbom/THIRD_PARTY_NOTICES.md" ]]; then
  install -Dm644 "$ROOT/artifacts/sbom/THIRD_PARTY_NOTICES.md" "$STAGE/usr/share/doc/narrivy/THIRD_PARTY_NOTICES.md" 2>/dev/null || true
  chmod 0644 "$STAGE/usr/share/doc/narrivy/THIRD_PARTY_NOTICES.md" 2>/dev/null || true
fi

# copyright file for Debian compliance 0644
if [[ -f "$ROOT/LICENSE" ]]; then
  install -Dm644 "$ROOT/LICENSE" "$STAGE/usr/share/doc/narrivy/copyright"
  chmod 0644 "$STAGE/usr/share/doc/narrivy/copyright"
else
  echo "Narrivy — MIT License (see THIRD_PARTY_NOTICES.md)" > "$STAGE/usr/share/doc/narrivy/copyright"
  chmod 0644 "$STAGE/usr/share/doc/narrivy/copyright"
fi

# control: substitute version from template (0644)
mkdir -p "$STAGE/DEBIAN"
if [[ -f "$DEB_CTRL_DIR/control" ]]; then
  sed "s/^Version: .*/Version: ${VERSION}/" "$DEB_CTRL_DIR/control" > "$STAGE/DEBIAN/control"
elif [[ -f "$PKG_DIR/deb/control" ]]; then
  sed "s/^Version: .*/Version: ${VERSION}/" "$PKG_DIR/deb/control" > "$STAGE/DEBIAN/control"
else
  echo "error: no control file found in packaging/deb/control or packaging/linux/deb/control" >&2
  exit 1
fi
chmod 0644 "$STAGE/DEBIAN/control"

# maintainer scripts 0755
for s in postinst postrm prerm; do
  # prefer packaging/deb then packaging/linux/scripts then packaging/linux/deb
  for cand in "$DEB_CTRL_DIR/$s" "$PKG_DIR/scripts/$s" "$PKG_DIR/deb/$s"; do
    if [[ -f "$cand" ]]; then
      install -m755 "$cand" "$STAGE/DEBIAN/$s"
      chmod 0755 "$STAGE/DEBIAN/$s"
      break
    fi
  done
done

# changelog (gzip -n for reproducibility — no timestamp) 0644
mkdir -p "$STAGE/usr/share/doc/narrivy"
CHG=""
for cand in "$DEB_CTRL_DIR/changelog" "$PKG_DIR/deb/changelog"; do
  [[ -f "$cand" ]] && CHG="$cand" && break
done
if [[ -n "$CHG" ]]; then
  {
    echo "narrivy (${VERSION}) noble; urgency=medium"
    echo
    tail -n +2 "$CHG"
  } | gzip -n -9 > "$STAGE/usr/share/doc/narrivy/changelog.gz"
  chmod 644 "$STAGE/usr/share/doc/narrivy/changelog.gz"
fi

# conffiles / triggers not needed for skeleton; XDG paths are per-user, not system conffiles

# normalize mtimes for reproducible builds (SOURCE_DATE_EPOCH honored by dpkg-deb >= 1.18.11)
if [[ "$SOURCE_DATE_EPOCH" != "0" ]]; then
  find "$STAGE" -exec touch -h -d "@$SOURCE_DATE_EPOCH" {} + 2>/dev/null || true
  echo "[build_deb] clamped mtimes to $SOURCE_DATE_EPOCH ($(date -u -d "@$SOURCE_DATE_EPOCH" 2>/dev/null || date -u -r "$SOURCE_DATE_EPOCH" 2>/dev/null || echo epoch))" >&2
else
  echo "[build_deb] SOURCE_DATE_EPOCH=0 leaving mtimes as-is (non-reproducible)" >&2
fi

# Ensure correct permissions for DEBIAN dir itself
chmod 0755 "$STAGE/DEBIAN"
# Ensure all dirs 0755 and files 0644/0755 as appropriate
find "$STAGE/usr" -type d -exec chmod 0755 {} + 2>/dev/null || true

# --- build ---
OUT_DIR="$ROOT/artifacts/releases"
mkdir -p "$OUT_DIR"
OUT="$OUT_DIR/narrivy_${VERSION}_amd64.deb"
rm -f "$OUT"
if ! command -v dpkg-deb >/dev/null 2>&1; then
  echo "error: dpkg-deb not found — install dpkg-dev" >&2
  exit 1
fi
dpkg-deb --build --root-owner-group "$STAGE" "$OUT"

# Verify permissions inside deb
echo "[build_deb] verifying package contents permissions" >&2
dpkg-deb --contents "$OUT" | head -n 30 >&2 || true
# Verify build-info present
if ! dpkg-deb --contents "$OUT" | grep -q "build-info.json"; then
  echo "error: build-info.json missing from .deb" >&2
  exit 1
fi

echo "built: $OUT" >&2
dpkg-deb --info "$OUT" | sed -n '1,30p' >&2 || true
sha256sum "$OUT"

# Append to BUILD_LOG.md
{
  echo ""
  echo "## $(date -u +%Y-%m-%dT%H:%M:%SZ) — build $VERSION"
  echo "- git commit: $(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo unknown)"
  echo "- SOURCE_DATE_EPOCH: $SOURCE_DATE_EPOCH"
  echo "- rustc: $(rustc --version 2>/dev/null || echo unknown)"
  echo "- cargo: $(cargo --version 2>/dev/null || echo unknown)"
  echo "- artifact: $OUT"
  echo "- sha256: $(sha256sum "$OUT" | awk '{print $1}')"
  echo "- size: $(stat -c%s "$OUT" 2>/dev/null || stat -f%z "$OUT" 2>/dev/null || echo unknown) bytes"
} >> "$ROOT/packaging/deb/BUILD_LOG.md" 2>/dev/null || true
