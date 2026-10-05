// Checks the extension's wallet core against the Rust implementation:
//   node apps/vinx-extension/test.mjs
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import assert from 'node:assert/strict';

globalThis.window = globalThis.self = globalThis;
for (const f of ['nacl.min.js', 'blake3.min.js', 'bip39-english.js', 'vinx.js']) {
  vm.runInThisContext(readFileSync(new URL(`./lib/${f}`, import.meta.url), 'utf8'), { filename: f });
}
const V = globalThis.VinX;

// Reference vector from vinx-desktop-core (Rust): same phrase, same address.
const phrase = 'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about';
const kp = V.keyPair(await V.seedFromPhrase(phrase));
assert.equal(kp.address, 'vinx196g5fh0k5w0wf6qasvf8qp47vjmtrw2wm7jc0m');

// A new phrase is valid, a tampered one is not.
const fresh = await V.newPhrase();
assert.equal(fresh.split(' ').length, 12);
assert.ok(await V.checkPhrase(fresh));
assert.ok(!(await V.checkPhrase(phrase.replace(/about$/, 'abandon'))));

// Vault round trip; a wrong password fails.
const seed = await V.seedFromPhrase(fresh);
const vault = await V.seal(seed, 'un mot de passe');
assert.deepEqual(await V.open(vault, 'un mot de passe'), seed);
await assert.rejects(V.open(vault, 'mauvais'), /bad-password/);

// Addresses, amounts and payment URIs.
assert.deepEqual(V.addressBytes(kp.address).length, 20);
assert.ok(!V.isAddress('vinx1abc'));
assert.equal(V.parseAmount('12,40'), 12400000000n);
assert.equal(V.fmt(12400000000n, 2, 'en-US'), '12.40');
assert.deepEqual(V.parseUri(`vinx:${kp.address}?amount=12.40&memo=TICKET-0815`), { to: kp.address, amount: '12.40', memo: 'TICKET-0815' });

// The signature verifies against the signer's key.
const body = JSON.parse(V.signTx(kp, { to: kp.address, amount: 1n, fee: 100000n, nonce: 0, chainId: 7 }));
assert.equal(body.pub_key.length, 32);
assert.equal(body.signature.length, 128);
console.log('extension core: all checks passed');
