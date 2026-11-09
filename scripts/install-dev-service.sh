#!/usr/bin/env bash
# Install narrivy-daemon.service for this user, running this checkout's
# development daemon (scripts/dev-daemon.sh). The packaged build installs a
# unit of the same name; the app's "Start at login" enables whichever is
# installed, and the app starts it when it finds the service not running.
#
#   scripts/install-dev-service.sh            install (does not enable)
#   scripts/install-dev-service.sh --remove   remove it again
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
unit_dir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
unit="$unit_dir/narrivy-daemon.service"

if [[ "${1:-}" == "--remove" ]]; then
    systemctl --user disable --now narrivy-daemon.service 2>/dev/null || true
    rm -f "$unit"
    systemctl --user daemon-reload
    echo "Removed $unit"
    exit 0
fi

mkdir -p "$unit_dir"
cat >"$unit" <<UNIT
# Written by scripts/install-dev-service.sh: the development daemon.
[Unit]
Description=Narrivy dictation daemon (development checkout)
# With the desktop session: typing into apps needs its display.
PartOf=graphical-session.target
After=graphical-session.target pipewire.service
Wants=pipewire.service

[Service]
Type=simple
WorkingDirectory=$repo
# The development daemon builds with cargo first.
Environment=PATH=$HOME/.cargo/bin:$HOME/.local/bin:/usr/local/bin:/usr/bin:/bin
ExecStart=$repo/scripts/dev-daemon.sh
Restart=on-failure
RestartSec=3s
TimeoutStartSec=10min

[Install]
WantedBy=graphical-session.target
UNIT
systemctl --user daemon-reload
echo "Installed $unit"
