//! OpenPGP card applet: public-key retrieval and signing.

use crate::account::{Account, Curve, openpgp_slot};
use crate::apdu::{
    self, CLA, INS_GET_PUBLIC_KEY, INS_INTERNAL_AUTHENTICATE, INS_PSO, INS_SELECT, INS_VERIFY,
};
use crate::error::Error;
use crate::pin;
use pcsc::Card;

/// OpenPGP card application identifier (RID + application).
const AID: &[u8] = &[0xD2, 0x76, 0x00, 0x01, 0x24, 0x01];

// VERIFY PW1 modes: 0x81 authorizes PSO:CDS (SIG); 0x82 authorizes other
// commands such as INTERNAL AUTHENTICATE (AUT).
const P2_PW1_SIGN: u8 = 0x81;
const P2_PW1_OTHER: u8 = 0x82;
const P1_SIGN: u8 = 0x9E;
const P2_COMPUTE_SIGNATURE: u8 = 0x9A;

/// Control Reference Template tag for a slot's key (used by GET PUBLIC KEY).
fn crt_tag(slot: u8) -> Result<u8, Error> {
    match slot {
        openpgp_slot::SIG => Ok(0xB6),
        openpgp_slot::AUT => Ok(0xA4),
        _ => Err(Error::Unsupported(format!(
            "OpenPGP slot {slot:#04x}: only SIG and AUT are supported"
        ))),
    }
}

fn select(card: &Card) -> Result<(), Error> {
    let mut buf = [0u8; 512];
    apdu::send(card, CLA, INS_SELECT, 0x04, 0x00, Some(AID), None, &mut buf)?;
    Ok(())
}

/// Read the GET PUBLIC KEY response (raw TLV) for the given slot.
fn get_pubkey_raw_on(card: &Card, slot: u8) -> Result<Vec<u8>, Error> {
    let crt = crt_tag(slot)?;
    // GET PUBLIC KEY (P1=0x81 read), extended-length APDU, CRT = `<tag> 00`.
    let cmd = [
        CLA,
        INS_GET_PUBLIC_KEY,
        0x81,
        0x00,
        0x00, // extended Lc marker
        0x00,
        0x02, // Lc = 2
        crt,  // CRT tag (B6 = SIG, A4 = AUT)
        0x00, // CRT length 0
        0x00,
        0x00, // extended Le
    ];
    let mut pk_buf = [0u8; 512];
    let pubkey_data = apdu::transmit(card, &cmd, &mut pk_buf)?;
    Ok(pubkey_data.to_vec())
}

fn get_pubkey_raw(slot: u8) -> Result<Vec<u8>, Error> {
    let (_ctx, card) = apdu::connect()?;
    select(&card)?;
    get_pubkey_raw_on(&card, slot)
}

/// Fetch the public key from an OpenPGP slot (SIG or AUT).
pub fn get_pubkey(acc: &Account) -> Result<Vec<u8>, Error> {
    let raw = get_pubkey_raw(acc.slot)?;
    match acc.curve {
        Curve::Ed25519 => extract_ed25519_pubkey(&raw).map(|k| k.to_vec()),
        Curve::Secp256k1 => extract_secp256k1_pubkey(&raw).map(|k| k.to_vec()),
    }
}

/// Read an OpenPGP slot and auto-detect its curve from the response
/// (Ed25519 → Solana, secp256k1 → Ethereum). Returns `(curve, public_key)`.
pub fn detect_account(slot: u8) -> Result<(Curve, Vec<u8>), Error> {
    let raw = get_pubkey_raw(slot)?;
    if let Ok(k) = extract_ed25519_pubkey(&raw) {
        return Ok((Curve::Ed25519, k.to_vec()));
    }
    if let Ok(k) = extract_secp256k1_pubkey(&raw) {
        return Ok((Curve::Secp256k1, k.to_vec()));
    }
    Err(Error::PubkeyNotFound(format!(
        "OpenPGP slot {slot:#04x}: not an Ed25519 or secp256k1 key"
    )))
}

/// Sign `message` with an OpenPGP slot. `pin` is used if `Some`, else prompted.
///
/// - SIG: PSO:COMPUTE DIGITAL SIGNATURE (PW1 mode 0x81).
/// - AUT: INTERNAL AUTHENTICATE (PW1 mode 0x82).
///
/// Returns the raw card signature (Ed25519: 64 bytes; secp256k1: `R || S`).
pub fn sign(acc: &Account, message: &[u8], pin: Option<&str>) -> Result<Vec<u8>, Error> {
    let (_ctx, card) = apdu::connect()?;
    select(&card)?;

    let mut buf = [0u8; 512];
    let mut sig_buf = [0u8; 512];
    let signature = match acc.slot {
        openpgp_slot::SIG => {
            let user_pin = pin::resolve(pin, "Enter User PIN (PW1/PIN2): ")?;
            apdu::send(
                &card,
                CLA,
                INS_VERIFY,
                0x00,
                P2_PW1_SIGN,
                Some(user_pin.as_bytes()),
                None,
                &mut buf,
            )?;
            apdu::send(
                &card,
                CLA,
                INS_PSO,
                P1_SIGN,
                P2_COMPUTE_SIGNATURE,
                Some(message),
                Some(0x00),
                &mut sig_buf,
            )?
        }
        openpgp_slot::AUT => {
            let user_pin = pin::resolve(pin, "Enter User PIN (PW1): ")?;
            apdu::send(
                &card,
                CLA,
                INS_VERIFY,
                0x00,
                P2_PW1_OTHER,
                Some(user_pin.as_bytes()),
                None,
                &mut buf,
            )?;
            apdu::send(
                &card,
                CLA,
                INS_INTERNAL_AUTHENTICATE,
                0x00,
                0x00,
                Some(message),
                Some(0x00),
                &mut sig_buf,
            )?
        }
        _ => {
            return Err(Error::Unsupported(format!(
                "OpenPGP slot {:#04x}: only SIG and AUT are supported",
                acc.slot
            )));
        }
    };
    Ok(signature.to_vec())
}

/// Extract a 32-byte Ed25519 public key from an OpenPGP GET PUBLIC KEY response.
///
/// The key sits in a `0x86 0x20 <32 bytes>` TLV inside the response template.
pub fn extract_ed25519_pubkey(tlv: &[u8]) -> Result<[u8; 32], Error> {
    if tlv.len() < 2 {
        return Err(Error::PubkeyNotFound(
            "GET PUBLIC KEY response too short".into(),
        ));
    }
    match tlv.windows(2).position(|w| w == [0x86, 0x20]) {
        Some(pos) => {
            let start = pos + 2;
            let end = start + 32;
            if end > tlv.len() {
                return Err(Error::PubkeyNotFound(
                    "truncated 0x86 public-key TLV".into(),
                ));
            }
            Ok(<[u8; 32]>::try_from(&tlv[start..end]).expect("slice is exactly 32 bytes"))
        }
        None => Err(Error::PubkeyNotFound(format!(
            "no Ed25519 (0x86,0x20) tag in response (TLV: {})",
            hex::encode(tlv)
        ))),
    }
}

/// Extract a 65-byte uncompressed secp256k1 public key (`0x04 || X || Y`) from
/// an OpenPGP GET PUBLIC KEY response.
///
/// The point sits in a `0x86 0x41 0x04 <64 bytes>` TLV.
pub fn extract_secp256k1_pubkey(tlv: &[u8]) -> Result<[u8; 65], Error> {
    if tlv.len() < 2 {
        return Err(Error::PubkeyNotFound(
            "GET PUBLIC KEY response too short".into(),
        ));
    }
    match tlv.windows(2).position(|w| w == [0x86, 0x41]) {
        Some(pos) => {
            let start = pos + 2;
            let end = start + 65;
            if end > tlv.len() {
                return Err(Error::PubkeyNotFound(
                    "truncated 0x86 secp256k1 point TLV".into(),
                ));
            }
            let point = <[u8; 65]>::try_from(&tlv[start..end]).expect("slice is exactly 65 bytes");
            if point[0] != 0x04 {
                return Err(Error::PubkeyNotFound(format!(
                    "expected uncompressed point (0x04 prefix), got {:#04x}",
                    point[0]
                )));
            }
            Ok(point)
        }
        None => Err(Error::PubkeyNotFound(format!(
            "no secp256k1 (0x86,0x41) tag in response (TLV: {})",
            hex::encode(tlv)
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_key_from_well_formed_tlv() {
        // 7F49 22 86 20 <32 bytes> wrapper, as returned by the card.
        let key: [u8; 32] = core::array::from_fn(|i| i as u8);
        let mut tlv = vec![0x7F, 0x49, 0x22, 0x86, 0x20];
        tlv.extend_from_slice(&key);
        assert_eq!(extract_ed25519_pubkey(&tlv).unwrap(), key);
    }

    #[test]
    fn missing_tag_is_an_error() {
        let tlv = [0x7F, 0x49, 0x02, 0x99, 0x00];
        assert!(matches!(
            extract_ed25519_pubkey(&tlv),
            Err(Error::PubkeyNotFound(_))
        ));
    }

    #[test]
    fn truncated_key_is_an_error() {
        // Tag present but fewer than 32 trailing bytes.
        let tlv = [0x86, 0x20, 0x01, 0x02, 0x03];
        assert!(matches!(
            extract_ed25519_pubkey(&tlv),
            Err(Error::PubkeyNotFound(_))
        ));
    }

    #[test]
    fn extracts_secp256k1_uncompressed_point() {
        let mut point = [0u8; 65];
        point[0] = 0x04;
        for (i, b) in point.iter_mut().enumerate().skip(1) {
            *b = i as u8;
        }
        let mut tlv = vec![0x7F, 0x49, 0x43, 0x86, 0x41];
        tlv.extend_from_slice(&point);
        assert_eq!(extract_secp256k1_pubkey(&tlv).unwrap(), point);
    }

    #[test]
    fn secp256k1_rejects_non_uncompressed_prefix() {
        let mut tlv = vec![0x86, 0x41, 0x02]; // 0x02 = compressed prefix
        tlv.extend_from_slice(&[0u8; 64]);
        assert!(matches!(
            extract_secp256k1_pubkey(&tlv),
            Err(Error::PubkeyNotFound(_))
        ));
    }
}
