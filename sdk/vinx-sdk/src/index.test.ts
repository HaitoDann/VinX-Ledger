import { readFileSync } from 'fs';
import { join } from 'path';
import { VinxClient, resolvePaymentAddress, parsePaymentAddress, encodeMemo, verifyAccountProof, accountKey, verifyPaymentReceipt, headerHash } from './index';

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
    const tx = { tx_type: 'Transfer', to: 'vinx1b', amount: '10', fee: '100000', nonce: 0, chain_id: 7, payload: [], pub_key: new Array(32).fill(1), signature: 'ab'.repeat(64) };
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
      height: 7,
      state_root: 'bb'.repeat(32),
      accounts_root: 'cc'.repeat(32),
      consensus_root: 'dd'.repeat(32),
      key: 'ee'.repeat(32),
      leaf_value: null,
      proof_leaf: null,
      siblings: ['cc'.repeat(32)],
      valid: true,
    };
    const spy = mockFetch(payload);
    const client = new VinxClient(BASE);
    const result = await client.accountProof(addr);
    expect(spy).toHaveBeenCalledWith(`${BASE}/account/${addr}/proof`);
    expect(result.address).toBe(addr);
    expect(result.siblings).toHaveLength(1);
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

describe('verifyAccountProof()', () => {
  // Generated by the Rust node (crates/vinx-state/tests/proof_fixture.rs).
  const fx = JSON.parse(
    readFileSync(join(__dirname, '__fixtures__', 'account_proof.json'), 'utf8')
  );
  const [present, absent] = fx.cases;

  it('verifies an existing account against the trusted state root', () => {
    expect(verifyAccountProof(present.proof, fx.state_root, present.account)).toBe(true);
  });

  it('verifies the absence of an account', () => {
    expect(absent.account).toBeNull();
    expect(verifyAccountProof(absent.proof, fx.state_root, null)).toBe(true);
  });

  it('rejects a wrong balance', () => {
    const forged = { ...present.account, balance_atoms: '999999999' };
    expect(verifyAccountProof(present.proof, fx.state_root, forged)).toBe(false);
  });

  it('rejects an existing account claimed absent', () => {
    expect(verifyAccountProof(present.proof, fx.state_root, null)).toBe(false);
  });

  it('rejects an untrusted state root', () => {
    expect(verifyAccountProof(present.proof, 'aa'.repeat(32), present.account)).toBe(false);
  });

  it('rejects a tampered sibling', () => {
    const p = { ...present.proof, siblings: [...present.proof.siblings] };
    p.siblings[0] = '00'.repeat(32);
    expect(verifyAccountProof(p, fx.state_root, present.account)).toBe(false);
  });

  it('matches the Rust account key', () => {
    const hex = Buffer.from(accountKey(present.proof.address)).toString('hex');
    expect(hex).toBe(present.proof.key);
  });
});

describe('verifyPaymentReceipt()', () => {
  // Generated by the Rust node (tests/integration.rs, test_payment_receipt_proves_inclusion).
  const r = JSON.parse(
    readFileSync(join(__dirname, '__fixtures__', 'payment_receipt.json'), 'utf8')
  );

  it('recomputes the Rust block hash', () => {
    expect(headerHash(r.header)).toBe(r.block_hash);
  });

  it('verifies the receipt of the right transaction', () => {
    expect(verifyPaymentReceipt(r, r.tx_hash)).toBe(true);
  });

  it('rejects another transaction hash', () => {
    const other = 'ab'.repeat(32);
    expect(verifyPaymentReceipt({ ...r, tx_hash: other }, other)).toBe(false);
  });

  it('rejects a wrong index or a tampered sibling', () => {
    expect(verifyPaymentReceipt({ ...r, index: (r.index + 1) % r.header.tx_count }, r.tx_hash)).toBe(false);
    const sibs = [...r.siblings];
    sibs[0] = '00'.repeat(32);
    expect(verifyPaymentReceipt({ ...r, siblings: sibs }, r.tx_hash)).toBe(false);
  });

  it('rejects a tampered header', () => {
    const header = { ...r.header, timestamp: r.header.timestamp + 1 };
    expect(verifyPaymentReceipt({ ...r, header }, r.tx_hash)).toBe(false);
  });
});

describe('payment addresses and memo (ADR 0085)', () => {
  const addr = JSON.parse(
    readFileSync(join(__dirname, '__fixtures__', 'account_proof.json'), 'utf8')
  ).cases[0].proof.address;

  it('parses name@domain only', () => {
    expect(parsePaymentAddress('julie@vinxpay.com')).toEqual({ name: 'julie', domain: 'vinxpay.com' });
    expect(parsePaymentAddress('Julie@x.com')).toBeNull();
    expect(parsePaymentAddress('a@x.com/evil')).toBeNull();
    expect(parsePaymentAddress(addr)).toBeNull();
  });

  it('resolves through .well-known/vinx.json over https', async () => {
    const f = jest.fn(async (url: string) => ({
      ok: true,
      json: async () => ({ names: { julie: addr } }),
      url,
    }));
    await expect(resolvePaymentAddress('julie@vinxpay.com', f as any)).resolves.toBe(addr);
    expect(f.mock.calls[0][0]).toBe('https://vinxpay.com/.well-known/vinx.json?name=julie');
  });

  it('rejects unknown names and invalid addresses', async () => {
    const f = async () => ({ ok: true, json: async () => ({ names: { bob: 'nope' } }) });
    await expect(resolvePaymentAddress('julie@x.com', f as any)).rejects.toThrow();
    await expect(resolvePaymentAddress('bob@x.com', f as any)).rejects.toThrow();
  });

  it('bounds the memo to 32 bytes', () => {
    expect(encodeMemo('FAC-2026-0412')).toHaveLength(13);
    expect(() => encodeMemo('x'.repeat(33))).toThrow();
  });
});
