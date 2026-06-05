#!/usr/bin/env bash
# Register the yubiwallet-host native-messaging manifest for local browsers.
#
#   ./install.sh <chrome-extension-id> [path-to-yubiwallet-host]
#
# If the host path is omitted, the release binary is built and used. Writes a
# manifest pointing at the host + your extension id into each installed browser's
# NativeMessagingHosts directory (Chrome/Chromium/Brave/Edge + Firefox).
set -euo pipefail
NAME="com.yubiwallet.host"
EXT_ID="${1:?usage: install.sh <chrome-extension-id> [host-path]}"

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
HOST="${2:-}"
if [[ -z "$HOST" ]]; then
  echo "Building yubiwallet-host (release)…"
  (cd "$REPO_ROOT" && cargo build --release -p yubiwallet-host)
  HOST="$REPO_ROOT/target/release/yubiwallet-host"
fi
HOST="$(cd "$(dirname "$HOST")" && pwd)/$(basename "$HOST")"
[[ -x "$HOST" ]] || { echo "host not found/executable: $HOST" >&2; exit 1; }

chrome_manifest() {
  cat <<EOF
{
  "name": "$NAME",
  "description": "YubiWallet native messaging host",
  "path": "$HOST",
  "type": "stdio",
  "allowed_origins": ["chrome-extension://$EXT_ID/"]
}
EOF
}
firefox_manifest() {
  cat <<EOF
{
  "name": "$NAME",
  "description": "YubiWallet native messaging host",
  "path": "$HOST",
  "type": "stdio",
  "allowed_extensions": ["$EXT_ID"]
}
EOF
}

case "$(uname -s)" in
  Darwin)
    APP="$HOME/Library/Application Support"
    chrome_dirs=("$APP/Google/Chrome/NativeMessagingHosts" "$APP/Chromium/NativeMessagingHosts" "$APP/BraveSoftware/Brave-Browser/NativeMessagingHosts" "$APP/Microsoft Edge/NativeMessagingHosts")
    firefox_dirs=("$APP/Mozilla/NativeMessagingHosts")
    ;;
  *)
    cfg="$HOME/.config"
    chrome_dirs=("$cfg/google-chrome/NativeMessagingHosts" "$cfg/chromium/NativeMessagingHosts" "$cfg/BraveSoftware/Brave-Browser/NativeMessagingHosts" "$cfg/microsoft-edge/NativeMessagingHosts")
    firefox_dirs=("$HOME/.mozilla/native-messaging-hosts")
    ;;
esac

for d in "${chrome_dirs[@]}"; do
  [[ -d "$(dirname "$d")" ]] || continue
  mkdir -p "$d"; chrome_manifest > "$d/$NAME.json"; echo "installed: $d/$NAME.json"
done
for d in "${firefox_dirs[@]}"; do
  [[ -d "$(dirname "$d")" ]] || continue
  mkdir -p "$d"; firefox_manifest > "$d/$NAME.json"; echo "installed: $d/$NAME.json"
done
echo "Done. Host: $HOST  ·  Extension: $EXT_ID"
