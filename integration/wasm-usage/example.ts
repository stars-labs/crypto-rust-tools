/**
 * Using the yubiwallet-wasm npm package for chain-independent logic.
 *
 * The WASM module CANNOT sign — it can't reach the card. It provides the pure
 * helpers a wallet UI needs around the native host: derive display addresses
 * from a public key, and turn the card's raw `R||S` into an EIP-155 `(r,s,v)`.
 *
 * Build first:  integration/wasm-usage/build.sh web
 * Then import from the generated package (here via a relative path; in your app
 * you'd `npm install` it or point at yubiwallet-wasm/pkg).
 */
import init, {
  solana_address,
  eth_address,
  eth_signature,
} from "../../yubiwallet-wasm/pkg/yubiwallet_wasm.js";

export async function demo() {
  await init(); // load the .wasm (web target)

  // 1. Derive display addresses from public keys returned by the host.
  const ed25519Pub = "00".repeat(32); // 32-byte ed25519 key, hex
  console.log("solana:", solana_address(ed25519Pub));

  const secpPub = "04" + "11".repeat(64); // 65-byte uncompressed key, hex
  console.log("evm:", eth_address(secpPub));

  // 2. Convert the host's secp256k1 output into a full EIP-155 signature.
  //    (The host already returns r/s/recovery_id; use this when you instead
  //     have raw R||S and want the recid computed for a given chain.)
  const hash = "22".repeat(32); // 32-byte tx hash, hex
  const rs = "33".repeat(64); // raw R||S from the card, hex
  const chainId = 1n; // Ethereum mainnet
  const sig = eth_signature(secpPub, hash, rs, chainId);
  console.log(`r=${sig.r} s=${sig.s} v=${sig.v}`);
}
