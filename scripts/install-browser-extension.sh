#!/usr/bin/env bash
# Install Narrivy's browser connector (the native-messaging host) for Brave,
# Chrome and Chromium, for development (D26, SQ-WRITE).
#
# The host binary is copied to ~/.local/lib/narrivy and registered for the
# extension ID fixed by the public key in the extension's manifest. The
# extension itself is loaded unpacked from apps/narrivy-browser-extension.
#
# Usage: scripts/install-browser-extension.sh [--uninstall]
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
name="com.narrivy.writing"
extension="$repo/apps/narrivy-browser-extension"
install_dir="$HOME/.local/lib/narrivy"
host="$install_dir/narrivy-browser-host"
browsers=(
    "$HOME/.config/BraveSoftware/Brave-Browser"
    "$HOME/.config/google-chrome"
    "$HOME/.config/chromium"
)

if [[ "${1:-}" == "--uninstall" ]]; then
    for dir in "${browsers[@]}"; do
        rm -f "$dir/NativeMessagingHosts/$name.json"
    done
    rm -f "$host"
    echo "Removed the Narrivy browser connector."
    exit 0
fi

# The extension ID Chromium derives from the manifest's public key.
extension_id="$(python3 - "$extension/manifest.json" <<'PY'
import base64, hashlib, json, sys
key = json.load(open(sys.argv[1]))["key"]
digest = hashlib.sha256(base64.b64decode(key)).hexdigest()[:32]
print("".join(chr(ord("a") + int(c, 16)) for c in digest))
PY
)"

cargo build --locked --release --manifest-path "$repo/Cargo.toml" -p narrivy-browser-host
install -d -m 0755 "$install_dir"
install -m 0755 "$repo/target/release/narrivy-browser-host" "$host"

registered=0
for dir in "${browsers[@]}"; do
    [[ -d "$dir" ]] || continue
    install -d -m 0755 "$dir/NativeMessagingHosts"
    cat > "$dir/NativeMessagingHosts/$name.json" <<JSON
{
  "name": "$name",
  "description": "Narrivy writing assistant",
  "path": "$host",
  "type": "stdio",
  "allowed_origins": ["chrome-extension://$extension_id/"]
}
JSON
    echo "Registered for ${dir#"$HOME/.config/"}"
    registered=1
done
if [[ $registered == 0 ]]; then
    echo "No Brave, Chrome or Chromium profile found; nothing registered." >&2
    exit 1
fi

cat <<TXT

Connector installed for extension $extension_id.
Load the extension once (the ID stays the same on every load):
  1. Open brave://extensions (or chrome://extensions)
  2. Turn on "Developer mode" (top right)
  3. Click "Load unpacked" and choose:
     $extension
TXT
