/**
 * Wire types for the YubiWallet native-messaging protocol (protocol v1).
 *
 * A browser extension talks to the `yubiwallet-host` binary via
 * `chrome.runtime.connectNative("com.yubiwallet.host")`. The browser itself
 * frames each message (u32-le length + UTF-8 JSON), so on the JS side you just
 * `postMessage` / receive plain objects — no manual framing.
 */

/** An account discovered on the card. `id` is what you pass back to sign. */
export interface Account {
  /** `applet:slot:curve`, e.g. `piv:9a:ed25519` or `openpgp:sig:secp256k1`. */
  id: string;
  /** Chain family hint: `"solana"` (ed25519) or `"evm"` (secp256k1). */
  family: "solana" | "evm";
  curve: "ed25519" | "secp256k1";
  applet: "piv" | "openpgp";
  slot: string;
  /** Base58 (Solana) or `0x…` (EVM) address. */
  address: string;
  /** Hex-encoded public key (32B ed25519 / 65B uncompressed secp256k1). */
  pubkey: string;
}

export interface GetInfoResult {
  name: string;
  version: string;
  protocol: number;
  methods: string[];
}

export interface GetStatusResult {
  card_present: boolean;
  accounts: Account[];
}

export interface ListAccountsResult {
  accounts: Account[];
}

/** secp256k1 result: EIP-155 `r`/`s` (hex, 32B each) + 0/1 recovery id. */
export interface SignSecp256k1Result {
  r: string;
  s: string;
  recovery_id: number;
}

/** ed25519 result: 64-byte detached signature (hex). */
export interface SignEd25519Result {
  signature: string;
}

/** Request/response envelopes. `id` correlates responses to requests. */
export interface Request<P = unknown> {
  id: number;
  method: string;
  params?: P;
}

export interface ResponseError {
  code: string;
  message: string;
}

export interface Response<R = unknown> {
  id: number;
  result?: R;
  error?: ResponseError;
}

export type Method =
  | "get_info"
  | "get_status"
  | "list_accounts"
  | "sign_secp256k1"
  | "sign_ed25519";
