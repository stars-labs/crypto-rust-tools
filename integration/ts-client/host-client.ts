/**
 * Minimal typed client for the YubiWallet native-messaging host.
 *
 * Drop this into a browser extension's background/service worker. It wraps the
 * long-lived `chrome.runtime.connectNative` port with a request/response API
 * keyed by an incrementing message id.
 *
 *   const host = new HostClient();
 *   const { accounts } = await host.listAccounts();
 *   const sig = await host.signEd25519(accounts[0].id, "deadbeef");
 *
 * The host collects the YubiKey PIN itself (env or pinentry) — never send a PIN
 * through this channel.
 */
import type {
  Account,
  GetInfoResult,
  GetStatusResult,
  ListAccountsResult,
  Method,
  Request,
  Response,
  SignEd25519Result,
  SignSecp256k1Result,
} from "./types";

const HOST_NAME = "com.yubiwallet.host";

type Pending = {
  resolve: (value: unknown) => void;
  reject: (reason: Error) => void;
};

export class HostClient {
  private port: chrome.runtime.Port;
  private nextId = 1;
  private pending = new Map<number, Pending>();

  constructor(hostName: string = HOST_NAME) {
    this.port = chrome.runtime.connectNative(hostName);
    this.port.onMessage.addListener((msg) => this.onMessage(msg as Response));
    this.port.onDisconnect.addListener(() => this.onDisconnect());
  }

  private onMessage(resp: Response) {
    const p = this.pending.get(resp.id);
    if (!p) return;
    this.pending.delete(resp.id);
    if (resp.error) p.reject(new Error(`${resp.error.code}: ${resp.error.message}`));
    else p.resolve(resp.result);
  }

  private onDisconnect() {
    const err = new Error(
      chrome.runtime.lastError?.message ?? "native host disconnected",
    );
    for (const p of this.pending.values()) p.reject(err);
    this.pending.clear();
  }

  private call<R>(method: Method, params?: unknown): Promise<R> {
    const id = this.nextId++;
    const req: Request = { id, method, params };
    return new Promise<R>((resolve, reject) => {
      this.pending.set(id, { resolve: resolve as (v: unknown) => void, reject });
      this.port.postMessage(req);
    });
  }

  getInfo(): Promise<GetInfoResult> {
    return this.call("get_info");
  }

  getStatus(): Promise<GetStatusResult> {
    return this.call("get_status");
  }

  listAccounts(): Promise<ListAccountsResult> {
    return this.call("list_accounts");
  }

  /** Sign a 32-byte transaction hash (hex) with an EVM account. */
  signSecp256k1(accountId: Account["id"], prehash: string): Promise<SignSecp256k1Result> {
    return this.call("sign_secp256k1", { account_id: accountId, prehash });
  }

  /** Sign a full message (hex) with an Ed25519 account (Solana/Sui). */
  signEd25519(accountId: Account["id"], message: string): Promise<SignEd25519Result> {
    return this.call("sign_ed25519", { account_id: accountId, message });
  }

  disconnect() {
    this.port.disconnect();
  }
}
