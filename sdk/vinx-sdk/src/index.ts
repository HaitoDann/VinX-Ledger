/**
 * VinX Ledger TypeScript SDK
 * Typed client for the VinX node RPC API.
 */

// ─── Response types ──────────────────────────────────────────────────────────

export interface HealthResponse {
  status: string;
  height: number;
  mempool_pending: number;
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
  frozen: boolean;
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
    activation_height: number;
    announced_at: number;
  } | null;
}

export interface NetworkStatsResponse {
  base_fee_atoms: string;
  staking_pool: string;
  melt_pool: string;
  circulating_supply: string;
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

// ─── Transaction payload ──────────────────────────────────────────────────────

export interface SignedTransaction {
  tx_type: string;
  from: string;
  to: string;
  amount: string;
  fee: string;
  nonce: number;
  pub_key?: string;
  signature?: string;
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
}

export default VinxClient;
