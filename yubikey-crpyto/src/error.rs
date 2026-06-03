//! Error types and smart-card status-word decoding.

/// Errors returned by YubiKey card operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("PC/SC error: {0}")]
    Pcsc(#[from] pcsc::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("no smart card readers found")]
    NoReaders,

    #[error("card response too short (< 2 bytes)")]
    ShortResponse,

    #[error("card error {status:04X}: {message}")]
    Card { status: u16, message: String },

    #[error("APDU data field too long ({0} bytes, max 255 for short APDU)")]
    DataTooLong(usize),

    #[error("public key not found: {0}")]
    PubkeyNotFound(String),

    #[error("unsupported operation: {0}")]
    Unsupported(String),

    #[error("parse error: {0}")]
    Parse(String),

    #[error("could not recover Ethereum recovery id from signature")]
    RecoveryFailed,

    #[error("signature error: {0}")]
    Signature(String),
}

/// Human-readable description of a smart-card status word (SW1 SW2).
///
/// Pure function over the 16-bit status; covers the OpenPGP/PIV error codes
/// this crate cares about and falls back to a hex dump for anything else.
pub fn status_word_message(status: u16) -> String {
    match status {
        0x9000 => "success".to_string(),
        0x63C0..=0x63CF => format!("PIN verification failed. Retries left: {}", status & 0x0F),
        0x6983 => "PIN verification failed. Authentication method blocked (PIN locked)".to_string(),
        0x6982 => {
            "Security status not satisfied (e.g., PIN required or wrong PIN context)".to_string()
        }
        0x6985 => "Conditions of use not satisfied (e.g., key usage not allowed)".to_string(),
        0x6A80 => "Wrong data field parameters (e.g., incorrect PIN length/format)".to_string(),
        0x6A86 => "Incorrect P1/P2 parameters".to_string(),
        0x6A88 => {
            "Referenced data not found (e.g., key invalid or PIN reference wrong)".to_string()
        }
        0x6D00 => "Instruction not supported".to_string(),
        0x6E00 => "Class not supported".to_string(),
        _ => format!("Card returned error status: {status:04X}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_retry_count_is_decoded() {
        assert_eq!(
            status_word_message(0x63C2),
            "PIN verification failed. Retries left: 2"
        );
        assert_eq!(
            status_word_message(0x63C0),
            "PIN verification failed. Retries left: 0"
        );
    }

    #[test]
    fn known_status_words_have_descriptions() {
        assert!(status_word_message(0x6983).contains("blocked"));
        assert!(status_word_message(0x6982).contains("Security status"));
        assert!(status_word_message(0x6985).contains("Conditions of use"));
        assert_eq!(status_word_message(0x9000), "success");
    }

    #[test]
    fn unknown_status_word_falls_back_to_hex() {
        assert_eq!(
            status_word_message(0x1234),
            "Card returned error status: 1234"
        );
    }
}
