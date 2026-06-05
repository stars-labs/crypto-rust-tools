//! Interactive PIN entry.

use crate::error::Error;
use std::io::{self, Write};

/// Prompt the user for a PIN on stdin and return the trimmed string.
pub fn prompt(message: &str) -> Result<String, Error> {
    print!("{message}");
    io::stdout().flush()?;
    let mut pin = String::new();
    io::stdin().read_line(&mut pin)?;
    Ok(pin.trim().to_string())
}

/// Use the caller-supplied PIN if present, otherwise prompt on stdin.
///
/// Lets non-interactive callers (e.g. the native-messaging host) provide the
/// PIN without the library reading from stdin.
pub fn resolve(pin: Option<&str>, message: &str) -> Result<String, Error> {
    match pin {
        Some(p) => Ok(p.to_string()),
        None => prompt(message),
    }
}
