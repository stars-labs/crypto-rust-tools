#!/usr/bin/env bash
# Sui: sign + execute from every PIV Ed25519 slot against a local Sui network.
# Requires: `sui start --with-faucet --force-regenesis` running, `sui client`
# env pointing at localnet (http://127.0.0.1:9000), PIV slots provisioned.
#
#   YK_PIN=123456 ./run-sui.sh
set -uo pipefail
PIN=${YK_PIN:-123456}
SLOTS="9a 9c 9d 9e 82 83 84 85 86 87 88 89 8a 8b 8c 8d 8e 8f 90 91 92 93 94 95"

cd "$(dirname "$0")/../.."
cargo build -q -p yubikey-crypto --example sui_sign
BIN=target/debug/examples/sui_sign
RECIP=$(sui client active-address)
gpgconf --kill scdaemon gpg-agent 2>/dev/null || true

# pass 1: derive each address and request faucet funding
declare -A ADDR
for s in $SLOTS; do
  a=$("$BIN" "$s")
  ADDR[$s]=$a
  sui client faucet --address "$a" >/dev/null 2>&1
  echo "funded $s -> $a"
done
echo "waiting for funding to settle..."; sleep 30

# pass 2: build unsigned tx, sign on card, execute
ok=0
for s in $SLOTS; do
  a=${ADDR[$s]}
  coin=$(sui client gas "$a" --json 2>/dev/null | grep gasCoinId | head -1 | grep -oE '0x[0-9a-f]+')
  [[ -z "$coin" ]] && { echo "slot $s ${a:0:12}… NO COIN"; continue; }
  txb=$(sui client transfer-sui --to "$RECIP" --amount 1000000 --sui-coin-object-id "$coin" \
        --gas-budget 5000000 --serialize-unsigned-transaction 2>/dev/null | tail -1)
  sig=$(printf '%s\n' "$PIN" | "$BIN" "$s" "$txb" 2>/dev/null | awk '{print $NF}')
  res=$(sui client execute-signed-tx --tx-bytes "$txb" --signatures "$sig" 2>&1)
  if echo "$res" | grep -q "Status: Success"; then
    echo "slot $s ${a:0:12}… $(echo "$res" | grep -oE 'Transaction Digest: [A-Za-z0-9]+' | head -1) Success"
    ok=$((ok+1))
  else
    echo "slot $s ${a:0:12}… FAILED: $(echo "$res" | grep -iE 'error' | head -1)"
  fi
done
echo "=== Sui: $ok/$(echo $SLOTS | wc -w) execute Success ==="
