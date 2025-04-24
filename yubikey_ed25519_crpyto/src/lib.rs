use hex;
use pcsc::*;
use std::error::Error;
use std::io::{self, Write};

const OPENGPG_AID_HEX: &str = "D27600012401";
const INS_SELECT: u8 = 0xA4;
const INS_VERIFY: u8 = 0x20;
const P2_VERIFY_USER_PIN: u8 = 0x81;
const CLA_GP: u8 = 0x00;
const INS_PSO: u8 = 0x2A;
const P1_SIGN: u8 = 0x9E;
const P2_COMPUTE_SIGNATURE: u8 = 0x9A;
const INS_GET_PUBLIC_KEY: u8 = 0x47;
const P1_SIG_KEY: u8 = 0x81;
const P2_GET_PUBKEY: u8 = 0x00;

/// Sends an APDU command to the card and returns the response data.
fn send_apdu<'a>(
    card: &Card,
    _description: &str,
    cla: u8,
    ins: u8,
    p1: u8,
    p2: u8,
    data: Option<&[u8]>,
    le: Option<u8>,
    response_buffer: &'a mut [u8],
) -> Result<&'a [u8], Box<dyn Error>> {
    let mut command = Vec::new();
    command.extend_from_slice(&[cla, ins, p1, p2]);

    match data {
        Some(d) => {
            if d.len() > 255 {
                return Err("Data field too long for simple APDU (max 255 bytes)".into());
            }
            command.push(d.len() as u8);
            command.extend_from_slice(d);
        }
        None => {
            if le.is_none() {
                command.push(0x00);
            }
        }
    }

    if let Some(le_byte) = le {
        command.push(le_byte);
    }

    let response_slice = card.transmit(&command, response_buffer)?;

    if response_slice.len() < 2 {
        return Err("Received response is too short (less than 2 bytes)".into());
    }

    let response_len = response_slice.len();
    let sw1 = response_slice[response_len - 2];
    let sw2 = response_slice[response_len - 1];
    let status = ((sw1 as u16) << 8) | (sw2 as u16);

    let data_slice = &response_slice[..response_len - 2];

    if status == 0x9000 {
        Ok(data_slice)
    } else {
        let error_msg = match status {
            0x63C0..=0x63CF => format!("PIN verification failed. Retries left: {}", status & 0x0F),
            0x6983 => {
                "PIN verification failed. Authentication method blocked (PIN Locked)".to_string()
            }
            0x6982 => "Security status not satisfied (e.g., PIN required or wrong PIN context)"
                .to_string(),
            0x6985 => "Conditions of use not satisfied (e.g., key usage not allowed)".to_string(),
            0x6A80 => "Wrong data field parameters (e.g., incorrect PIN length/format)".to_string(),
            0x6A86 => "Incorrect P1/P2 parameters".to_string(),
            0x6A88 => {
                "Referenced data not found (e.g., key invalid or PIN reference wrong)".to_string()
            }
            0x6D00 => "Instruction not supported".to_string(),
            0x6E00 => "Class not supported".to_string(),
            _ => format!("Card returned error status: {:04X}", status),
        };
        Err(error_msg.into())
    }
}

/// Prompts the user for their PIN securely (without echoing).
pub fn get_pin_from_user(prompt: &str) -> Result<String, Box<dyn Error>> {
    print!("{}", prompt);
    io::stdout().flush()?;
    let mut pin = String::new();
    io::stdin().read_line(&mut pin)?;
    Ok(pin.trim().to_string())
}

/// Fetch the public key from the YubiKey (OpenPGP signature key)
pub fn get_pubkey_from_yubikey() -> Result<[u8; 32], Box<dyn Error>> {
    let ctx = Context::establish(Scope::User)?;
    let mut readers_buf = [0; 2048];
    let mut readers = ctx.list_readers(&mut readers_buf)?;
    let reader = readers.next().ok_or("No smart card readers found.")?;
    let card = ctx.connect(reader, ShareMode::Shared, Protocols::T0 | Protocols::T1)?;

    let mut response_buffer = [0u8; 512];

    let aid_bytes = hex::decode(OPENGPG_AID_HEX)?;
    send_apdu(
        &card,
        "SELECT APP",
        CLA_GP,
        INS_SELECT,
        0x04,
        0x00,
        Some(aid_bytes.as_slice()),
        None,
        &mut response_buffer,
    )?;

    response_buffer.fill(0);
    let pubkey_data = {
        let apdu = vec![
            CLA_GP,
            INS_GET_PUBLIC_KEY,
            P1_SIG_KEY,
            P2_GET_PUBKEY,
            0x00,
            0x00,
            0x02,
            0xB6,
            0x00,
            0x00,
            0x00,
        ];
        let response_slice = card.transmit(&apdu, &mut response_buffer)?;
        if response_slice.len() < 2 {
            return Err("Received response is too short (less than 2 bytes)".into());
        }
        let response_len = response_slice.len();
        let sw1 = response_slice[response_len - 2];
        let sw2 = response_slice[response_len - 1];
        let status = ((sw1 as u16) << 8) | (sw2 as u16);
        let data_slice = &response_slice[..response_len - 2];
        if status == 0x9000 {
            data_slice
        } else {
            let error_msg = match status {
                0x63C0..=0x63CF => {
                    format!("PIN verification failed. Retries left: {}", status & 0x0F)
                }
                0x6983 => "PIN verification failed. Authentication method blocked (PIN Locked)"
                    .to_string(),
                0x6982 => "Security status not satisfied (e.g., PIN required or wrong PIN context)"
                    .to_string(),
                0x6985 => {
                    "Conditions of use not satisfied (e.g., key usage not allowed)".to_string()
                }
                0x6A80 => {
                    "Wrong data field parameters (e.g., incorrect PIN length/format)".to_string()
                }
                0x6A86 => "Incorrect P1/P2 parameters".to_string(),
                0x6A88 => "Referenced data not found (e.g., key invalid or PIN reference wrong)"
                    .to_string(),
                0x6D00 => "Instruction not supported".to_string(),
                0x6E00 => "Class not supported".to_string(),
                _ => format!("Card returned error status: {:04X}", status),
            };
            return Err(error_msg.into());
        }
    };

    if pubkey_data.len() < 2 {
        return Err("GET PUBLIC KEY response too short or empty (no public key returned)".into());
    }

    let key_bytes = if let Some(pos) = pubkey_data.windows(2).position(|w| w == [0x86, 0x20]) {
        &pubkey_data[pos + 2..pos + 2 + 32]
    } else {
        return Err(format!(
            "Could not find Ed25519 public key in response (TLV: {})",
            hex::encode(pubkey_data)
        )
        .into());
    };

    Ok(<[u8; 32]>::try_from(key_bytes)?)
}

/// Sign the provided message bytes with the YubiKey's signature key.
pub fn sign_with_yubikey(message: &[u8]) -> Result<Vec<u8>, Box<dyn Error>> {
    let ctx = Context::establish(Scope::User)?;

    let mut readers_buf = [0; 2048];
    let mut readers = ctx.list_readers(&mut readers_buf)?;

    let reader = match readers.next() {
        Some(reader) => reader,
        None => return Err("No smart card readers found.".into()),
    };

    let card = ctx.connect(reader, ShareMode::Shared, Protocols::T0 | Protocols::T1)?;

    let mut response_buffer = [0u8; 512];

    let aid_bytes = hex::decode(OPENGPG_AID_HEX)?;
    let select_apdu_data = Some(aid_bytes.as_slice());
    send_apdu(
        &card,
        "SELECT APP",
        CLA_GP,
        INS_SELECT,
        0x04,
        0x00,
        select_apdu_data,
        None,
        &mut response_buffer,
    )?;

    let user_pin = get_pin_from_user("Enter User PIN (PW1/PIN2): ")?;

    let pin_bytes = user_pin.as_bytes();
    response_buffer.fill(0);
    send_apdu(
        &card,
        "VERIFY PIN",
        CLA_GP,
        INS_VERIFY,
        0x00,
        P2_VERIFY_USER_PIN,
        Some(pin_bytes),
        None,
        &mut response_buffer,
    )?;

    let pso_data = Some(message);
    response_buffer.fill(0);
    let signature_data_slice = send_apdu(
        &card,
        "PSO SIGN",
        CLA_GP,
        INS_PSO,
        P1_SIGN,
        P2_COMPUTE_SIGNATURE,
        pso_data,
        Some(0x00),
        &mut response_buffer,
    )?;

    Ok(signature_data_slice.to_vec())
}
