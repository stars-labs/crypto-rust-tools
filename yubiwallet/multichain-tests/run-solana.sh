#!/usr/bin/env bash
# Solana: sign + broadcast from every PIV Ed25519 slot against a local node
# (surfpool by default). Requires: surfpool running on :8899, PIV slots
# provisioned with Ed25519 keys, and the YubiKey inserted.
#
#   YK_PIN=123456 ./run-solana.sh
set -uo pipefail
PIN=${YK_PIN:-123456}
SLOTS="9a 9c 9d 9e 82 83 84 85 86 87 88 89 8a 8b 8c 8d 8e 8f 90 91 92 93 94 95"

cd "$(dirname "$0")/../.."                      # workspace root (crypto-rust-tools)
cargo build -q -p yubiwallet --example sol_surfpool
BIN=target/debug/examples/sol_surfpool
gpgconf --kill scdaemon gpg-agent 2>/dev/null || true   # release card from scdaemon

ok=0
for s in $SLOTS; do
  line=$(printf '%s\n' "$PIN" | "$BIN" "$s" 2>&1 | grep -oE "slot $s .*")
  echo "$line"
  [[ "$line" == *BROADCAST_OK* ]] && ok=$((ok+1))
done
echo "=== Solana: $ok/$(echo $SLOTS | wc -w) broadcast OK ==="
