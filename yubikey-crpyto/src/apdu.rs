//! APDU construction and PC/SC transport.
//!
//! `build_apdu` is a pure function (unit-tested); `connect`/`transmit`/`send`
//! wrap the hardware and are exercised by manual integration runs.

use crate::error::{Error, status_word_message};
use pcsc::{Card, Context, Protocols, Scope, ShareMode};

// Shared instruction bytes used across the OpenPGP and PIV applets.
pub const CLA: u8 = 0x00;
pub const INS_SELECT: u8 = 0xA4;
pub const INS_VERIFY: u8 = 0x20;
pub const INS_PSO: u8 = 0x2A;
pub const INS_INTERNAL_AUTHENTICATE: u8 = 0x88;
pub const INS_GET_PUBLIC_KEY: u8 = 0x47;
pub const INS_GENERAL_AUTHENTICATE: u8 = 0x87;
pub const INS_GET_DATA: u8 = 0xCB;

/// Build a short-form (Lc/Le ≤ 255) APDU command.
///
/// Layout mirrors the original `send_apdu` behavior:
/// - `Some(data)` -> append `Lc || data`, then optional `Le`.
/// - `None` data with no `Le` -> append a single `0x00` (Le = 256).
/// - `None` data with `Le` -> append only `Le`.
pub fn build_apdu(
    cla: u8,
    ins: u8,
    p1: u8,
    p2: u8,
    data: Option<&[u8]>,
    le: Option<u8>,
) -> Result<Vec<u8>, Error> {
    let mut cmd = vec![cla, ins, p1, p2];
    match data {
        Some(d) => {
            if d.len() > 255 {
                return Err(Error::DataTooLong(d.len()));
            }
            cmd.push(d.len() as u8);
            cmd.extend_from_slice(d);
        }
        None => {
            if le.is_none() {
                cmd.push(0x00);
            }
        }
    }
    if let Some(le) = le {
        cmd.push(le);
    }
    Ok(cmd)
}

/// Build an extended-length APDU command.
///
/// Used for PIV operations whose data or expected response can exceed 255
/// bytes (e.g. signing a full Solana message, or reading a certificate):
/// - `data`: encoded as `0x00 <Lc-hi> <Lc-lo> || data` (3-byte Lc).
/// - `le`: `Some(n)` appends a 2-byte Le (`0x0000` means "up to 65536").
pub fn build_apdu_ext(cla: u8, ins: u8, p1: u8, p2: u8, data: &[u8], le: Option<u16>) -> Vec<u8> {
    let mut cmd = vec![cla, ins, p1, p2];
    cmd.push(0x00);
    cmd.push((data.len() >> 8) as u8);
    cmd.push((data.len() & 0xFF) as u8);
    cmd.extend_from_slice(data);
    if let Some(le) = le {
        cmd.push((le >> 8) as u8);
        cmd.push((le & 0xFF) as u8);
    }
    cmd
}

/// Establish a PC/SC context and connect to the first available reader.
///
/// Returns both the context and card; the context must outlive the card.
pub fn connect() -> Result<(Context, Card), Error> {
    let ctx = Context::establish(Scope::User)?;
    let mut readers_buf = [0u8; 2048];
    let reader = {
        let mut readers = ctx.list_readers(&mut readers_buf)?;
        readers.next().ok_or(Error::NoReaders)?.to_owned()
    };
    let card = ctx.connect(&reader, ShareMode::Shared, Protocols::T0 | Protocols::T1)?;
    Ok((ctx, card))
}

/// Transmit a raw APDU and return the response data (status word stripped).
///
/// Maps a non-`0x9000` status word into [`Error::Card`].
pub fn transmit<'a>(card: &Card, command: &[u8], buf: &'a mut [u8]) -> Result<&'a [u8], Error> {
    let resp = card.transmit(command, buf)?;
    if resp.len() < 2 {
        return Err(Error::ShortResponse);
    }
    let n = resp.len();
    let status = ((resp[n - 2] as u16) << 8) | resp[n - 1] as u16;
    if status == 0x9000 {
        Ok(&resp[..n - 2])
    } else {
        Err(Error::Card {
            status,
            message: status_word_message(status),
        })
    }
}

/// Convenience wrapper: build a short-form APDU then transmit it.
#[allow(clippy::too_many_arguments)]
pub fn send<'a>(
    card: &Card,
    cla: u8,
    ins: u8,
    p1: u8,
    p2: u8,
    data: Option<&[u8]>,
    le: Option<u8>,
    buf: &'a mut [u8],
) -> Result<&'a [u8], Error> {
    let cmd = build_apdu(cla, ins, p1, p2, data, le)?;
    transmit(card, &cmd, buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_and_le_are_appended() {
        // CLA INS P1 P2 Lc <data> Le
        let cmd = build_apdu(0x00, 0x2A, 0x9E, 0x9A, Some(&[0xAA, 0xBB]), Some(0x00)).unwrap();
        assert_eq!(cmd, vec![0x00, 0x2A, 0x9E, 0x9A, 0x02, 0xAA, 0xBB, 0x00]);
    }

    #[test]
    fn no_data_no_le_appends_single_zero() {
        let cmd = build_apdu(0x00, 0xA4, 0x04, 0x00, None, None).unwrap();
        assert_eq!(cmd, vec![0x00, 0xA4, 0x04, 0x00, 0x00]);
    }

    #[test]
    fn no_data_with_le_appends_only_le() {
        let cmd = build_apdu(0x00, 0xCB, 0x3F, 0xFF, None, Some(0x00)).unwrap();
        assert_eq!(cmd, vec![0x00, 0xCB, 0x3F, 0xFF, 0x00]);
    }

    #[test]
    fn data_without_le_has_no_trailing_byte() {
        let cmd = build_apdu(0x00, 0x20, 0x00, 0x81, Some(&[0x31, 0x32]), None).unwrap();
        assert_eq!(cmd, vec![0x00, 0x20, 0x00, 0x81, 0x02, 0x31, 0x32]);
    }

    #[test]
    fn oversized_data_is_rejected() {
        let big = vec![0u8; 256];
        assert!(matches!(
            build_apdu(0x00, 0x2A, 0x00, 0x00, Some(&big), None),
            Err(Error::DataTooLong(256))
        ));
    }

    #[test]
    fn extended_apdu_encodes_three_byte_lc() {
        let data = [0xAA, 0xBB, 0xCC];
        let cmd = build_apdu_ext(0x00, 0x87, 0xE0, 0x9A, &data, Some(0));
        // CLA INS P1 P2 | 00 Lc-hi Lc-lo | data | Le-hi Le-lo
        assert_eq!(
            cmd,
            vec![
                0x00, 0x87, 0xE0, 0x9A, 0x00, 0x00, 0x03, 0xAA, 0xBB, 0xCC, 0x00, 0x00
            ]
        );
    }

    #[test]
    fn extended_apdu_handles_data_over_255_bytes() {
        let data = vec![0x11; 300];
        let cmd = build_apdu_ext(0x00, 0x87, 0xE0, 0x9A, &data, None);
        assert_eq!(&cmd[..7], &[0x00, 0x87, 0xE0, 0x9A, 0x00, 0x01, 0x2C]); // Lc = 300
        assert_eq!(cmd.len(), 7 + 300); // no Le appended
    }
}
