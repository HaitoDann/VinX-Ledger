// VinX wallet core for the browser extension: addresses, recovery phrase, signing,
// encrypted vault, payment URIs. No UI here, so it can be checked against the Rust
// implementation (see test.mjs). Compatible with vinx-wallet and the desktop app:
// the same 12 words give the same address everywhere.
'use strict';
(function (root) {
  const nacl = root.nacl;
  const blake3 = root.vinxBlake3;
  const subtle = root.crypto.subtle;
  const enc = new TextEncoder();

  const DEC = 1000000000n; // 9 decimals
  const hex = (b) => Array.from(b, (x) => x.toString(16).padStart(2, '0')).join('');
  const unhex = (h) => Uint8Array.from(h.match(/.{2}/g) || [], (x) => parseInt(x, 16));
  const be = (v, n) => {
    const b = new Uint8Array(n);
    v = BigInt(v);
    for (let i = n - 1; i >= 0; i--) { b[i] = Number(v & 0xffn); v >>= 8n; }
    return b;
  };
  const concat = (parts) => {
    const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
    let o = 0;
    for (const p of parts) { out.set(p, o); o += p.length; }
    return out;
  };

  // ── Bech32m addresses (vinx_crypto::Address) ────────────────────────────────
  const B32 = 'qpzry9x8gf2tvdw0s3jn54khce6mua7l';
  function polymod(v) {
    const G = [0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3];
    let c = 1;
    for (const x of v) {
      const t = c >>> 25;
      c = ((c & 0x1ffffff) << 5) ^ x;
      for (let i = 0; i < 5; i++) if ((t >>> i) & 1) c ^= G[i];
    }
    return c;
  }
  const hrpx = (h) => [...h].map((c) => c.charCodeAt(0) >> 5).concat([0], [...h].map((c) => c.charCodeAt(0) & 31));
  function b32enc(bytes) {
    const d = [];
    let acc = 0, bits = 0;
    for (const b of bytes) { acc = (acc << 8) | b; bits += 8; while (bits >= 5) { bits -= 5; d.push((acc >> bits) & 31); } }
    if (bits) d.push((acc << (5 - bits)) & 31);
    const pm = polymod(hrpx('vinx').concat(d, [0, 0, 0, 0, 0, 0])) ^ 0x2bc830a3;
    return 'vinx1' + d.concat([0, 1, 2, 3, 4, 5].map((i) => (pm >>> (5 * (5 - i))) & 31)).map((x) => B32[x]).join('');
  }
  function addressBytes(addr) {
    const s = String(addr).trim().toLowerCase();
    if (!s.startsWith('vinx1')) throw new Error('bad-address');
    const v = [...s.slice(5)].map((c) => { const i = B32.indexOf(c); if (i < 0) throw new Error('bad-address'); return i; });
    if (polymod(hrpx('vinx').concat(v)) !== 0x2bc830a3) throw new Error('bad-address');
    const out = [];
    let acc = 0, bits = 0;
    for (const w of v.slice(0, -6)) { acc = (acc << 5) | w; bits += 5; while (bits >= 8) { bits -= 8; out.push((acc >> bits) & 255); } }
    if (out.length !== 20) throw new Error('bad-address');
    return new Uint8Array(out);
  }
  const isAddress = (a) => { try { addressBytes(a); return true; } catch { return false; } };
  // Address = BLAKE3(0x00 ‖ Ed25519 public key)[..20]
  const addressOf = (pk) => { const t = new Uint8Array(33); t.set(pk, 1); return b32enc(blake3(t).slice(0, 20)); };

  // ── BIP-39 recovery phrase (bip39 crate: seed = PBKDF2-SHA512, key = seed[..32]) ─
  async function newPhrase() {
    const entropy = root.crypto.getRandomValues(new Uint8Array(16));
    const check = new Uint8Array(await subtle.digest('SHA-256', entropy))[0] >> 4; // 4 bits
    let bits = '';
    for (const b of entropy) bits += b.toString(2).padStart(8, '0');
    bits += check.toString(2).padStart(4, '0');
    const words = [];
    for (let i = 0; i < 12; i++) words.push(root.BIP39_EN[parseInt(bits.slice(i * 11, i * 11 + 11), 2)]);
    return words.join(' ');
  }
  function normalise(phrase) {
    return String(phrase).trim().toLowerCase().split(/\s+/).join(' ');
  }
  async function checkPhrase(phrase) {
    const words = normalise(phrase).split(' ');
    if (![12, 15, 18, 21, 24].includes(words.length)) return false;
    let bits = '';
    for (const w of words) {
      const i = root.BIP39_EN.indexOf(w);
      if (i < 0) return false;
      bits += i.toString(2).padStart(11, '0');
    }
    const ent = bits.length * 32 / 33;
    const entropy = Uint8Array.from(bits.slice(0, ent).match(/.{8}/g), (b) => parseInt(b, 2));
    const hash = new Uint8Array(await subtle.digest('SHA-256', entropy));
    let hbits = '';
    for (const b of hash) hbits += b.toString(2).padStart(8, '0');
    return bits.slice(ent) === hbits.slice(0, bits.length - ent);
  }
  async function seedFromPhrase(phrase) {
    if (!(await checkPhrase(phrase))) throw new Error('bad-phrase');
    const key = await subtle.importKey('raw', enc.encode(normalise(phrase).normalize('NFKD')), 'PBKDF2', false, ['deriveBits']);
    const bits = await subtle.deriveBits({ name: 'PBKDF2', hash: 'SHA-512', salt: enc.encode('mnemonic'), iterations: 2048 }, key, 512);
    return new Uint8Array(bits).slice(0, 32);
  }
  function keyPair(seed) {
    const kp = nacl.sign.keyPair.fromSeed(seed);
    return { pk: kp.publicKey, sk: kp.secretKey, address: addressOf(kp.publicKey) };
  }

  // ── Encrypted vault (PBKDF2-SHA256 600 000 iterations → AES-256-GCM) ────────
  async function vaultKey(password, salt, iterations) {
    const base = await subtle.importKey('raw', enc.encode(password), 'PBKDF2', false, ['deriveKey']);
    return subtle.deriveKey({ name: 'PBKDF2', hash: 'SHA-256', salt, iterations }, base, { name: 'AES-GCM', length: 256 }, false, ['encrypt', 'decrypt']);
  }
  async function seal(seed, password) {
    if (String(password).length < 8) throw new Error('short-password');
    const salt = root.crypto.getRandomValues(new Uint8Array(16));
    const iv = root.crypto.getRandomValues(new Uint8Array(12));
    const iterations = 600000;
    const ct = await subtle.encrypt({ name: 'AES-GCM', iv }, await vaultKey(password, salt, iterations), seed);
    return { v: 1, kdf: 'pbkdf2-sha256', iterations, salt: hex(salt), iv: hex(iv), ct: hex(new Uint8Array(ct)), address: keyPair(seed).address };
  }
  async function open(vault, password) {
    try {
      const pt = await subtle.decrypt({ name: 'AES-GCM', iv: unhex(vault.iv) }, await vaultKey(password, unhex(vault.salt), vault.iterations), unhex(vault.ct));
      return new Uint8Array(pt);
    } catch { throw new Error('bad-password'); }
  }

  // ── Transactions (vinx-core Transaction::signing_bytes) ─────────────────────
  // disc ‖ 0x00 (Ed25519) ‖ public key ‖ to ‖ amount(16) ‖ fee(16) ‖ nonce(8) ‖ chain_id(4)
  // ‖ expiry 0x00 ‖ payload_len(4) ‖ payload ‖ sponsor 0x00
  const DISC = { Transfer: 1, Stake: 2, Unstake: 3 };
  function signTx(kp, { type = 'Transfer', to, amount, fee, nonce, chainId, payload = new Uint8Array() }) {
    const msg = concat([[DISC[type]], [0], kp.pk, addressBytes(to), be(amount, 16), be(fee, 16), be(nonce, 8), be(chainId, 4), [0], be(payload.length, 4), payload, [0]].map((p) => Uint8Array.from(p)));
    const sig = nacl.sign.detached(msg, kp.sk);
    return `{"tx_type":"${type}","to":"${to}","amount":${amount},"fee":${fee},"nonce":${nonce},"chain_id":${chainId},"payload":[${Array.from(payload).join(',')}],"pub_key":[${Array.from(kp.pk).join(',')}],"signature":"${hex(sig)}"}`;
  }

  // ── Amounts and payment URIs ────────────────────────────────────────────────
  function parseAmount(s) {
    s = String(s).trim().replace(/\s/g, '').replace(',', '.');
    if (!/^\d+(\.\d{0,9})?$/.test(s)) throw new Error('bad-amount');
    const [w, f = ''] = s.split('.');
    return BigInt(w) * DEC + BigInt((f + '000000000').slice(0, 9));
  }
  function fmt(atoms, digits = 2, locale = 'fr-FR') {
    const a = BigInt(atoms), neg = a < 0n, x = neg ? -a : a;
    const w = (x / DEC).toLocaleString(locale);
    const f = (x % DEC).toString().padStart(9, '0').slice(0, digits);
    const dec = locale.startsWith('fr') ? ',' : '.';
    return (neg ? '−' : '') + w + (digits ? dec + f : '');
  }
  // vinx:<address>?amount=12.40&memo=REF (as shown by merchant QR codes)
  function parseUri(text) {
    const s = String(text).trim();
    if (!/^vinx:/i.test(s)) return null;
    const [addr, query = ''] = s.slice(5).split('?');
    if (!isAddress(addr)) return null;
    const q = new URLSearchParams(query);
    return { to: addr.toLowerCase(), amount: q.get('amount') || '', memo: q.get('memo') || '' };
  }

  root.VinX = { hex, unhex, addressOf, addressBytes, isAddress, newPhrase, checkPhrase, seedFromPhrase, keyPair, seal, open, signTx, parseAmount, fmt, parseUri, DEC };
})(typeof window !== 'undefined' ? window : globalThis);
