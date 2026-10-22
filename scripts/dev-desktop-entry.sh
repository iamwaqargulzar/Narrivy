#!/usr/bin/env bash
# Install (or with --remove, uninstall) a per-user desktop entry and icons for
# the development desktop build, so the dock and launcher show Narrivy's icon.
# The window's Wayland app ID is the binary name, `narrivy-desktop`, which the
# entry's StartupWMClass matches. A packaged install ships its own entry.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
entry="$data/applications/narrivy-dev.desktop"
icons="$repo/apps/narrivy-desktop/src-tauri/icons"
sizes=(32 128)

if [[ "${1:-}" == "--remove" ]]; then
    rm -f "$entry"
    for size in "${sizes[@]}" 512; do
        rm -f "$data/icons/hicolor/${size}x${size}/apps/narrivy.png"
    done
    exit 0
fi

for size in "${sizes[@]}"; do
    install -D -m 0644 "$icons/${size}x${size}.png" \
        "$data/icons/hicolor/${size}x${size}/apps/narrivy.png"
done
install -D -m 0644 "$icons/icon.png" "$data/icons/hicolor/512x512/apps/narrivy.png"

install -d "$data/applications"
cat > "$entry" <<EOF
[Desktop Entry]
Name=Narrivy (dev)
Comment=Local-first voice dictation (development build)
Exec=$repo/target/debug/narrivy-desktop
Icon=narrivy
Terminal=false
Type=Application
Categories=Utility;AudioVideo;
StartupWMClass=narrivy-desktop
EOF
chmod 0644 "$entry"
update-desktop-database "$data/applications" 2>/dev/null || true
echo "Installed $entry"
