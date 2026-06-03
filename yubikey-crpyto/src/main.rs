//! `yubikey-crypto` CLI: discover and inspect YubiKey signing accounts.
//!
//! Examples:
//!   yubikey-crypto list
//!   yubikey-crypto address --applet piv --slot 9a --curve ed25519
//!   yubikey-crypto address --applet openpgp --slot sig --curve secp256k1

use clap::{Parser, Subcommand};
use std::process::ExitCode;
use yubikey_crypto::{Account, Applet, Curve, eth, get_pubkey, parse_slot};

#[derive(Parser)]
#[command(
    name = "yubikey-crypto",
    about = "YubiKey multi-account signing utility"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Enumerate PIV signing slots and show any Ed25519 accounts found.
    List,
    /// Show the address for one account.
    Address(AccountArgs),
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
            use yubikey_crypto::openpgp_slot;
            for (name, slot) in [("SIG", openpgp_slot::SIG), ("AUT", openpgp_slot::AUT)] {
                println!("OpenPGP ({name} slot):");
                match yubikey_crypto::openpgp_account(slot) {
                    Ok((curve, pk)) => {
                        println!("  {}  ({})", format_address(curve, &pk)?, hex::encode(&pk))
                    }
                    Err(e) => println!("  (none: {e})"),
                }
            }

            // PIV slots: Ed25519 → Solana.
            println!("PIV slots (Ed25519 → Solana):");
            let mut found = 0;
            for info in yubikey_crypto::list_piv_accounts().map_err(|e| format!("{e}"))? {
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
    }
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
