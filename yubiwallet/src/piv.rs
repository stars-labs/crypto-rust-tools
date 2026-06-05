//! PIV applet: Ed25519 multi-slot signing and public-key retrieval.
//!
//! PIV gives ~24 signing slots (9A/9C/9D/9E + retired 82..95), enabling many
//! Ed25519 accounts on one YubiKey. Requires firmware 5.7+ for Ed25519.
//! PIV does **not** support secp256k1.
//!
//! Notes on the protocol:
//! - PIV has no "read public key" command; the key is recovered by reading the
//!   slot's X.509 certificate (GET DATA) and pulling the SubjectPublicKeyInfo.
//! - Signing uses GENERAL AUTHENTICATE. For Ed25519 the card signs the raw
//!   message (PureEdDSA), so messages can exceed 255 bytes -> extended APDUs.

use crate::account::{Account, Curve, piv_slot};
use crate::apdu::{self, CLA, INS_GENERAL_AUTHENTICATE, INS_GET_DATA, INS_SELECT, INS_VERIFY};
use crate::error::Error;
use crate::pin;
use pcsc::Card;

/// PIV application identifier.
const AID: &[u8] = &[
    0xA0, 0x00, 0x00, 0x03, 0x08, 0x00, 0x00, 0x10, 0x00, 0x01, 0x00,
];

/// PIV algorithm reference for Ed25519 (firmware 5.7+).
const ALG_ED25519: u8 = 0xE0;
/// VERIFY P2 for the PIV application PIN.
const P2_PIV_PIN: u8 = 0x80;

// BER-TLV tags used by GENERAL AUTHENTICATE and GET DATA.
const TAG_DYNAMIC_AUTH: u8 = 0x7C;
const TAG_RESPONSE: u8 = 0x82;
const TAG_CHALLENGE: u8 = 0x81;
const TAG_DATA_OBJECT: u8 = 0x53;
const TAG_CERTIFICATE: u8 = 0x70;

/// Standard Ed25519 SubjectPublicKeyInfo prefix:
/// `SEQ { SEQ { OID 1.3.101.112 } BIT STRING (0 unused) }` then 32 key bytes.
const ED25519_SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2A, 0x30, 0x05, 0x06, 0x03, 0x2B, 0x65, 0x70, 0x03, 0x21, 0x00,
];

/// Information about one PIV slot discovered by [`list_accounts`].
#[derive(Debug, Clone)]
pub struct SlotInfo {
    pub slot: u8,
    /// `Some(32-byte key)` if the slot has a readable Ed25519 certificate.
    pub ed25519_pubkey: Option<[u8; 32]>,
}

/// Fetch the Ed25519 public key for a PIV account.
pub fn get_pubkey(acc: &Account) -> Result<Vec<u8>, Error> {
    ensure_ed25519(acc)?;
    if !piv_slot::is_valid(acc.slot) {
        return Err(Error::Unsupported(format!(
            "{:#04x} is not a PIV signing slot",
            acc.slot
        )));
    }
    let (_ctx, card) = apdu::connect()?;
    select(&card)?;
    Ok(fetch_ed25519_pubkey(&card, acc.slot)?.to_vec())
}

/// Sign `message` with a PIV Ed25519 account. `pin` is used if `Some`, else prompted.
pub fn sign(acc: &Account, message: &[u8], pin: Option<&str>) -> Result<Vec<u8>, Error> {
    ensure_ed25519(acc)?;
    if !piv_slot::is_valid(acc.slot) {
        return Err(Error::Unsupported(format!(
            "{:#04x} is not a PIV signing slot",
            acc.slot
        )));
    }
    let (_ctx, card) = apdu::connect()?;
    select(&card)?;

    let pin_str = pin::resolve(pin, "Enter PIV PIN: ")?;
    let padded = pin_padded(&pin_str)?;
    let mut buf = [0u8; 512];
    apdu::send(
        &card,
        CLA,
        INS_VERIFY,
        0x00,
        P2_PIV_PIN,
        Some(&padded),
        None,
        &mut buf,
    )?;

    let ga = build_general_authenticate_data(message);
    let cmd = apdu::build_apdu_ext(
        CLA,
        INS_GENERAL_AUTHENTICATE,
        ALG_ED25519,
        acc.slot,
        &ga,
        Some(0),
    );
    let mut resp_buf = [0u8; 4096];
    let resp = apdu::transmit(&card, &cmd, &mut resp_buf)?;
    parse_signature_response(resp)
}

/// Enumerate all PIV signing slots, reporting which hold a readable Ed25519 key.
pub fn list_accounts() -> Result<Vec<SlotInfo>, Error> {
    let (_ctx, card) = apdu::connect()?;
    select(&card)?;
    Ok(piv_slot::all()
        .into_iter()
        .map(|slot| SlotInfo {
            slot,
            ed25519_pubkey: fetch_ed25519_pubkey(&card, slot).ok(),
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Card helpers (hardware-dependent).
// ---------------------------------------------------------------------------

fn ensure_ed25519(acc: &Account) -> Result<(), Error> {
    match acc.curve {
        Curve::Ed25519 => Ok(()),
        Curve::Secp256k1 => Err(Error::Unsupported(
            "PIV does not support secp256k1; use the OpenPGP applet".into(),
        )),
    }
}

fn select(card: &Card) -> Result<(), Error> {
    let mut buf = [0u8; 512];
    apdu::send(card, CLA, INS_SELECT, 0x04, 0x00, Some(AID), None, &mut buf)?;
    Ok(())
}

fn fetch_ed25519_pubkey(card: &Card, slot: u8) -> Result<[u8; 32], Error> {
    let tag = slot_object_tag(slot)?;
    let request = [0x5C, 0x03, tag[0], tag[1], tag[2]];
    let cmd = apdu::build_apdu_ext(CLA, INS_GET_DATA, 0x3F, 0xFF, &request, Some(0));
    let mut buf = [0u8; 4096];
    let resp = apdu::transmit(card, &cmd, &mut buf)?;
    let cert = extract_certificate(resp)?;
    extract_ed25519_from_cert(cert)
}

// ---------------------------------------------------------------------------
// Pure helpers (unit-tested).
// ---------------------------------------------------------------------------

/// Map a PIV slot id to its 3-byte data-object tag (the value of `5C`).
pub fn slot_object_tag(slot: u8) -> Result<[u8; 3], Error> {
    let low = match slot {
        piv_slot::AUTH => 0x05,
        piv_slot::SIGN => 0x0A,
        piv_slot::KEY_MGMT => 0x0B,
        piv_slot::CARD_AUTH => 0x01,
        piv_slot::RETIRED_FIRST..=piv_slot::RETIRED_LAST => 0x0D + (slot - piv_slot::RETIRED_FIRST),
        _ => return Err(Error::Unsupported(format!("not a PIV slot: {slot:#04x}"))),
    };
    Ok([0x5F, 0xC1, low])
}

/// Encode a BER-TLV definite length.
fn ber_len(n: usize) -> Vec<u8> {
    if n < 0x80 {
        vec![n as u8]
    } else if n <= 0xFF {
        vec![0x81, n as u8]
    } else if n <= 0xFFFF {
        vec![0x82, (n >> 8) as u8, n as u8]
    } else {
        vec![0x83, (n >> 16) as u8, (n >> 8) as u8, n as u8]
    }
}

/// Build the GENERAL AUTHENTICATE data field: `7C L { 82 00 81 L <message> }`.
fn build_general_authenticate_data(message: &[u8]) -> Vec<u8> {
    let mut inner = vec![TAG_RESPONSE, 0x00, TAG_CHALLENGE];
    inner.extend_from_slice(&ber_len(message.len()));
    inner.extend_from_slice(message);

    let mut data = vec![TAG_DYNAMIC_AUTH];
    data.extend_from_slice(&ber_len(inner.len()));
    data.extend_from_slice(&inner);
    data
}

/// Read a BER-TLV definite length starting at `i`; returns `(length, next_index)`.
fn read_ber_len(data: &[u8], mut i: usize) -> Result<(usize, usize), Error> {
    let first = *data
        .get(i)
        .ok_or_else(|| Error::Parse("TLV length truncated".into()))?;
    i += 1;
    if first < 0x80 {
        return Ok((first as usize, i));
    }
    let num = (first & 0x7F) as usize;
    if num == 0 || num > 4 {
        return Err(Error::Parse(format!("bad BER length byte {first:#04x}")));
    }
    let mut len = 0usize;
    for _ in 0..num {
        let b = *data
            .get(i)
            .ok_or_else(|| Error::Parse("TLV length truncated".into()))?;
        i += 1;
        len = (len << 8) | b as usize;
    }
    Ok((len, i))
}

/// Find a single-byte-tag TLV and return its value slice.
fn read_tlv(data: &[u8], tag: u8) -> Result<&[u8], Error> {
    let mut i = 0;
    while i < data.len() {
        let t = data[i];
        i += 1;
        let (len, after_len) = read_ber_len(data, i)?;
        let start = after_len;
        let end = start
            .checked_add(len)
            .filter(|&e| e <= data.len())
            .ok_or_else(|| Error::Parse(format!("TLV {t:#04x} overruns buffer")))?;
        if t == tag {
            return Ok(&data[start..end]);
        }
        i = end;
    }
    Err(Error::Parse(format!("TLV tag {tag:#04x} not found")))
}

/// Extract the signature bytes from a GENERAL AUTHENTICATE response
/// (`7C L { 82 L <signature> }`).
fn parse_signature_response(resp: &[u8]) -> Result<Vec<u8>, Error> {
    let dynamic = read_tlv(resp, TAG_DYNAMIC_AUTH)?;
    let sig = read_tlv(dynamic, TAG_RESPONSE)?;
    Ok(sig.to_vec())
}

/// Extract the X.509 certificate DER from a PIV GET DATA response
/// (`53 L { 70 L <cert> 71 01 <info> ... }`).
fn extract_certificate(resp: &[u8]) -> Result<&[u8], Error> {
    let object = read_tlv(resp, TAG_DATA_OBJECT)?;
    let cert = read_tlv(object, TAG_CERTIFICATE)?;
    if cert.is_empty() {
        return Err(Error::PubkeyNotFound("empty certificate in slot".into()));
    }
    Ok(cert)
}

/// Pull a 32-byte Ed25519 key out of an X.509 certificate DER by locating its
/// SubjectPublicKeyInfo prefix.
fn extract_ed25519_from_cert(der: &[u8]) -> Result<[u8; 32], Error> {
    match der
        .windows(ED25519_SPKI_PREFIX.len())
        .position(|w| w == ED25519_SPKI_PREFIX)
    {
        Some(pos) => {
            let start = pos + ED25519_SPKI_PREFIX.len();
            let end = start + 32;
            if end > der.len() {
                return Err(Error::PubkeyNotFound(
                    "truncated Ed25519 key in cert".into(),
                ));
            }
            Ok(<[u8; 32]>::try_from(&der[start..end]).expect("slice is exactly 32 bytes"))
        }
        None => Err(Error::PubkeyNotFound(
            "no Ed25519 SubjectPublicKeyInfo in certificate (is the slot Ed25519?)".into(),
        )),
    }
}

/// Pad a PIV PIN (ASCII, ≤ 8 bytes) to 8 bytes with `0xFF`.
fn pin_padded(pin: &str) -> Result<Vec<u8>, Error> {
    let bytes = pin.as_bytes();
    if bytes.len() > 8 {
        return Err(Error::Parse("PIV PIN must be at most 8 characters".into()));
    }
    let mut v = bytes.to_vec();
    v.resize(8, 0xFF);
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_slot_object_tags() {
        assert_eq!(slot_object_tag(0x9A).unwrap(), [0x5F, 0xC1, 0x05]);
        assert_eq!(slot_object_tag(0x9C).unwrap(), [0x5F, 0xC1, 0x0A]);
        assert_eq!(slot_object_tag(0x9D).unwrap(), [0x5F, 0xC1, 0x0B]);
        assert_eq!(slot_object_tag(0x9E).unwrap(), [0x5F, 0xC1, 0x01]);
    }

    #[test]
    fn retired_slot_object_tags_span_the_range() {
        assert_eq!(slot_object_tag(0x82).unwrap(), [0x5F, 0xC1, 0x0D]);
        assert_eq!(slot_object_tag(0x95).unwrap(), [0x5F, 0xC1, 0x20]);
    }

    #[test]
    fn invalid_slot_is_rejected() {
        assert!(slot_object_tag(0x00).is_err());
        assert!(slot_object_tag(0x96).is_err());
    }

    #[test]
    fn ber_len_short_and_long_forms() {
        assert_eq!(ber_len(0), vec![0x00]);
        assert_eq!(ber_len(127), vec![0x7F]);
        assert_eq!(ber_len(128), vec![0x81, 0x80]);
        assert_eq!(ber_len(255), vec![0x81, 0xFF]);
        assert_eq!(ber_len(256), vec![0x82, 0x01, 0x00]);
        assert_eq!(ber_len(300), vec![0x82, 0x01, 0x2C]);
    }

    #[test]
    fn general_authenticate_template_short_message() {
        let msg = [0xDE, 0xAD, 0xBE, 0xEF];
        let data = build_general_authenticate_data(&msg);
        // 7C 08 | 82 00 81 04 DE AD BE EF
        assert_eq!(
            data,
            vec![0x7C, 0x08, 0x82, 0x00, 0x81, 0x04, 0xDE, 0xAD, 0xBE, 0xEF]
        );
    }

    #[test]
    fn general_authenticate_template_long_message() {
        let msg = vec![0xAB; 200];
        let data = build_general_authenticate_data(&msg);
        // inner = 82 00 81 81 C8 <200> => len 205 -> outer 7C 81 CD ...
        assert_eq!(&data[..2], &[0x7C, 0x81]);
        assert_eq!(data[2], 205); // 0xCD
        assert_eq!(&data[3..7], &[0x82, 0x00, 0x81, 0x81]);
        assert_eq!(data[7], 200); // 0xC8
        assert_eq!(data.len(), 3 + 205);
    }

    #[test]
    fn round_trip_signature_response() {
        // 7C 42 82 40 <64-byte sig>  (outer len = 2 + 64 = 66 = 0x42)
        let sig: Vec<u8> = (0..64).collect();
        let mut resp = vec![0x7C, 0x42, 0x82, 0x40];
        resp.extend_from_slice(&sig);
        assert_eq!(parse_signature_response(&resp).unwrap(), sig);
    }

    #[test]
    fn extract_cert_then_key() {
        // Build a minimal cert DER that embeds the Ed25519 SPKI.
        let key: [u8; 32] = core::array::from_fn(|i| (i + 1) as u8);
        let mut cert = vec![0xAA, 0xBB]; // some leading cert bytes
        cert.extend_from_slice(&ED25519_SPKI_PREFIX);
        cert.extend_from_slice(&key);
        cert.extend_from_slice(&[0xCC]); // trailing bytes

        // Wrap as PIV GET DATA: 53 L { 70 L <cert> 71 01 00 }
        let mut object = vec![TAG_CERTIFICATE];
        object.extend_from_slice(&ber_len(cert.len()));
        object.extend_from_slice(&cert);
        object.extend_from_slice(&[0x71, 0x01, 0x00]);

        let mut resp = vec![TAG_DATA_OBJECT];
        resp.extend_from_slice(&ber_len(object.len()));
        resp.extend_from_slice(&object);

        let extracted = extract_certificate(&resp).unwrap();
        assert_eq!(extract_ed25519_from_cert(extracted).unwrap(), key);
    }

    #[test]
    fn cert_without_ed25519_key_errors() {
        let der = [0x30, 0x10, 0x06, 0x03, 0x2A, 0x86, 0x48]; // not Ed25519
        assert!(matches!(
            extract_ed25519_from_cert(&der),
            Err(Error::PubkeyNotFound(_))
        ));
    }

    #[test]
    fn pin_padding_to_eight_bytes() {
        assert_eq!(
            pin_padded("123456").unwrap(),
            vec![0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0xFF, 0xFF]
        );
        assert_eq!(pin_padded("12345678").unwrap().len(), 8);
        assert!(pin_padded("123456789").is_err());
    }
}
