use anyhow::{Context, Result, anyhow};
use clap::Parser;
use std::io::{self, BufRead};

#[derive(Parser)]
#[clap(
    author,
    version,
    about = "Convert GPG-exported ED25519 SSH key to Solana address"
)]
struct Cli {
    /// SSH key string. If not provided, reads from stdin
    #[clap(value_name = "SSH_KEY")]
    ssh_key: Option<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let ssh_key = if let Some(key) = cli.ssh_key {
        key
    } else {
        let stdin = io::stdin();
        let mut lines = stdin.lock().lines();
        lines.next().ok_or_else(|| anyhow!("No input provided"))??
    };

    let solana_address = convert_ssh_to_solana(&ssh_key)?;
    println!("{}", solana_address);

    Ok(())
}

fn convert_ssh_to_solana(ssh_key: &str) -> Result<String> {
    // Extract the base64 part from the SSH key
    let parts: Vec<&str> = ssh_key.split_whitespace().collect();
    if parts.len() < 2 {
        return Err(anyhow!(
            "Invalid SSH key format. Expected: 'ssh-ed25519 BASE64_DATA [COMMENT]'"
        ));
    }

    if parts[0] != "ssh-ed25519" {
        return Err(anyhow!(
            "Only ED25519 keys are supported, found: {}",
            parts[0]
        ));
    }

    let base64_data = parts[1];

    // Decode the base64 data
    let ssh_key_bytes = base64::decode(base64_data).context("Failed to decode base64 SSH key")?;

    // The format of the SSH key is:
    // - 4 bytes: length of "ssh-ed25519" string
    // - n bytes: "ssh-ed25519" string
    // - 4 bytes: length of public key
    // - 32 bytes: public key

    // Skip the first part until we get to the public key
    let mut index = 4 + "ssh-ed25519".len();

    // Read the length of the public key
    if index + 4 > ssh_key_bytes.len() {
        return Err(anyhow!("SSH key data is too short"));
    }

    // Skip the length field (4 bytes) and read the public key
    index += 4;
    if index + 32 > ssh_key_bytes.len() {
        return Err(anyhow!(
            "SSH key data is too short to contain ED25519 public key"
        ));
    }

    let pubkey_bytes = &ssh_key_bytes[index..index + 32];

    // Convert to Solana address (base58 encoded)
    let solana_address = bs58::encode(pubkey_bytes).into_string();

    Ok(solana_address)
}
