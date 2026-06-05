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
mod error;
pub mod eth;

// Card I/O lives behind the default `card` feature. Disabling it (e.g. for a
// wasm build of the chain-independent logic) drops the pcsc dependency.
#[cfg(feature = "card")]
mod apdu;
#[cfg(feature = "card")]
mod openpgp;
#[cfg(feature = "card")]
mod pin;
#[cfg(feature = "card")]
mod piv;

pub use account::{Account, Applet, Curve, openpgp_slot, parse_slot, piv_slot};
pub use error::{Error, status_word_message};
#[cfg(feature = "card")]
pub use openpgp::detect_account as openpgp_account;
#[cfg(feature = "card")]
pub use piv::{SlotInfo as PivSlotInfo, list_accounts as list_piv_accounts};

/// Fetch the public key for `account`.
#[cfg(feature = "card")]
pub fn get_pubkey(account: &Account) -> Result<Vec<u8>, Error> {
    match account.applet {
        Applet::OpenPgp => openpgp::get_pubkey(account),
        Applet::Piv => piv::get_pubkey(account),
    }
}

/// Sign `message` with `account`'s key, prompting for the PIN on stdin.
#[cfg(feature = "card")]
pub fn sign(account: &Account, message: &[u8]) -> Result<Vec<u8>, Error> {
    sign_dispatch(account, message, None)
}

/// Sign `message` with a caller-supplied PIN (no stdin prompt).
///
/// Intended for non-interactive callers such as the native-messaging host.
#[cfg(feature = "card")]
pub fn sign_with_pin(account: &Account, message: &[u8], pin: &str) -> Result<Vec<u8>, Error> {
    sign_dispatch(account, message, Some(pin))
}

#[cfg(feature = "card")]
fn sign_dispatch(account: &Account, message: &[u8], pin: Option<&str>) -> Result<Vec<u8>, Error> {
    match account.applet {
        Applet::OpenPgp => openpgp::sign(account, message, pin),
        Applet::Piv => piv::sign(account, message, pin),
    }
}

// ---------------------------------------------------------------------------
// Backward-compatible API (OpenPGP signature slot, Ed25519).
// ---------------------------------------------------------------------------

/// Prompt the user for their PIN (without trailing newline).
#[cfg(feature = "card")]
pub fn get_pin_from_user(prompt: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(pin::prompt(prompt)?)
}

/// Fetch the Ed25519 public key from the OpenPGP signature slot.
#[cfg(feature = "card")]
pub fn get_pubkey_from_yubikey() -> Result<[u8; 32], Box<dyn std::error::Error>> {
    let pk = get_pubkey(&Account::openpgp_sig())?;
    Ok(<[u8; 32]>::try_from(pk.as_slice())?)
}

/// Sign `message` with the OpenPGP signature slot.
#[cfg(feature = "card")]
pub fn sign_with_yubikey(message: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    Ok(sign(&Account::openpgp_sig(), message)?)
}
