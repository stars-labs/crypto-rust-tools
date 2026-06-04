//! Account selection: which applet, which key slot, which curve.

use crate::error::Error;
use std::str::FromStr;

/// The smart-card application that holds the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applet {
    /// OpenPGP card applet. Supports Ed25519 and secp256k1, but only ~2-3
    /// usable signing slots (SIG / AUT).
    OpenPgp,
    /// PIV applet. Supports Ed25519 (firmware 5.7+) but NOT secp256k1.
    /// Provides ~24 slots (9A/9C/9D/9E + retired 82..95).
    Piv,
}

/// Signature curve for an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Curve {
    /// Ed25519 — Solana, Aptos, Sui, etc.
    Ed25519,
    /// secp256k1 — Ethereum, Bitcoin, Cosmos, Tron, etc. (OpenPGP only).
    Secp256k1,
}

/// A selectable signing account = applet + slot + curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Account {
    pub applet: Applet,
    /// Slot/key reference. See [`openpgp_slot`] and [`piv_slot`].
    pub slot: u8,
    pub curve: Curve,
}

impl Account {
    /// The default account: OpenPGP signature slot, Ed25519. Matches the
    /// behavior of the original single-key API.
    pub fn openpgp_sig() -> Self {
        Account {
            applet: Applet::OpenPgp,
            slot: openpgp_slot::SIG,
            curve: Curve::Ed25519,
        }
    }
}

impl FromStr for Applet {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Error> {
        match s.trim().to_ascii_lowercase().as_str() {
            "openpgp" | "pgp" | "gpg" => Ok(Applet::OpenPgp),
            "piv" => Ok(Applet::Piv),
            _ => Err(Error::Parse(format!(
                "unknown applet '{s}' (use openpgp|piv)"
            ))),
        }
    }
}

impl FromStr for Curve {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Error> {
        match s.trim().to_ascii_lowercase().as_str() {
            "ed25519" | "ed" => Ok(Curve::Ed25519),
            "secp256k1" | "k1" => Ok(Curve::Secp256k1),
            _ => Err(Error::Parse(format!(
                "unknown curve '{s}' (use ed25519|secp256k1)"
            ))),
        }
    }
}

/// Parse a slot string for the given applet.
///
/// - OpenPGP: `sig` / `aut` (and `signature` / `auth`).
/// - PIV: `auth` / `sign` / `keymgmt` / `cardauth`, or any hex slot id
///   (`9a`, `0x9a`, `82`, ...).
pub fn parse_slot(applet: Applet, s: &str) -> Result<u8, Error> {
    let t = s.trim().to_ascii_lowercase();
    match applet {
        Applet::OpenPgp => match t.as_str() {
            "sig" | "signature" => Ok(openpgp_slot::SIG),
            "aut" | "auth" | "authentication" => Ok(openpgp_slot::AUT),
            _ => Err(Error::Parse(format!(
                "unknown OpenPGP slot '{s}' (use sig|aut)"
            ))),
        },
        Applet::Piv => {
            let v = match t.as_str() {
                "auth" => piv_slot::AUTH,
                "sign" => piv_slot::SIGN,
                "keymgmt" | "key-mgmt" => piv_slot::KEY_MGMT,
                "cardauth" | "card-auth" => piv_slot::CARD_AUTH,
                other => parse_hex_byte(other)
                    .ok_or_else(|| Error::Parse(format!("unknown PIV slot '{s}'")))?,
            };
            if piv_slot::is_valid(v) {
                Ok(v)
            } else {
                Err(Error::Parse(format!("{s} is not a valid PIV signing slot")))
            }
        }
    }
}

fn parse_hex_byte(s: &str) -> Option<u8> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    u8::from_str_radix(s, 16).ok()
}

/// OpenPGP key references (per OpenPGP card spec).
pub mod openpgp_slot {
    /// Signature key — signs via PSO:COMPUTE DIGITAL SIGNATURE.
    pub const SIG: u8 = 0x01;
    /// Authentication key — signs via INTERNAL AUTHENTICATE.
    pub const AUT: u8 = 0x03;
}

/// PIV slot identifiers (per NIST SP 800-73 / Yubico PIV).
pub mod piv_slot {
    pub const AUTH: u8 = 0x9A;
    pub const SIGN: u8 = 0x9C;
    pub const KEY_MGMT: u8 = 0x9D;
    pub const CARD_AUTH: u8 = 0x9E;

    /// Inclusive range of the 20 "retired" key-management slots (0x82..=0x95).
    pub const RETIRED_FIRST: u8 = 0x82;
    pub const RETIRED_LAST: u8 = 0x95;

    /// True if `slot` is a PIV slot that can hold a signing key.
    pub fn is_valid(slot: u8) -> bool {
        matches!(slot, AUTH | SIGN | KEY_MGMT | CARD_AUTH)
            || (RETIRED_FIRST..=RETIRED_LAST).contains(&slot)
    }

    /// All signing-capable PIV slots, in a stable order (4 standard + 20 retired).
    pub fn all() -> Vec<u8> {
        let mut slots = vec![AUTH, SIGN, KEY_MGMT, CARD_AUTH];
        slots.extend(RETIRED_FIRST..=RETIRED_LAST);
        slots
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_account_is_openpgp_sig_ed25519() {
        let a = Account::openpgp_sig();
        assert_eq!(a.applet, Applet::OpenPgp);
        assert_eq!(a.slot, openpgp_slot::SIG);
        assert_eq!(a.curve, Curve::Ed25519);
    }

    #[test]
    fn piv_slot_validation() {
        assert!(piv_slot::is_valid(0x9A));
        assert!(piv_slot::is_valid(0x82));
        assert!(piv_slot::is_valid(0x95));
        assert!(!piv_slot::is_valid(0x81));
        assert!(!piv_slot::is_valid(0x96));
        assert!(!piv_slot::is_valid(0x00));
    }

    #[test]
    fn piv_has_24_signing_slots() {
        let all = piv_slot::all();
        assert_eq!(all.len(), 24);
        assert!(all.iter().all(|&s| piv_slot::is_valid(s)));
    }

    #[test]
    fn applet_parsing() {
        assert_eq!("openpgp".parse::<Applet>().unwrap(), Applet::OpenPgp);
        assert_eq!("PIV".parse::<Applet>().unwrap(), Applet::Piv);
        assert!("foo".parse::<Applet>().is_err());
    }

    #[test]
    fn curve_parsing() {
        assert_eq!("ed25519".parse::<Curve>().unwrap(), Curve::Ed25519);
        assert_eq!("secp256k1".parse::<Curve>().unwrap(), Curve::Secp256k1);
        assert!("rsa".parse::<Curve>().is_err());
    }

    #[test]
    fn openpgp_slot_aliases() {
        assert_eq!(
            parse_slot(Applet::OpenPgp, "sig").unwrap(),
            openpgp_slot::SIG
        );
        assert_eq!(
            parse_slot(Applet::OpenPgp, "AUT").unwrap(),
            openpgp_slot::AUT
        );
        assert!(parse_slot(Applet::OpenPgp, "9a").is_err());
    }

    #[test]
    fn piv_slot_aliases_and_hex() {
        assert_eq!(parse_slot(Applet::Piv, "auth").unwrap(), 0x9A);
        assert_eq!(parse_slot(Applet::Piv, "9a").unwrap(), 0x9A);
        assert_eq!(parse_slot(Applet::Piv, "0x9c").unwrap(), 0x9C);
        assert_eq!(parse_slot(Applet::Piv, "82").unwrap(), 0x82);
        assert_eq!(parse_slot(Applet::Piv, "95").unwrap(), 0x95);
        assert!(parse_slot(Applet::Piv, "96").is_err());
        assert!(parse_slot(Applet::Piv, "zz").is_err());
    }
}
