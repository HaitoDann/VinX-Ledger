import { VinxClient, verifyMerkleProof, MerkleProofStep } from './index';

const BASE = 'http://localhost:8080';

function mockFetch(data: unknown, status = 200): jest.SpyInstance {
  return jest.spyOn(globalThis, 'fetch').mockResolvedValueOnce({
    ok: status >= 200 && status < 300,
    status,
    statusText: 'OK',
    json: async () => data,
  } as Response);
}

afterEach(() => jest.restoreAllMocks());

describe('VinxClient constructor', () => {
  it('strips trailing slash', () => {
    const c = new VinxClient('http://localhost:8080/');
    // access private via cast
    expect((c as any).baseUrl).toBe('http://localhost:8080');
  });
  it('defaults to localhost:8080', () => {
    const c = new VinxClient();
    expect((c as any).baseUrl).toBe('http://localhost:8080');
  });
});

describe('VinxClient.health()', () => {
  it('calls GET /health and returns response', async () => {
    const payload = { status: 'ok', height: 42, mempool_pending: 3, chain_id: 42 };
    const spy = mockFetch(payload);
    const client = new VinxClient(BASE);
    const result = await client.health();
    expect(spy).toHaveBeenCalledWith(`${BASE}/health`);
    expect(result).toEqual(payload);
  });
});

describe('VinxClient.height()', () => {
  it('calls GET /chain/height', async () => {
    const payload = { height: 100 };
    const spy = mockFetch(payload);
    const client = new VinxClient(BASE);
    const result = await client.height();
    expect(spy).toHaveBeenCalledWith(`${BASE}/chain/height`);
    expect(result.height).toBe(100);
  });
});

describe('VinxClient.account()', () => {
  it('calls GET /account/:address', async () => {
    const addr = 'vinx1abc';
    const payload = { address: addr, balance: '1000', balance_atoms: '1000000000000000000000', nonce: 0, staked: '0', staked_atoms: '0' };
    const spy = mockFetch(payload);
    const client = new VinxClient(BASE);
    const result = await client.account(addr);
    expect(spy).toHaveBeenCalledWith(`${BASE}/account/${addr}`);
    expect(result.address).toBe(addr);
  });
});

describe('VinxClient.accountTxs()', () => {
  it('calls GET /account/:address/txs with no params', async () => {
    const addr = 'vinx1abc';
    const payload = { address: addr, total: 0, offset: 0, txs: [] };
    const spy = mockFetch(payload);
    const client = new VinxClient(BASE);
    await client.accountTxs(addr);
    expect(spy).toHaveBeenCalledWith(`${BASE}/account/${addr}/txs`);
  });

  it('appends limit and offset query params', async () => {
    const addr = 'vinx1abc';
    mockFetch({ address: addr, total: 0, offset: 10, txs: [] });
    const client = new VinxClient(BASE);
    const spy = jest.spyOn(client as any, 'get');
    await client.accountTxs(addr, { limit: 5, offset: 10 });
    expect(spy).toHaveBeenCalledWith(`/account/${addr}/txs?limit=5&offset=10`);
  });
});

describe('VinxClient.block()', () => {
  it('calls GET /block/:height', async () => {
    const payload = { height: 1, hash: 'abc', prev_hash: '000', timestamp: 1000, validator: 'vinx1v', tx_count: 0, state_root: 'sr', base_fee: 100, signatures_count: 1, finalized: true, transactions: [] };
    const spy = mockFetch(payload);
    const client = new VinxClient(BASE);
    const result = await client.block(1);
    expect(spy).toHaveBeenCalledWith(`${BASE}/block/1`);
    expect(result.height).toBe(1);
  });
});

describe('VinxClient.tx()', () => {
  it('calls GET /tx/:hash', async () => {
    const hash = 'deadbeef';
    const payload = { tx_type: 'Transfer', from: 'vinx1a', to: 'vinx1b', amount: '10', amount_atoms: '10000', fee: '0.001', fee_atoms: '100', nonce: 0, hash, block_height: 5, block_hash: 'abc' };
    const spy = mockFetch(payload);
    const client = new VinxClient(BASE);
    const result = await client.tx(hash);
    expect(spy).toHaveBeenCalledWith(`${BASE}/tx/${hash}`);
    expect(result.hash).toBe(hash);
  });
});

describe('VinxClient.submitTx()', () => {
  it('calls POST /tx/submit', async () => {
    const payload = { accepted: true, tx_hash: 'abc123' };
    const spy = mockFetch(payload);
    const client = new VinxClient(BASE);
    const tx = { tx_type: 'Transfer', from: 'vinx1a', to: 'vinx1b', amount: '10', fee: '0.001', nonce: 0 };
    const result = await client.submitTx(tx);
    expect(spy).toHaveBeenCalledWith(
      `${BASE}/tx/submit`,
      expect.objectContaining({ method: 'POST', body: JSON.stringify(tx) })
    );
    expect(result.accepted).toBe(true);
  });
});

describe('VinxClient.mempool()', () => {
  it('calls GET /mempool/size', async () => {
    const spy = mockFetch({ pending: 7 });
    const client = new VinxClient(BASE);
    const result = await client.mempool();
    expect(spy).toHaveBeenCalledWith(`${BASE}/mempool/size`);
    expect(result.pending).toBe(7);
  });
});

describe('VinxClient.validators()', () => {
  it('calls GET /validators', async () => {
    const payload = { count: 1, quorum: 1, validators: ['vinx1v'] };
    const spy = mockFetch(payload);
    const client = new VinxClient(BASE);
    const result = await client.validators();
    expect(spy).toHaveBeenCalledWith(`${BASE}/validators`);
    expect(result.count).toBe(1);
  });
});

describe('VinxClient.protocolStatus()', () => {
  it('calls GET /protocol/version', async () => {
    const payload = { current_version: '1.0.0', pending_upgrade: null };
    const spy = mockFetch(payload);
    const client = new VinxClient(BASE);
    const result = await client.protocolStatus();
    expect(spy).toHaveBeenCalledWith(`${BASE}/protocol/version`);
    expect(result.current_version).toBe('1.0.0');
    expect(result.pending_upgrade).toBeNull();
  });
});

describe('VinxClient.networkStats()', () => {
  it('calls GET /network/stats', async () => {
    const payload = { base_fee_atoms: '1000000000000000', foundry: '99000000000.00', circulating_supply: '1000000000.00' };
    const spy = mockFetch(payload);
    const client = new VinxClient(BASE);
    const result = await client.networkStats();
    expect(spy).toHaveBeenCalledWith(`${BASE}/network/stats`);
    expect(result.circulating_supply).toBeDefined();
  });
});

describe('VinxClient.chainSync()', () => {
  it('calls GET /chain/sync with params', async () => {
    const payload = { from: 0, count: 1, blocks: [] };
    const spy = mockFetch(payload);
    const client = new VinxClient(BASE);
    const result = await client.chainSync(0, 50);
    expect(spy).toHaveBeenCalledWith(`${BASE}/chain/sync?from=0&limit=50`);
    expect(result.from).toBe(0);
  });
});

describe('VinxClient.accountProof()', () => {
  it('calls GET /account/:address/proof', async () => {
    const addr = 'vinx1abc';
    const payload = {
      address: addr,
      leaf_hash: 'aa'.repeat(32),
      state_root: 'bb'.repeat(32),
      proof: [{ sibling: 'cc'.repeat(32), sibling_is_right: true }],
      valid: true,
    };
    const spy = mockFetch(payload);
    const client = new VinxClient(BASE);
    const result = await client.accountProof(addr);
    expect(spy).toHaveBeenCalledWith(`${BASE}/account/${addr}/proof`);
    expect(result.address).toBe(addr);
    expect(result.proof).toHaveLength(1);
    expect(result.valid).toBe(true);
  });
});

describe('VinxClient.faucetRequest()', () => {
  it('calls POST /faucet/request with address', async () => {
    const addr = 'vinx1abc';
    const payload = { accepted: true, tx_hash: 'deadbeef', amount_atoms: '100000000000000000000', to: addr };
    const spy = mockFetch(payload);
    const client = new VinxClient(BASE);
    const result = await client.faucetRequest(addr);
    expect(spy).toHaveBeenCalledWith(
      `${BASE}/faucet/request`,
      expect.objectContaining({ method: 'POST', body: JSON.stringify({ address: addr }) })
    );
    expect(result.accepted).toBe(true);
    expect(result.to).toBe(addr);
  });
});

describe('verifyMerkleProof()', () => {
  it('returns true for a single-leaf tree (empty proof)', async () => {
    // Single leaf: root == leaf itself (no hashing needed; proof is empty)
    const leaf = 'ab'.repeat(32);
    const valid = await verifyMerkleProof(leaf, [], leaf);
    expect(valid).toBe(true);
  });

  it('returns false when leaf does not match root', async () => {
    const leaf = 'aa'.repeat(32);
    const root = 'bb'.repeat(32);
    const valid = await verifyMerkleProof(leaf, [], root);
    expect(valid).toBe(false);
  });

  it('correctly reconstructs root with a single proof step', async () => {
    // Compute SHA-256(leaf || sibling) as the expected root
    const leafHex = '01'.repeat(32);
    const siblingHex = '02'.repeat(32);
    // Compute expected root using crypto.subtle
    const leafBytes = Uint8Array.from({ length: 32 }, () => 0x01);
    const siblingBytes = Uint8Array.from({ length: 32 }, () => 0x02);
    const combined = new Uint8Array(64);
    combined.set(leafBytes);
    combined.set(siblingBytes, 32);
    const hashBuf = await globalThis.crypto.subtle.digest('SHA-256', combined);
    const expectedRoot = Array.from(new Uint8Array(hashBuf))
      .map(b => b.toString(16).padStart(2, '0'))
      .join('');

    const proof: MerkleProofStep[] = [{ sibling: siblingHex, sibling_is_right: true }];
    const valid = await verifyMerkleProof(leafHex, proof, expectedRoot);
    expect(valid).toBe(true);
  });

  it('returns false with tampered sibling', async () => {
    const leafHex = '01'.repeat(32);
    const siblingHex = '02'.repeat(32);
    const tamperedSibling = '03'.repeat(32);
    // Compute root using the real sibling
    const leafBytes = Uint8Array.from({ length: 32 }, () => 0x01);
    const siblingBytes = Uint8Array.from({ length: 32 }, () => 0x02);
    const combined = new Uint8Array(64);
    combined.set(leafBytes);
    combined.set(siblingBytes, 32);
    const hashBuf = await globalThis.crypto.subtle.digest('SHA-256', combined);
    const expectedRoot = Array.from(new Uint8Array(hashBuf))
      .map(b => b.toString(16).padStart(2, '0'))
      .join('');

    // Use the tampered sibling — should produce a different root
    const proof: MerkleProofStep[] = [{ sibling: tamperedSibling, sibling_is_right: true }];
    const valid = await verifyMerkleProof(leafHex, proof, expectedRoot);
    expect(valid).toBe(false);
  });
});

describe('VinxClient error handling', () => {
  it('throws on non-ok response', async () => {
    jest.spyOn(globalThis, 'fetch').mockResolvedValueOnce({
      ok: false,
      status: 404,
      statusText: 'Not Found',
      json: async () => ({ error: 'not found' }),
    } as Response);
    const client = new VinxClient(BASE);
    await expect(client.health()).rejects.toThrow('VinX API error 404: not found');
  });

  it('falls back to statusText when error body has no .error field', async () => {
    jest.spyOn(globalThis, 'fetch').mockResolvedValueOnce({
      ok: false,
      status: 500,
      statusText: 'Internal Server Error',
      json: async () => ({}),
    } as Response);
    const client = new VinxClient(BASE);
    await expect(client.height()).rejects.toThrow('VinX API error 500');
  });
});
