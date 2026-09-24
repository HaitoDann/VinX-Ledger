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

export interface MerkleProofStep {
  sibling: string;        // hex-encoded SHA-256 hash of the sibling node
  sibling_is_right: boolean; // true = sibling is the right child, current is left
}

export interface MerkleProofResponse {
  address: string;
  leaf_hash: string;   // hex SHA-256 of the account leaf
  state_root: string;  // hex SHA-256 of the Merkle state root
  proof: MerkleProofStep[];
  valid: boolean;      // server-side verification result (use verifyMerkleProof for client-side)
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
   * Use `verifyMerkleProof` to verify the proof client-side.
   */
  accountProof(address: string): Promise<MerkleProofResponse> {
    return this.get(`/account/${address}/proof`);
  }

  /**
   * Request testnet tokens from the faucet.
   * The node must have a faucet keypair configured.
   */
  faucetRequest(address: string): Promise<FaucetResponse> {
    return this.post("/faucet/request", { address });
  }
}

// ─── Light client — Merkle proof verification ─────────────────────────────────

function hexToBytes(hex: string): Uint8Array {
  const bytes = new Uint8Array(hex.length / 2);
  for (let i = 0; i < hex.length; i += 2) {
    bytes[i / 2] = parseInt(hex.slice(i, i + 2), 16);
  }
  return bytes;
}

function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes)
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

async function sha256Concat(leftHex: string, rightHex: string): Promise<string> {
  const left = hexToBytes(leftHex);
  const right = hexToBytes(rightHex);
  const combined = new Uint8Array(left.length + right.length);
  combined.set(left);
  combined.set(right, left.length);
  const hashBuffer = await globalThis.crypto.subtle.digest("SHA-256", combined);
  return bytesToHex(new Uint8Array(hashBuffer));
}

/**
 * Verify a Merkle inclusion proof client-side without trusting the server.
 *
 * Reconstructs the Merkle root from the leaf hash and proof steps, then
 * compares it against the expected `stateRoot`. Returns `true` if the proof
 * is valid (leaf is included in the tree that produces `stateRoot`).
 *
 * All hash values must be lowercase hex strings (64 chars / 32 bytes).
 *
 * @example
 * const proof = await client.accountProof("vinx1abc...");
 * const valid = await verifyMerkleProof(proof.leaf_hash, proof.proof, proof.state_root);
 */
export async function verifyMerkleProof(
  leafHash: string,
  proof: MerkleProofStep[],
  stateRoot: string
): Promise<boolean> {
  let current = leafHash.toLowerCase();
  for (const step of proof) {
    const sibling = step.sibling.toLowerCase();
    if (step.sibling_is_right) {
      // current node is the left child
      current = await sha256Concat(current, sibling);
    } else {
      // sibling is the left child, current is right
      current = await sha256Concat(sibling, current);
    }
  }
  return current === stateRoot.toLowerCase();
}

export default VinxClient;
