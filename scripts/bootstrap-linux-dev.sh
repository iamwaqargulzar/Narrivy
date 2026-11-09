#!/usr/bin/env bash
set -euo pipefail
packages=(
  build-essential curl wget file pkg-config cmake clang
  libwebkit2gtk-4.1-dev libxdo-dev libssl-dev
  libayatana-appindicator3-dev librsvg2-dev
  libpipewire-0.3-dev libdbus-1-dev
)
if [[ "${1:-}" != "--install" ]]; then
  printf 'Narrivy Linux dev bootstrap (Debian/Ubuntu family)\n\nWould install:\n'
  printf '  %s\n' "${packages[@]}"
  printf '\nRun this script with --install to execute apt-get. Review for your distro first.\n'
  exit 0
fi
if ! command -v apt-get >/dev/null 2>&1; then
  echo "apt-get not found; use the equivalent packages for this distro." >&2
  exit 2
fi
sudo apt-get update
sudo apt-get install -y "${packages[@]}"
