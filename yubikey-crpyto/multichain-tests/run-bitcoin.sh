#!/usr/bin/env bash
# Bitcoin: sign + broadcast a P2WPKH spend from each OpenPGP secp256k1 slot on a
# local regtest node. Requires: bitcoind regtest running with --datadir=$BTC_DATADIR
# (default /tmp/btcreg), and the OpenPGP SIG/AUT slots holding secp256k1 keys.
#
#   YK_PIN=123456 BTC_DATADIR=/tmp/btcreg ./run-bitcoin.sh
set -uo pipefail
PIN=${YK_PIN:-123456}
DD=${BTC_DATADIR:-/tmp/btcreg}
SLOTS="sig aut"

cd "$(dirname "$0")/../.."
cargo build -q -p yubikey-crypto --example btc_sign
BIN=target/debug/examples/btc_sign
btc() { bitcoin-cli -datadir="$DD" "$@"; }

# wallet with spendable coins
btc createwallet test >/dev/null 2>&1 || btc loadwallet test >/dev/null 2>&1 || true
btc generatetoaddress 101 "$(btc getnewaddress)" >/dev/null 2>&1
gpgconf --kill scdaemon gpg-agent 2>/dev/null || true

ok=0
for slot in $SLOTS; do
  addr=$("$BIN" "$slot" 2>/dev/null | grep -oE 'bcrt1[a-z0-9]+')
  ftxid=$(btc sendtoaddress "$addr" 1.0)
  btc generatetoaddress 1 "$(btc getnewaddress)" >/dev/null 2>&1
  read -r pvout psat < <(btc getrawtransaction "$ftxid" true | python3 -c "
import json,sys
for o in json.load(sys.stdin)['vout']:
    if o['scriptPubKey'].get('address')=='$addr': print(o['n'], int(round(o['value']*1e8))); break")
  daddr=$(btc getnewaddress "" bech32)
  dspk=$(btc getaddressinfo "$daddr" | python3 -c 'import json,sys;print(json.load(sys.stdin)["scriptPubKey"])')
  raw=$(printf '%s\n' "$PIN" | "$BIN" "$slot" sign "$ftxid" "$pvout" "$psat" "$dspk" "$((psat-1000))" 2>/dev/null | awk '{print $NF}')
  txid=$(btc sendrawtransaction "$raw" 2>&1)
  if [[ "$txid" =~ ^[0-9a-f]{64}$ ]]; then
    btc generatetoaddress 1 "$(btc getnewaddress)" >/dev/null 2>&1
    echo "slot $slot $addr BROADCAST_OK txid=$txid"
    ok=$((ok+1))
  else
    echo "slot $slot $addr ERR: $txid"
  fi
done
echo "=== Bitcoin: $ok/2 broadcast OK ==="
