//! YubiKey hardware signing for blockchain transactions.
//!
//! Keys are selected via an [`Account`] = applet + slot + curve, letting one
//! YubiKey hold multiple accounts:
//!
//! - **Ed25519** (Solana, etc.) via PIV (~24 slots) or OpenPGP.
//! - **secp256k1** (Ethereum, etc.) via OpenPGP only (~2-3 slots; PIV cannot
//!   do secp256k1).
//!
//! The high-level entry points are [`get_pubkey`] and [`sign`]. The original
//! single-key functions [`get_pubkey_from_yubikey`] / [`sign_with_yubikey`]
//! remain as thin wrappers over the OpenPGP signature slot.

mod account;
mod apdu;
mod error;
pub mod eth;
mod openpgp;
mod pin;
mod piv;

pub use account::{Account, Applet, Curve, openpgp_slot, parse_slot, piv_slot};
pub use error::{Error, status_word_message};
pub use openpgp::detect_account as openpgp_account;
pub use piv::{SlotInfo as PivSlotInfo, list_accounts as list_piv_accounts};

/// Fetch the public key for `account`.
pub fn get_pubkey(account: &Account) -> Result<Vec<u8>, Error> {
    match account.applet {
        Applet::OpenPgp => openpgp::get_pubkey(account),
        Applet::Piv => piv::get_pubkey(account),
    }
}

/// Sign `message` with `account`'s key. May prompt for a PIN.
pub fn sign(account: &Account, message: &[u8]) -> Result<Vec<u8>, Error> {
    match account.applet {
        Applet::OpenPgp => openpgp::sign(account, message),
        Applet::Piv => piv::sign(account, message),
    }
}

// ---------------------------------------------------------------------------
// Backward-compatible API (OpenPGP signature slot, Ed25519).
// ---------------------------------------------------------------------------

/// Prompt the user for their PIN (without trailing newline).
pub fn get_pin_from_user(prompt: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(pin::prompt(prompt)?)
}

/// Fetch the Ed25519 public key from the OpenPGP signature slot.
pub fn get_pubkey_from_yubikey() -> Result<[u8; 32], Box<dyn std::error::Error>> {
    let pk = get_pubkey(&Account::openpgp_sig())?;
    Ok(<[u8; 32]>::try_from(pk.as_slice())?)
}

/// Sign `message` with the OpenPGP signature slot.
pub fn sign_with_yubikey(message: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    Ok(sign(&Account::openpgp_sig(), message)?)
}
