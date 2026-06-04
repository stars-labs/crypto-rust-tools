#!/usr/bin/env bash
# Demonstrates the *backup-able* workflow: generate an Ed25519 key OFF-card,
# derive its Solana address offline, then import the SAME key into two PIV slots
# and confirm YubiWallet reports the identical address from both — proving the
# backup file alone determines the account, so it can be restored to any YubiKey.
#
#   YK_PIN=123456 ./run-backup.sh
#
# NOTE: this OVERWRITES PIV slots 94 and 95. Use a test key.
set -uo pipefail
PIN=${YK_PIN:-123456}
MGMT=${PIV_MGMT:-010203040506070801020304050607080102030405060708}

cd "$(dirname "$0")/../.."
cargo build -q -p yubiwallet
BIN=target/debug/yubiwallet
gpgconf --kill scdaemon gpg-agent 2>/dev/null || true

# 1) Generate an Ed25519 key OFF-card. THIS file is your backup — keep it
#    encrypted (e.g. `age`/`gpg`) in 2+ locations.
openssl genpkey -algorithm ed25519 -out /tmp/yk_backup_key.pem 2>/dev/null
openssl pkey -in /tmp/yk_backup_key.pem -pubout -out /tmp/yk_backup_pub.pem 2>/dev/null

# 2) Derive the Solana address offline, straight from the backup file.
OFFLINE=$(openssl pkey -in /tmp/yk_backup_key.pem -pubout -outform DER 2>/dev/null | tail -c 32 | python3 -c '
import sys
b = sys.stdin.buffer.read()
alpha = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
n = int.from_bytes(b, "big"); s = ""
while n: n, r = divmod(n, 58); s = alpha[r] + s
print("1" * (len(b) - len(b.lstrip(b"\x00"))) + s)')
echo "offline address (from backup file): $OFFLINE"

# 3) Import the SAME key into two PIV slots + self-signed certs (so the public
#    key is readable). Restoring to a fresh YubiKey is the exact same import.
ok=1
for s in 94 95; do
  ykman piv keys import -m "$MGMT" "$s" /tmp/yk_backup_key.pem >/dev/null 2>&1
  ykman piv certificates generate -m "$MGMT" -P "$PIN" -s "CN=backup-$s" "$s" /tmp/yk_backup_pub.pem >/dev/null 2>&1
  a=$("$BIN" address --applet piv --slot "$s" --curve ed25519 2>/dev/null | grep -oE 'Solana: .*' | awk '{print $2}')
  echo "slot $s on-card address:           $a"
  [ "$a" = "$OFFLINE" ] || ok=0
done

rm -f /tmp/yk_backup_key.pem /tmp/yk_backup_pub.pem
if [ "$ok" = 1 ]; then
  echo "=== BACKUP OK: offline file == slot 94 == slot 95 (restorable to any YubiKey) ==="
else
  echo "=== MISMATCH — backup not reproducible ==="; exit 1
fi
