/**
 * End-to-end example: connect → negotiate → list accounts → sign.
 *
 * Run inside an extension background context (where `chrome.runtime` exists).
 * Wire it to a button/message handler; this is illustrative, not production UI.
 */
import { HostClient } from "./host-client";

export async function demo() {
  const host = new HostClient();

  // 1. Negotiate: confirm protocol + the methods the host offers.
  const info = await host.getInfo();
  console.log(`host ${info.name} v${info.version}, protocol ${info.protocol}`);
  if (info.protocol !== 1) throw new Error(`unsupported protocol ${info.protocol}`);

  // 2. Discover accounts on the inserted YubiKey.
  const { accounts } = await host.listAccounts();
  if (accounts.length === 0) throw new Error("no YubiKey accounts found — is the card inserted?");
  for (const a of accounts) console.log(`  ${a.id}  ${a.family}  ${a.address}`);

  // 3a. Ed25519 (Solana/Sui): sign a full serialized message (hex).
  const solana = accounts.find((a) => a.curve === "ed25519");
  if (solana) {
    const message = "00".repeat(32); // ← your serialized tx message, hex
    const { signature } = await host.signEd25519(solana.id, message);
    console.log(`ed25519 sig: ${signature}`);
    // Assemble the chain tx in the extension (host signs bytes only).
  }

  // 3b. secp256k1 (EVM): sign a 32-byte tx hash; get back EIP-155 r/s + recid.
  const evm = accounts.find((a) => a.curve === "secp256k1");
  if (evm) {
    const prehash = "11".repeat(32); // ← keccak256 of your RLP-encoded tx, hex
    const { r, s, recovery_id } = await host.signSecp256k1(evm.id, prehash);
    console.log(`evm sig: r=${r} s=${s} recid=${recovery_id}`);
    // v = recovery_id + 27, or chainId*2 + 35 + recovery_id for EIP-155.
  }

  host.disconnect();
}
