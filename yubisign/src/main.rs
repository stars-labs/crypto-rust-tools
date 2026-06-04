//! `yubisign` CLI: a seedless multi-chain hardware wallet on a YubiKey.
//!
//! Examples:
//!   yubisign list
//!   yubisign address --applet piv --slot 9a --curve ed25519
//!   yubisign address --applet openpgp --slot sig --curve secp256k1
//!   yubisign ssh-to-solana "ssh-ed25519 AAAA..."

use clap::{Parser, Subcommand};
use std::process::ExitCode;
use yubisign::{Account, Applet, Curve, eth, get_pubkey, parse_slot};

#[derive(Parser)]
#[command(
    name = "yubisign",
    about = "Seedless multi-chain hardware wallet on a YubiKey"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Enumerate all accounts (OpenPGP SIG/AUT + PIV Ed25519 slots).
    List,
    /// Show the address for one account.
    Address(AccountArgs),
    /// Convert a GPG-exported Ed25519 SSH public key to a Solana address.
    SshToSolana {
        /// SSH key (`ssh-ed25519 AAAA... [comment]`). Reads stdin if omitted.
        ssh_key: Option<String>,
    },
}

#[derive(Parser)]
struct AccountArgs {
    /// Applet: openpgp | piv
    #[arg(long, default_value = "piv")]
    applet: String,
    /// Slot: openpgp sig|aut, or piv auth|sign|keymgmt|cardauth|<hex e.g. 9a,82>
    #[arg(long, default_value = "9a")]
    slot: String,
    /// Curve: ed25519 | secp256k1
    #[arg(long, default_value = "ed25519")]
    curve: String,
}

impl AccountArgs {
    fn to_account(&self) -> Result<Account, String> {
        let applet: Applet = self.applet.parse().map_err(|e| format!("{e}"))?;
        let curve: Curve = self.curve.parse().map_err(|e| format!("{e}"))?;
        let slot = parse_slot(applet, &self.slot).map_err(|e| format!("{e}"))?;
        if applet == Applet::Piv && curve == Curve::Secp256k1 {
            return Err("PIV does not support secp256k1; use --applet openpgp".into());
        }
        Ok(Account {
            applet,
            slot,
            curve,
        })
    }
}

/// Render an account's public key as the relevant chain address.
fn format_address(curve: Curve, pubkey: &[u8]) -> Result<String, String> {
    match curve {
        Curve::Ed25519 => Ok(format!("Solana: {}", bs58::encode(pubkey).into_string())),
        Curve::Secp256k1 => {
            let addr = eth::address_from_pubkey(pubkey).map_err(|e| format!("{e}"))?;
            Ok(format!("Ethereum: {}", eth::address_to_hex(&addr)))
        }
    }
}

fn run() -> Result<(), String> {
    match Cli::parse().command {
        Command::List => {
            // OpenPGP SIG/AUT slots: each holds one key (Solana if Ed25519,
            // Ethereum if secp256k1).
            use yubisign::openpgp_slot;
            for (name, slot) in [("SIG", openpgp_slot::SIG), ("AUT", openpgp_slot::AUT)] {
                println!("OpenPGP ({name} slot):");
                match yubisign::openpgp_account(slot) {
                    Ok((curve, pk)) => {
                        println!("  {}  ({})", format_address(curve, &pk)?, hex::encode(&pk))
                    }
                    Err(e) => println!("  (none: {e})"),
                }
            }

            // PIV slots: Ed25519 → Solana.
            println!("PIV slots (Ed25519 → Solana):");
            let mut found = 0;
            for info in yubisign::list_piv_accounts().map_err(|e| format!("{e}"))? {
                if let Some(pk) = info.ed25519_pubkey {
                    found += 1;
                    println!(
                        "  slot {:#04x}  Solana: {}  ({})",
                        info.slot,
                        bs58::encode(pk).into_string(),
                        hex::encode(pk)
                    );
                }
            }
            if found == 0 {
                println!(
                    "  (none — generate one, e.g. `ykman piv keys generate -a ED25519 9a ...`)"
                );
            }
            Ok(())
        }
        Command::Address(args) => {
            let account = args.to_account()?;
            let pubkey = get_pubkey(&account).map_err(|e| format!("{e}"))?;
            println!("Public key: {}", hex::encode(&pubkey));
            println!("{}", format_address(account.curve, &pubkey)?);
            Ok(())
        }
        Command::SshToSolana { ssh_key } => {
            let key = match ssh_key {
                Some(k) => k,
                None => {
                    use std::io::BufRead;
                    std::io::stdin()
                        .lock()
                        .lines()
                        .next()
                        .ok_or("no input provided")?
                        .map_err(|e| format!("{e}"))?
                }
            };
            println!("{}", ssh_ed25519_to_solana(&key)?);
            Ok(())
        }
    }
}

/// Extract the Ed25519 key from an `ssh-ed25519` public key and return its
/// Solana (base58) address. Wire format: `<len>"ssh-ed25519"<len><32-byte key>`.
fn ssh_ed25519_to_solana(ssh_key: &str) -> Result<String, String> {
    use base64::Engine;
    let parts: Vec<&str> = ssh_key.split_whitespace().collect();
    if parts.len() < 2 || parts[0] != "ssh-ed25519" {
        return Err("expected 'ssh-ed25519 <base64> [comment]'".into());
    }
    let raw = base64::engine::general_purpose::STANDARD
        .decode(parts[1])
        .map_err(|e| format!("base64 decode: {e}"))?;
    let idx = 4 + "ssh-ed25519".len() + 4;
    let key = raw.get(idx..idx + 32).ok_or("SSH key data too short")?;
    Ok(bs58::encode(key).into_string())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {e}");
            ExitCode::FAILURE
        }
    }
}
