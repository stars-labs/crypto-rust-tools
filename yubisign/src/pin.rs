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
