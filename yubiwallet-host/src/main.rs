//! YubiWallet native-messaging host.
//!
//! A browser extension speaks to this binary over Chrome/Firefox Native
//! Messaging: each message is a little-endian u32 length prefix followed by a
//! UTF-8 JSON object. The host only performs *key operations* on the YubiKey
//! (via the `yubiwallet` library); all chain logic lives in the extension.
//!
//! Methods:
//!   list_accounts  {}                              -> { accounts: [...] }
//!   sign_secp256k1 { account_id, prehash:<32B hex> } -> { r, s, recovery_id }
//!   sign_ed25519   { account_id, message:<hex> }   -> { signature:<64B hex> }
//!   get_status     {}                              -> { card_present, accounts }
//!
//! PIN entry (M0): from $YUBIWALLET_PIN if set (testing), else prompted on
//! /dev/tty. A GUI pinentry for the browser-launched daemon is a later milestone.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{self, BufRead, Read, Write};
use yubiwallet::{
    Account, Applet, Curve, eth, get_pubkey, list_piv_accounts, openpgp_account, openpgp_slot,
    parse_slot, sign_with_pin,
};

#[derive(Deserialize)]
struct Req {
    id: u64,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Serialize)]
struct Resp {
    id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ErrObj>,
}

#[derive(Serialize)]
struct ErrObj {
    code: String,
    message: String,
}

type MethodResult = Result<Value, (String, String)>;

fn main() {
    let mut stdin = io::stdin().lock();
    while let Some(msg) = read_message(&mut stdin) {
        let resp = handle(&msg);
        write_message(&resp);
    }
}

/// Read one Native Messaging frame: u32-le length + that many bytes.
fn read_message(r: &mut impl Read) -> Option<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).ok()?;
    let n = u32::from_le_bytes(len) as usize;
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf).ok()?;
    Some(buf)
}

fn write_message(bytes: &[u8]) {
    let mut out = io::stdout().lock();
    let _ = out.write_all(&(bytes.len() as u32).to_le_bytes());
    let _ = out.write_all(bytes);
    let _ = out.flush();
}

fn handle(msg: &[u8]) -> Vec<u8> {
    let req: Req = match serde_json::from_slice(msg) {
        Ok(r) => r,
        Err(e) => return serialize(0, Err(("BAD_REQUEST".into(), e.to_string()))),
    };
    let result = dispatch(&req.method, &req.params);
    serialize(req.id, result)
}

fn serialize(id: u64, result: MethodResult) -> Vec<u8> {
    let resp = match result {
        Ok(v) => Resp {
            id,
            result: Some(v),
            error: None,
        },
        Err((code, message)) => Resp {
            id,
            result: None,
            error: Some(ErrObj { code, message }),
        },
    };
    serde_json::to_vec(&resp).unwrap_or_default()
}

fn dispatch(method: &str, params: &Value) -> MethodResult {
    match method {
        "list_accounts" => Ok(json!({ "accounts": list_accounts() })),
        "get_status" => {
            let accounts = list_accounts();
            Ok(json!({ "card_present": !accounts.is_empty(), "accounts": accounts }))
        }
        "sign_secp256k1" => sign_secp256k1(params),
        "sign_ed25519" => sign_ed25519(params),
        other => Err((
            "UNSUPPORTED_METHOD".into(),
            format!("unknown method: {other}"),
        )),
    }
}

// ---------------------------------------------------------------------------

fn list_accounts() -> Vec<Value> {
    let mut accounts = Vec::new();
    for (slot_name, slot) in [("sig", openpgp_slot::SIG), ("aut", openpgp_slot::AUT)] {
        if let Ok((curve, pk)) = openpgp_account(slot) {
            accounts.push(account_json("openpgp", slot_name, curve, &pk));
        }
    }
    if let Ok(slots) = list_piv_accounts() {
        for info in slots {
            if let Some(pk) = info.ed25519_pubkey {
                accounts.push(account_json(
                    "piv",
                    &format!("{:02x}", info.slot),
                    Curve::Ed25519,
                    &pk,
                ));
            }
        }
    }
    accounts
}

fn account_json(applet: &str, slot: &str, curve: Curve, pk: &[u8]) -> Value {
    let (family, address, curve_str) = match curve {
        Curve::Ed25519 => ("solana", bs58::encode(pk).into_string(), "ed25519"),
        Curve::Secp256k1 => {
            let addr = eth::address_from_pubkey(pk)
                .map(|a| eth::address_to_hex(&a))
                .unwrap_or_default();
            ("evm", addr, "secp256k1")
        }
    };
    json!({
        "id": format!("{applet}:{slot}:{curve_str}"),
        "family": family,
        "curve": curve_str,
        "applet": applet,
        "slot": slot,
        "address": address,
        "pubkey": hex::encode(pk),
    })
}

fn parse_account(id: &str) -> Result<Account, (String, String)> {
    let p: Vec<&str> = id.split(':').collect();
    let bad = |m: String| ("UNKNOWN_ACCOUNT".to_string(), m);
    if p.len() != 3 {
        return Err(bad(format!(
            "bad account_id '{id}' (want applet:slot:curve)"
        )));
    }
    let applet: Applet = p[0].parse().map_err(|e| bad(format!("{e}")))?;
    let slot = parse_slot(applet, p[1]).map_err(|e| bad(format!("{e}")))?;
    let curve: Curve = p[2].parse().map_err(|e| bad(format!("{e}")))?;
    Ok(Account {
        applet,
        slot,
        curve,
    })
}

fn param_str(params: &Value, key: &str) -> Result<String, (String, String)> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| ("BAD_REQUEST".to_string(), format!("missing param '{key}'")))
}

fn sign_secp256k1(params: &Value) -> MethodResult {
    let acc = parse_account(&param_str(params, "account_id")?)?;
    let prehash = hex::decode(param_str(params, "prehash")?.trim_start_matches("0x"))
        .map_err(|e| ("BAD_REQUEST".to_string(), format!("prehash hex: {e}")))?;
    let hash: [u8; 32] = prehash
        .as_slice()
        .try_into()
        .map_err(|_| ("BAD_REQUEST".to_string(), "prehash must be 32 bytes".into()))?;

    let pubkey = get_pubkey(&acc).map_err(card_err)?;
    let pin = read_pin().map_err(|m| ("PIN_FAILED".to_string(), m))?;
    let rs_vec = sign_with_pin(&acc, &hash, &pin).map_err(card_err)?;
    let rs: [u8; 64] = rs_vec
        .as_slice()
        .try_into()
        .map_err(|_| ("CARD_ERROR".to_string(), "expected 64-byte R||S".into()))?;

    // chain_id 0 → v = recovery_id + 35, so recovery_id = v - 35.
    let sig = eth::ethereum_signature(&pubkey, &hash, &rs, 0).map_err(card_err)?;
    Ok(json!({
        "r": hex::encode(sig.r),
        "s": hex::encode(sig.s),
        "recovery_id": sig.v.saturating_sub(35),
    }))
}

fn sign_ed25519(params: &Value) -> MethodResult {
    let acc = parse_account(&param_str(params, "account_id")?)?;
    let message = hex::decode(param_str(params, "message")?.trim_start_matches("0x"))
        .map_err(|e| ("BAD_REQUEST".to_string(), format!("message hex: {e}")))?;
    let pin = read_pin().map_err(|m| ("PIN_FAILED".to_string(), m))?;
    let sig = sign_with_pin(&acc, &message, &pin).map_err(card_err)?;
    Ok(json!({ "signature": hex::encode(sig) }))
}

fn card_err(e: yubiwallet::Error) -> (String, String) {
    ("CARD_ERROR".to_string(), e.to_string())
}

/// M0 PIN entry: `$YUBIWALLET_PIN` (testing) or a prompt on the controlling tty.
/// A GUI pinentry for the browser-launched daemon is a later milestone.
fn read_pin() -> Result<String, String> {
    if let Ok(p) = std::env::var("YUBIWALLET_PIN") {
        if !p.is_empty() {
            return Ok(p);
        }
    }
    if let Ok(mut w) = std::fs::OpenOptions::new().write(true).open("/dev/tty") {
        let _ = write!(w, "YubiWallet PIN: ");
        let _ = w.flush();
    }
    let tty = std::fs::File::open("/dev/tty")
        .map_err(|e| format!("no PIN source (set YUBIWALLET_PIN or run with a tty): {e}"))?;
    let mut line = String::new();
    io::BufReader::new(tty)
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    Ok(line.trim().to_string())
}
