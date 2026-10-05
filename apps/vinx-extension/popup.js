// VinX Wallet — popup. The seed is encrypted at rest (chrome.storage.local); once
// unlocked it lives in chrome.storage.session (memory only, cleared when the browser
// closes) for 15 minutes of inactivity. It is never sent anywhere: transactions are
// signed here and only the signed transaction goes to the node.
'use strict';
const V = window.VinX;
const $ = (id) => document.getElementById(id);
const esc = (s) => String(s ?? '').replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
const AUTO_LOCK_MS = 15 * 60 * 1000;
const DEFAULT_NODE = 'http://127.0.0.1:8545';

// ── Storage: the extension APIs, or localStorage when opened as a plain page (tests) ──
const ext = typeof chrome !== 'undefined' && chrome.storage && chrome.storage.local;
const store = {
  async get(area, key) {
    if (ext) return (await chrome.storage[area].get(key))[key];
    try { return JSON.parse(localStorage.getItem(`${area}:${key}`)); } catch { return undefined; }
  },
  async set(area, key, value) {
    if (ext) return chrome.storage[area].set({ [key]: value });
    localStorage.setItem(`${area}:${key}`, JSON.stringify(value));
  },
  async remove(area, key) {
    if (ext) return chrome.storage[area].remove(key);
    localStorage.removeItem(`${area}:${key}`);
  },
};

// ── State ─────────────────────────────────────────────────────────────────────
let kp = null; // { pk, sk, address } once unlocked
let pendingSeed = null; // created or restored, waiting for a password
let net = { ok: false, chainId: 0, baseFee: 100000n };
let account = { balance: 0n, nonce: 0 };
let confirmSend = null;

const ERR = {
  'bad-phrase': 'Phrase de récupération invalide (12 mots, vérifiez l’orthographe).',
  'bad-password': 'Mot de passe incorrect.',
  'short-password': 'Le mot de passe doit faire au moins 8 caractères.',
  'bad-address': 'Adresse invalide.',
  'bad-amount': 'Montant invalide.',
};
const errText = (e) => ERR[e && e.message] || (e && e.message) || String(e);
function msg(id, kind, text) { $(id).innerHTML = text ? `<div class="msg ${kind}">${esc(text)}</div>` : ''; }

// ── Screens ───────────────────────────────────────────────────────────────────
const VIEWS = ['v-welcome', 'v-create', 'v-import', 'v-password', 'v-unlock', 'v-home', 'v-send', 'v-recv', 'v-settings'];
function show(id) {
  VIEWS.forEach((v) => ($(v).hidden = v !== id));
  $('go-settings').hidden = !kp;
  if (id === 'v-send') resetSend();
  if (id === 'v-recv') renderReceive();
  if (id === 'v-home') refresh();
}
document.querySelectorAll('[data-go]').forEach((b) => (b.onclick = () => show(b.dataset.go)));
document.querySelectorAll('[data-copy]').forEach((b) => (b.onclick = async () => {
  await navigator.clipboard.writeText($(b.dataset.copy).textContent);
  b.title = 'Copié';
}));
$('go-settings').onclick = async () => { $('t-node').value = await nodeUrl(); msg('t-msg'); show('v-settings'); };

// ── Network ───────────────────────────────────────────────────────────────────
async function nodeUrl() { return ((await store.get('local', 'node')) || DEFAULT_NODE).replace(/\/$/, ''); }
async function api(path, init) {
  const r = await fetch((await nodeUrl()) + path, init);
  const j = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(j.error || `HTTP ${r.status}`);
  return j;
}
async function checkNet() {
  try {
    const h = await api('/health');
    const s = await api('/network/stats').catch(() => ({}));
    net = { ok: true, chainId: h.chain_id, height: h.height, baseFee: BigInt(s.base_fee_atoms || 100000) };
    $('net-dot').className = 'dot ok';
    $('net-txt').textContent = `${h.chain_id === 7 ? 'testnet' : 'chaîne ' + h.chain_id} · #${Number(h.height).toLocaleString('fr-FR')}`;
  } catch {
    net.ok = false;
    $('net-dot').className = 'dot';
    $('net-txt').textContent = 'hors ligne';
  }
}

// ── Unlock / lock ─────────────────────────────────────────────────────────────
async function openSession(seed) {
  kp = V.keyPair(seed);
  await store.set('session', 'unlocked', { seed: V.hex(seed), until: Date.now() + AUTO_LOCK_MS });
}
async function lock() {
  kp = null;
  await store.remove('session', 'unlocked');
  boot();
}
async function boot() {
  checkNet();
  const vault = await store.get('local', 'vault');
  if (!vault) return show('v-welcome');
  const s = await store.get('session', 'unlocked');
  if (s && s.until > Date.now()) {
    await openSession(V.unhex(s.seed)); // renews the timer
    return show('v-home');
  }
  await store.remove('session', 'unlocked');
  $('u-addr').textContent = vault.address;
  msg('u-msg');
  show('v-unlock');
  setTimeout(() => $('u-pass').focus(), 50);
}
async function unlock() {
  $('u-go').disabled = true;
  msg('u-msg', 'wait', 'Déchiffrement…');
  try {
    const seed = await V.open(await store.get('local', 'vault'), $('u-pass').value);
    $('u-pass').value = '';
    await openSession(seed);
    msg('u-msg');
    show('v-home');
  } catch (e) { msg('u-msg', 'err', errText(e)); }
  $('u-go').disabled = false;
}
$('u-go').onclick = unlock;
$('u-pass').onkeydown = (e) => { if (e.key === 'Enter') unlock(); };

// ── Create / import ───────────────────────────────────────────────────────────
$('w-create').onclick = async () => {
  const phrase = await V.newPhrase();
  pendingSeed = await V.seedFromPhrase(phrase);
  $('c-words').innerHTML = phrase.split(' ').map((w, i) => `<div><span>${i + 1}</span>${esc(w)}</div>`).join('');
  $('c-ok').checked = false;
  $('c-next').disabled = true;
  show('v-create');
};
$('c-ok').onchange = () => ($('c-next').disabled = !$('c-ok').checked);
$('c-next').onclick = () => { $('p-1').value = $('p-2').value = ''; msg('p-msg'); show('v-password'); };
$('w-import').onclick = () => { $('i-phrase').value = ''; msg('i-msg'); show('v-import'); };
$('i-next').onclick = async () => {
  try {
    pendingSeed = await V.seedFromPhrase($('i-phrase').value);
    $('i-phrase').value = '';
    $('p-1').value = $('p-2').value = '';
    msg('p-msg');
    show('v-password');
  } catch (e) { msg('i-msg', 'err', errText(e)); }
};
$('p-go').onclick = async () => {
  if ($('p-1').value !== $('p-2').value) return msg('p-msg', 'err', 'Les deux mots de passe diffèrent.');
  $('p-go').disabled = true;
  msg('p-msg', 'wait', 'Chiffrement…');
  try {
    await store.set('local', 'vault', await V.seal(pendingSeed, $('p-1').value));
    await openSession(pendingSeed);
    pendingSeed = null;
    show('v-home');
  } catch (e) { msg('p-msg', 'err', errText(e)); }
  $('p-go').disabled = false;
};

// ── Home ──────────────────────────────────────────────────────────────────────
async function refresh() {
  if (!kp) return;
  $('h-addr').textContent = kp.address;
  await checkNet();
  try {
    const a = await api('/account/' + kp.address);
    account = { balance: BigInt(a.balance_atoms), nonce: a.nonce };
  } catch { account = { balance: 0n, nonce: 0 }; }
  $('h-bal').innerHTML = `${V.fmt(account.balance)}<small>VINX</small>`;
  try {
    const { txs } = await api(`/account/${kp.address}/txs?limit=20`);
    $('h-txs').innerHTML = txs.length ? txs.map((t) => {
      const incoming = t.to === kp.address && t.from !== kp.address;
      const label = t.tx_type === 'Transfer' ? (incoming ? 'Reçu' : 'Envoyé') : ({ Stake: 'Garantie', Unstake: 'Retrait' }[t.tx_type] || t.tx_type);
      const who = t.tx_type === 'Transfer' ? (incoming ? 'de ' + t.from : 'à ' + t.to) : '';
      return `<div class="item"><div class="ico ${incoming ? 'in' : ''}"><svg class="i"><use href="#${incoming ? 'i-recv' : 'i-send'}"/></svg></div>
        <div class="grow"><div class="t">${label}</div><div class="s mono">${esc(t.memo ? t.memo + ' · ' : '')}${esc(who.slice(0, 22))}…</div></div>
        <div class="amt ${incoming ? 'in' : ''}">${incoming ? '+' : '−'}${V.fmt(t.amount_atoms)}</div></div>`;
    }).join('') : '<div class="empty">Aucune opération.</div>';
  } catch { $('h-txs').innerHTML = '<div class="empty">Historique indisponible.</div>'; }
}
$('h-faucet').onclick = async () => {
  msg('h-msg', 'wait', 'Demande au faucet du réseau de test…');
  try {
    await api('/faucet/request', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ address: kp.address }) });
    msg('h-msg', 'ok', 'Accepté : les VINX arrivent au prochain bloc.');
    setTimeout(refresh, 13000);
  } catch (e) { msg('h-msg', 'err', /not enabled/.test(e.message) ? 'Pas de faucet sur ce nœud.' : e.message); }
};

// ── Send ──────────────────────────────────────────────────────────────────────
function resetSend() {
  confirmSend = null;
  $('s-sum').hidden = true;
  $('s-go').textContent = 'Vérifier';
  msg('s-msg');
  $('s-avail').textContent = `Disponible : ${V.fmt(account.balance)} VINX`;
}
['s-amt', 's-memo'].forEach((id) => ($(id).oninput = resetSend));
// A merchant QR code (vinx:…?amount=…&memo=…) pasted in the recipient fills everything.
$('s-to').oninput = () => {
  const u = V.parseUri($('s-to').value);
  if (u) {
    $('s-to').value = u.to;
    if (u.amount) $('s-amt').value = u.amount;
    if (u.memo) $('s-memo').value = u.memo.slice(0, 32);
  }
  resetSend();
};
$('s-go').onclick = async () => {
  try {
    if (!confirmSend) {
      const to = $('s-to').value.trim().toLowerCase();
      V.addressBytes(to);
      const amount = V.parseAmount($('s-amt').value);
      if (amount <= 0n) throw new Error('bad-amount');
      const memo = new TextEncoder().encode($('s-memo').value.trim());
      if (memo.length > 32) throw new Error('Référence trop longue (32 octets maximum).');
      if (amount + net.baseFee > account.balance) throw new Error('Solde insuffisant.');
      confirmSend = { to, amount, memo };
      $('s-sum').innerHTML = `<div><span>À</span><span class="mono">${esc(to.slice(0, 14))}…${esc(to.slice(-6))}</span></div>
        ${memo.length ? `<div><span>Référence</span><span>${esc($('s-memo').value.trim())}</span></div>` : ''}
        <div><span>Frais</span><span class="mono">${V.fmt(net.baseFee, 4)}</span></div>
        <div><span>Montant</span><span class="mono">${V.fmt(amount, 9).replace(/0+$/, '').replace(/[,.]$/, '')} VINX</span></div>`;
      $('s-sum').hidden = false;
      $('s-go').textContent = 'Confirmer l’envoi';
      return;
    }
    $('s-go').disabled = true;
    msg('s-msg', 'wait', 'Envoi…');
    await refresh(); // fresh nonce and chain id
    const body = V.signTx(kp, { to: confirmSend.to, amount: confirmSend.amount, fee: net.baseFee, nonce: account.nonce, chainId: net.chainId, payload: confirmSend.memo });
    const r = await api('/tx/submit', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body });
    if (r.accepted === false) throw new Error(r.error || 'refusé par le nœud');
    msg('s-msg', 'ok', 'Envoyé : définitif au prochain bloc.');
    $('s-to').value = $('s-amt').value = $('s-memo').value = '';
    confirmSend = null;
    $('s-sum').hidden = true;
    $('s-go').textContent = 'Vérifier';
    setTimeout(refresh, 13000);
  } catch (e) { msg('s-msg', 'err', errText(e)); }
  $('s-go').disabled = false;
};

// ── Receive ───────────────────────────────────────────────────────────────────
function renderReceive() {
  $('r-addr').textContent = kp.address;
  const qr = qrcode(0, 'M');
  qr.addData('vinx:' + kp.address);
  qr.make();
  $('r-qr').innerHTML = qr.createSvgTag({ cellSize: 4, margin: 0, scalable: true });
}

// ── Settings ──────────────────────────────────────────────────────────────────
$('t-save').onclick = async () => {
  const v = $('t-node').value.trim().replace(/\/$/, '');
  if (!/^https?:\/\/.+/.test(v)) return msg('t-msg', 'err', 'Adresse attendue : http://… ou https://…');
  await store.set('local', 'node', v);
  await checkNet();
  msg('t-msg', net.ok ? 'ok' : 'err', net.ok ? `Connecté (chaîne ${net.chainId}).` : 'Nœud injoignable à cette adresse.');
};
$('t-lock').onclick = lock;
$('t-forget').onclick = async () => {
  if ($('t-forget').dataset.armed !== '1') {
    $('t-forget').dataset.armed = '1';
    $('t-forget').textContent = 'Confirmer : sans la phrase de 12 mots, les fonds sont perdus';
    return;
  }
  await store.remove('local', 'vault');
  await lock();
};

$('foot').textContent = `VinX Wallet ${ext ? chrome.runtime.getManifest().version : ''}`;
boot();
