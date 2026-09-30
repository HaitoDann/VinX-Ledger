import { blake3 } from "@noble/hashes/blake3";
import { bech32m } from "@scure/base";
/**
 * VinX Ledger TypeScript SDK
 * Typed client for the VinX node RPC API.
 */

// ─── Response types ──────────────────────────────────────────────────────────

export interface HealthResponse {
  status: string;
  height: number;
  /** Highest final (quorum-signed) height — irreversible (ADR 0002). */
  finalized_height: number;
  mempool_pending: number;
  /** Chain ID this node validates — sign transactions with it. */
  chain_id: number;
}

export interface HeightResponse {
  height: number;
}

export interface AccountResponse {
  address: string;
  balance: string;
  balance_atoms: string;
  nonce: number;
  staked: string;
  staked_atoms: string;
  /** The account refuses transfers without a memo (exchange deposit address). */
  memo_required?: boolean;
}

export interface TxResponse {
  tx_type: string;
  from: string;
  to: string;
  amount: string;
  amount_atoms: string;
  fee: string;
  fee_atoms: string;
  nonce: number;
  hash: string;
  /** Transfer memo (public, max 32 bytes) as UTF-8 text, when valid. */
  memo?: string;
  memo_hex?: string;
}

export interface TxWithBlockResponse extends TxResponse {
  block_height: number;
  block_hash: string;
}

export interface BlockResponse {
  height: number;
  hash: string;
  prev_hash: string;
  timestamp: number;
  validator: string;
  tx_count: number;
  state_root: string;
  base_fee: number;
  signatures_count: number;
  finalized: boolean;
  transactions: TxResponse[];
}

export interface TxSubmitResponse {
  accepted: boolean;
  tx_hash: string;
}

export interface ValidatorSetResponse {
  count: number;
  quorum: number;
  validators: string[];
}

export interface ProtocolStatusResponse {
  current_version: string;
  pending_upgrade: {
    version: string;
    /** Unix timestamp (seconds) at which the upgrade activates (ADR 0006). */
    activation_ts: number;
    announced_at: number;
  } | null;
}

export interface NetworkStatsResponse {
  base_fee_atoms: string;
  /** La Fonderie reserve (melt/forge). circulating_supply + foundry == MAX_SUPPLY. */
  foundry: string;
  circulating_supply: string;
  /** On-chain admin address (bech32), or null if none is configured. */
  admin_address: string | null;
}

export interface MempoolResponse {
  pending: number;
}

export interface AccountTxsResponse {
  address: string;
  total: number;
  offset: number;
  txs: TxWithBlockResponse[];
}

export interface ChainSyncResponse {
  from: number;
  count: number;
  blocks: BlockResponse[];
}

/**
 * Proof of an account against the `state_root` of block `height` (ADR 0083).
 * Verify it client-side with {@link verifyAccountProof}.
 */
export interface MerkleProofResponse {
  address: string;
  height: number;
  state_root: string;
  accounts_root: string;
  consensus_root: string;
  key: string;
  /** hash of the account leaf, or null when the account does not exist */
  leaf_value: string | null;
  /** leaf reached by the key's path: [key, value], or null (empty subtree) */
  proof_leaf: [string, string] | null;
  /** sibling hashes, root first */
  siblings: string[];
  /** server-side verification result — do not trust it, use verifyAccountProof */
  valid: boolean;
}

/** Block header fields needed to recompute the block hash. */
export interface HeaderJson {
  height: number;
  round: number;
  prev_hash: string;
  timestamp: number;
  validator: string;
  tx_count: number;
  state_root: string;
  base_fee: number;
  receipts_root: string;
  last_commit_hash: string;
}

/**
 * Payment receipt (ADR 0083): proof that a transaction is in a committed block.
 * Keep it — it stays verifiable after nodes prune the block (30-day window).
 */
export interface PaymentReceipt {
  tx_hash: string;
  index: number;
  /** sibling hashes, bottom-up */
  siblings: string[];
  header: HeaderJson;
  block_hash: string;
  /** quorum certificate of the block (BLS aggregate + signer bitmap) */
  commit: unknown;
  tx: unknown;
}

export interface FaucetResponse {
  accepted: boolean;
  tx_hash: string;
  amount_atoms: string;
  to: string;
}

// ─── Transaction payload ──────────────────────────────────────────────────────

/**
 * Signed transaction as accepted by `POST /tx/submit`.
 *
 * There is no `from` field (ADR 0081 D6): the sender address is derived by the node
 * from `pub_key`. The signature covers the canonical signing bytes:
 * `disc(1) ‖ key_type(1, 0 = Ed25519) ‖ pub_key(32) ‖ to(20) ‖ amount(16 BE) ‖ fee(16 BE)
 *  ‖ nonce(8 BE) ‖ chain_id(4 BE) ‖ expiry(0 | 1‖8 BE) ‖ payload_len(4 BE) ‖ payload
 *  ‖ sponsor(0 | 1‖20)`. Amounts are in atoms (1 VINX = 10^9 atoms).
 */
export interface SignedTransaction {
  tx_type: string;
  to: string;
  amount: string | number;
  fee: string | number;
  nonce: number;
  chain_id: number;
  /** For a Transfer: the memo, at most MAX_MEMO_BYTES bytes (public). */
  payload: number[];
  /** Ed25519 public key, 32 bytes. */
  pub_key: number[];
  /** Ed25519 signature, 64 bytes, lowercase hex. */
  signature: string;
}

// ─── Client ───────────────────────────────────────────────────────────────────

export class VinxClient {
  private readonly baseUrl: string;

  constructor(nodeUrl: string = "http://localhost:8080") {
    this.baseUrl = nodeUrl.replace(/\/$/, "");
  }

  private async get<T>(path: string): Promise<T> {
    const res = await fetch(`${this.baseUrl}${path}`);
    if (!res.ok) {
      const err = await res.json().catch(() => ({ error: res.statusText }));
      throw new Error(`VinX API error ${res.status}: ${(err as any).error}`);
    }
    return res.json() as Promise<T>;
  }

  private async post<T>(path: string, body: unknown): Promise<T> {
    const res = await fetch(`${this.baseUrl}${path}`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    if (!res.ok) {
      const err = await res.json().catch(() => ({ error: res.statusText }));
      throw new Error(`VinX API error ${res.status}: ${(err as any).error}`);
    }
    return res.json() as Promise<T>;
  }

  /** Check node health and current chain height. */
  health(): Promise<HealthResponse> {
    return this.get("/health");
  }

  /** Get the current chain tip height. */
  height(): Promise<HeightResponse> {
    return this.get("/chain/height");
  }

  /** Get account details for a bech32 address. */
  account(address: string): Promise<AccountResponse> {
    return this.get(`/account/${address}`);
  }

  /** Get transaction history for an address (newest-first). */
  accountTxs(
    address: string,
    opts: { limit?: number; offset?: number } = {}
  ): Promise<AccountTxsResponse> {
    const params = new URLSearchParams();
    if (opts.limit !== undefined) params.set("limit", String(opts.limit));
    if (opts.offset !== undefined) params.set("offset", String(opts.offset));
    const qs = params.toString() ? `?${params}` : "";
    return this.get(`/account/${address}/txs${qs}`);
  }

  /** Get a block by height. */
  block(height: number): Promise<BlockResponse> {
    return this.get(`/block/${height}`);
  }

  /** Get a transaction by hex hash. */
  tx(hash: string): Promise<TxWithBlockResponse> {
    return this.get(`/tx/${hash}`);
  }

  /** Submit a signed transaction to the mempool. */
  submitTx(tx: SignedTransaction): Promise<TxSubmitResponse> {
    return this.post("/tx/submit", tx);
  }

  /** Get the current mempool pending count. */
  mempool(): Promise<MempoolResponse> {
    return this.get("/mempool/size");
  }

  /** Get the active validator set. */
  validators(): Promise<ValidatorSetResponse> {
    return this.get("/validators");
  }

  /** Get current protocol version and pending upgrade (if any). */
  protocolStatus(): Promise<ProtocolStatusResponse> {
    return this.get("/protocol/version");
  }

  /** Get economic network stats: base_fee, staking pool, melt pool. */
  networkStats(): Promise<NetworkStatsResponse> {
    return this.get("/network/stats");
  }

  /** Sync a range of blocks from the node (useful for light clients). */
  chainSync(from: number, limit = 100): Promise<ChainSyncResponse> {
    return this.get(`/chain/sync?from=${from}&limit=${limit}`);
  }

  /**
   * Subscribe to real-time block events via Server-Sent Events.
   * Returns an EventSource — call `.close()` to unsubscribe.
   *
   * @example
   * const es = client.blockEvents();
   * es.onmessage = (e) => console.log(JSON.parse(e.data));
   */
  blockEvents(): EventSource {
    return new EventSource(`${this.baseUrl}/events`);
  }

  /**
   * Fetch the Merkle inclusion proof for an account in the current state root.
   * Use `verifyAccountProof` to verify the proof client-side.
   */
  accountProof(address: string): Promise<MerkleProofResponse> {
    return this.get(`/account/${address}/proof`);
  }

  /**
   * Request testnet tokens from the faucet.
   * The node must have a faucet keypair configured.
   */
  /** Payment receipt with its inclusion proof — verify with verifyPaymentReceipt. */
  paymentReceipt(txHash: string): Promise<PaymentReceipt> {
    return this.get(`/tx/${txHash}/proof`);
  }

  faucetRequest(address: string): Promise<FaucetResponse> {
    return this.post("/faucet/request", { address });
  }
}

// ─── Light client — Merkle proof verification ─────────────────────────────────

const enc = new TextEncoder();

function concat(...parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let o = 0;
  for (const p of parts) {
    out.set(p, o);
    o += p.length;
  }
  return out;
}

function beBytes(value: bigint, len: number): Uint8Array {
  const out = new Uint8Array(len);
  for (let i = len - 1; i >= 0; i--) {
    out[i] = Number(value & 0xffn);
    value >>= 8n;
  }
  return out;
}

function hexToBytes(hex: string): Uint8Array {
  if (!/^[0-9a-fA-F]*$/.test(hex) || hex.length % 2) throw new Error("invalid hex");
  return Uint8Array.from(hex.match(/../g) ?? [], (h) => parseInt(h, 16));
}

const eq = (a: Uint8Array, b: Uint8Array) =>
  a.length === b.length && a.every((x, i) => x === b[i]);

const bit = (key: Uint8Array, d: number) => ((key[d >> 3] >> (7 - (d & 7))) & 1) === 1;

/** Raw 20-byte payload of a `vinx1…` Bech32m address. */
export function addressBytes(address: string): Uint8Array {
  const { prefix, words } = bech32m.decode(address as `${string}1${string}`);
  if (prefix !== "vinx") throw new Error("not a vinx address");
  return bech32m.fromWords(words);
}

/** Tree key of an account (`account_key` in vinx-state). */
export function accountKey(address: string): Uint8Array {
  return blake3(concat(enc.encode("VINX_ACCOUNT_KEY"), addressBytes(address)));
}

/** Leaf value of an account (`hash_account` in vinx-state). */
export function hashAccount(account: AccountResponse): Uint8Array {
  return blake3(
    concat(
      addressBytes(account.address),
      beBytes(BigInt(account.balance_atoms), 16),
      beBytes(BigInt(account.nonce), 8),
      beBytes(BigInt(account.staked_atoms), 16)
    )
  );
}

/**
 * Verifies an account proof without trusting the server (ADR 0083).
 *
 * `trustedStateRoot` must come from a block header you trust (it is committed by the
 * quorum certificate of the next block). `account` is the account state to prove, or
 * `null` to prove the address has no account.
 */
export function verifyAccountProof(
  proof: MerkleProofResponse,
  trustedStateRoot: string,
  account: AccountResponse | null
): boolean {
  try {
    if (account && account.address !== proof.address) return false;
    const key = accountKey(proof.address);
    const expected = account ? hashAccount(account) : null;
    const siblings = proof.siblings.map(hexToBytes);
    if (siblings.length > 256) return false;
    const leafHash = (k: Uint8Array, v: Uint8Array) =>
      blake3(concat(enc.encode("VINX_SMT_LEAF"), k, v));
    let current: Uint8Array;
    if (proof.proof_leaf) {
      const k = hexToBytes(proof.proof_leaf[0]);
      const v = hexToBytes(proof.proof_leaf[1]);
      if (expected) {
        if (!eq(k, key) || !eq(v, expected)) return false;
      } else {
        if (eq(k, key)) return false;
        for (let d = 0; d < siblings.length; d++) if (bit(k, d) !== bit(key, d)) return false;
      }
      current = leafHash(k, v);
    } else {
      if (expected) return false;
      current = new Uint8Array(32);
    }
    const node = enc.encode("VINX_SMT_NODE");
    for (let d = siblings.length - 1; d >= 0; d--) {
      current = bit(key, d)
        ? blake3(concat(node, siblings[d], current))
        : blake3(concat(node, current, siblings[d]));
    }
    if (!eq(current, hexToBytes(proof.accounts_root))) return false;
    const stateRoot = blake3(
      concat(
        enc.encode("VINX:state_root:v2"),
        hexToBytes(proof.accounts_root),
        hexToBytes(proof.consensus_root)
      )
    );
    return eq(stateRoot, hexToBytes(trustedStateRoot));
  } catch {
    return false;
  }
}

/** Block hash of a header (`BlockHeader::hash` in vinx-core). */
export function headerHash(h: HeaderJson): string {
  const bytes = concat(
    beBytes(BigInt(h.height), 8),
    beBytes(BigInt(h.round), 4),
    hexToBytes(h.prev_hash),
    beBytes(BigInt(h.timestamp), 8),
    addressBytes(h.validator),
    beBytes(BigInt(h.tx_count), 4),
    hexToBytes(h.state_root),
    beBytes(BigInt(h.base_fee), 8),
    hexToBytes(h.receipts_root),
    hexToBytes(h.last_commit_hash)
  );
  return Array.from(blake3(bytes), (b) => b.toString(16).padStart(2, "0")).join("");
}

/**
 * Verifies a payment receipt for `txHash`: the transaction is entry `index` of the
 * block's transaction tree, and the header hashes to `block_hash`.
 *
 * This proves inclusion in *that block*. That the block is committed rests on
 * `receipt.commit` (a BLS quorum certificate) or on `block_hash` matching a block
 * returned by a node you trust; the SDK does not verify BLS signatures.
 */
export function verifyPaymentReceipt(receipt: PaymentReceipt, txHash: string): boolean {
  try {
    const tx = hexToBytes(txHash.toLowerCase());
    if (receipt.tx_hash.toLowerCase() !== txHash.toLowerCase()) return false;
    const count = receipt.header.tx_count;
    let idx = receipt.index;
    if (!Number.isInteger(idx) || idx < 0 || idx >= count) return false;
    const sibs = receipt.siblings.map(hexToBytes);
    let cur = blake3(concat(enc.encode("VINX_TX_LEAF"), tx));
    let len = count;
    let used = 0;
    const node = enc.encode("VINX_TX_NODE");
    while (len > 1) {
      if ((idx ^ 1) < len) {
        const sib = sibs[used++];
        if (!sib) return false;
        cur = idx % 2 === 0 ? blake3(concat(node, cur, sib)) : blake3(concat(node, sib, cur));
      }
      idx = Math.floor(idx / 2);
      len = Math.ceil(len / 2);
    }
    if (used !== sibs.length || !eq(cur, hexToBytes(receipt.header.receipts_root))) return false;
    return headerHash(receipt.header) === receipt.block_hash.toLowerCase();
  } catch {
    return false;
  }
}

/** Maximum memo of a transfer, in bytes (ADR 0085). */
export const MAX_MEMO_BYTES = 32;

/** Memo bytes for a transfer payload; throws if longer than MAX_MEMO_BYTES. */
export function encodeMemo(memo: string): number[] {
  const bytes = Array.from(enc.encode(memo));
  if (bytes.length > MAX_MEMO_BYTES) throw new Error(`memo exceeds ${MAX_MEMO_BYTES} bytes`);
  return bytes;
}

/** Splits a payment address `name@domain`; null for anything else (ADR 0085). */
export function parsePaymentAddress(handle: string): { name: string; domain: string } | null {
  const m = /^([a-z0-9._-]{1,64})@([a-zA-Z0-9-]+(\.[a-zA-Z0-9-]+)*(:[0-9]+)?)$/.exec(handle);
  return m ? { name: m[1], domain: m[2].toLowerCase() } : null;
}

/**
 * Resolves `name@domain` to a `vinx1…` address, off-chain (ADR 0085): fetches
 * `https://<domain>/.well-known/vinx.json?name=<name>`, which answers
 * `{"names": {"<name>": "vinx1…"}}`. Show the resolved address to the user and pay it.
 */
export async function resolvePaymentAddress(
  handle: string,
  fetchFn: typeof fetch = fetch
): Promise<string> {
  const p = parsePaymentAddress(handle);
  if (!p) throw new Error("not a name@domain payment address");
  const local = p.domain.startsWith("localhost") || p.domain.startsWith("127.0.0.1");
  const url = `${local ? "http" : "https"}://${p.domain}/.well-known/vinx.json?name=${p.name}`;
  const res = await fetchFn(url, { redirect: "error" });
  if (!res.ok) throw new Error(`${p.domain}: HTTP ${res.status}`);
  const doc = (await res.json()) as { names?: Record<string, string> };
  const addr = doc.names?.[p.name];
  if (typeof addr !== "string") throw new Error(`${handle} is not known by ${p.domain}`);
  addressBytes(addr); // throws on anything that is not a valid vinx1… address
  return addr;
}

export default VinxClient;
