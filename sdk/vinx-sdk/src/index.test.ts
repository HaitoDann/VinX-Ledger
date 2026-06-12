import { VinxClient } from './index';

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
    const payload = { status: 'ok', height: 42, mempool_pending: 3 };
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
    const payload = { address: addr, balance: '1000', balance_atoms: '1000000000000000000000', nonce: 0, staked: '0', staked_atoms: '0', frozen: false };
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
    const payload = { base_fee_atoms: '1000000000000000', staking_pool: '0.00', melt_pool: '0.00', circulating_supply: '21000000.00' };
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

describe('VinxClient.proposals()', () => {
  it('calls GET /governance/proposals', async () => {
    const payload = { proposals: [] };
    const spy = mockFetch(payload);
    const client = new VinxClient(BASE);
    const result = await client.proposals();
    expect(spy).toHaveBeenCalledWith(`${BASE}/governance/proposals`);
    expect(result.proposals).toEqual([]);
  });
});

describe('VinxClient.proposal()', () => {
  it('calls GET /governance/proposal/:id', async () => {
    const proposal = {
      id: 0,
      proposer: 'vinx1a',
      description: 'Add validator',
      action: { AddValidator: 'vinx1new' },
      submitted_at: 10,
      voting_ends_at: 100,
      yes_votes: [],
      no_votes: [],
      status: 'Active' as const,
    };
    const spy = mockFetch(proposal);
    const client = new VinxClient(BASE);
    const result = await client.proposal(0);
    expect(spy).toHaveBeenCalledWith(`${BASE}/governance/proposal/0`);
    expect(result.id).toBe(0);
    expect(result.status).toBe('Active');
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
