use axum::response::Html;

pub async fn index() -> Html<&'static str> {
    Html(HTML)
}

const HTML: &str = r####"<!DOCTYPE html>
<html lang="fr">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>VinX Ledger — Dashboard</title>
<!-- TweetNaCl: Ed25519 signing in the browser, no server contact for private key -->
<script src="https://cdn.jsdelivr.net/npm/tweetnacl@1.0.3/nacl-fast.min.js"></script>
<style>
  :root {
    --bg: #0d1117; --surface: #161b22; --surface2: #21262d;
    --border: #30363d; --text: #e6edf3; --muted: #8b949e;
    --accent: #58a6ff; --green: #3fb950; --red: #f85149;
    --orange: #d29922; --purple: #bc8cff; --radius: 8px;
  }
  * { box-sizing: border-box; margin: 0; padding: 0; }
  body { background: var(--bg); color: var(--text); font-family: 'Segoe UI', system-ui, sans-serif; font-size: 14px; }

  header { background: var(--surface); border-bottom: 1px solid var(--border); padding: 14px 32px; display: flex; align-items: center; gap: 12px; }
  .logo { font-size: 20px; font-weight: 700; color: var(--accent); }
  .logo span { color: var(--muted); font-weight: 400; font-size: 13px; margin-left: 8px; }
  .dot { width: 8px; height: 8px; border-radius: 50%; background: var(--green); margin-left: auto; }
  .dot.off { background: var(--red); }
  .dot-label { color: var(--muted); font-size: 12px; }

  main { max-width: 1060px; margin: 0 auto; padding: 24px; display: grid; gap: 20px; }

  .card { background: var(--surface); border: 1px solid var(--border); border-radius: var(--radius); overflow: hidden; }
  .card-header { padding: 12px 20px; border-bottom: 1px solid var(--border); font-weight: 600; font-size: 12px; color: var(--muted); text-transform: uppercase; letter-spacing: 0.6px; display: flex; align-items: center; }
  .card-body { padding: 20px; }

  /* Stats */
  .stats { display: grid; grid-template-columns: repeat(3,1fr); gap: 16px; }
  .stat-v { font-size: 30px; font-weight: 700; color: var(--accent); font-variant-numeric: tabular-nums; }
  .stat-l { color: var(--muted); font-size: 12px; margin-top: 3px; }

  /* Forms */
  .row { display: flex; gap: 8px; align-items: stretch; }
  input[type=text], input[type=number] {
    flex: 1; background: var(--bg); border: 1px solid var(--border); border-radius: 6px;
    padding: 9px 12px; color: var(--text); font-size: 14px; outline: none;
    transition: border-color .15s; font-family: inherit;
  }
  input[type=text].mono { font-family: 'Courier New', monospace; font-size: 13px; }
  input:focus { border-color: var(--accent); }
  input:disabled { opacity: .5; cursor: default; }

  button {
    background: var(--accent); color: #fff; border: none; border-radius: 6px;
    padding: 9px 18px; font-size: 14px; font-weight: 600; cursor: pointer;
    white-space: nowrap; transition: opacity .15s;
  }
  button:hover { opacity: .85; }
  button:disabled { opacity: .35; cursor: default; }
  button.ghost { background: var(--surface2); color: var(--text); border: 1px solid var(--border); }
  button.ghost:hover { border-color: var(--accent); color: var(--accent); opacity: 1; }
  button.danger { background: transparent; color: var(--red); border: 1px solid var(--border); }
  button.danger:hover { border-color: var(--red); opacity: 1; }

  /* Tags */
  .tag { display: inline-block; padding: 2px 8px; border-radius: 4px; font-size: 11px; font-weight: 600; }
  .tag.g { background: rgba(63,185,80,.15); color: var(--green); }
  .tag.r { background: rgba(248,81,73,.15); color: var(--red); }
  .tag.b { background: rgba(88,166,255,.15); color: var(--accent); }
  .tag.o { background: rgba(210,153,34,.15); color: var(--orange); }
  .tag.p { background: rgba(188,140,255,.15); color: var(--purple); }

  /* Result grids */
  .rg { display: grid; grid-template-columns: 1fr 1fr; gap: 10px; margin-top: 14px; }
  .ri { background: var(--bg); border: 1px solid var(--border); border-radius: 6px; padding: 10px 14px; }
  .ri .l { color: var(--muted); font-size: 11px; text-transform: uppercase; letter-spacing: .5px; margin-bottom: 3px; }
  .ri .v { font-weight: 600; word-break: break-all; }
  .ri .v.mono { font-family: 'Courier New', monospace; font-size: 12px; }
  .ri.full { grid-column: 1 / -1; }

  /* Wallet section */
  .wallet-loaded { display: none; }
  .wallet-info { display: flex; align-items: center; gap: 12px; flex-wrap: wrap; margin-top: 14px; background: var(--bg); border: 1px solid var(--border); border-radius: 6px; padding: 14px 16px; }
  .wallet-addr { font-family: 'Courier New', monospace; font-size: 12px; color: var(--text); flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .wallet-bal { color: var(--accent); font-weight: 700; white-space: nowrap; }
  .wallet-nonce { color: var(--muted); font-size: 12px; white-space: nowrap; }

  /* Tabs */
  .tabs { display: flex; gap: 0; border-bottom: 1px solid var(--border); margin-bottom: 18px; }
  .tab { padding: 9px 16px; cursor: pointer; color: var(--muted); font-size: 13px; border-bottom: 2px solid transparent; margin-bottom: -1px; transition: all .15s; user-select: none; }
  .tab:hover { color: var(--text); }
  .tab.on { color: var(--accent); border-bottom-color: var(--accent); }

  /* Fee hint */
  .fee-hint { font-size: 12px; color: var(--muted); margin-top: 8px; }
  .fee-hint span { color: var(--orange); font-weight: 600; }

  /* Action result */
  .action-result { margin-top: 14px; }

  /* Tx list */
  .tx-list { margin-top: 12px; display: grid; gap: 8px; }
  .tx-row { background: var(--bg); border: 1px solid var(--border); border-radius: 6px; padding: 12px 16px; display: grid; grid-template-columns: auto 1fr auto; gap: 12px; align-items: center; }

  /* Block nav */
  .bnav { display: flex; align-items: center; gap: 8px; }
  .bnav input { max-width: 110px; }
  .bnav button { padding: 8px 14px; font-size: 13px; }

  /* Messages */
  .msg { color: var(--muted); font-size: 13px; margin-top: 12px; }
  .msg.err { color: var(--red); }
  .msg.ok { color: var(--green); font-weight: 600; }

  /* Security notice */
  .sec-notice { background: rgba(210,153,34,.08); border: 1px solid rgba(210,153,34,.25); border-radius: 6px; padding: 10px 14px; font-size: 12px; color: var(--orange); margin-bottom: 14px; }

  /* Live badge */
  .live { display: inline-flex; align-items: center; gap: 5px; font-size: 11px; color: var(--green); margin-left: auto; }
  .live::before { content:''; width:6px; height:6px; border-radius:50%; background:var(--green); animation: pulse 2s infinite; }
  @keyframes pulse { 0%,100%{opacity:1} 50%{opacity:.3} }

  /* Responsive */
  @media(max-width:600px) {
    .stats { grid-template-columns:1fr; }
    .rg { grid-template-columns:1fr; }
    .tx-row { grid-template-columns:1fr; }
    header { padding:12px 16px; }
    main { padding:12px; }
    .bnav { flex-wrap:wrap; }
  }
</style>
</head>
<body>

<header>
  <div class="logo">VinX Ledger <span>DEVNET</span></div>
  <div id="hd" class="dot off"></div>
  <div id="hs" class="dot-label">Connexion…</div>
</header>

<main>

  <!-- Network status -->
  <div class="card">
    <div class="card-header">Réseau <span class="live">Live</span></div>
    <div class="card-body">
      <div class="stats">
        <div class="stat"><div class="stat-v" id="s-h">—</div><div class="stat-l">Hauteur de bloc</div></div>
        <div class="stat"><div class="stat-v" id="s-s">—</div><div class="stat-l">Statut</div></div>
        <div class="stat"><div class="stat-v" id="s-m">—</div><div class="stat-l">Mempool en attente</div></div>
      </div>
    </div>
  </div>

  <!-- Wallet -->
  <div class="card">
    <div class="card-header">Wallet</div>
    <div class="card-body">
      <div class="sec-notice">
        Votre clé secrète ne quitte jamais le navigateur — la signature est effectuée localement.
      </div>
      <div id="wallet-load-row" class="row">
        <label style="flex:1">
          <input type="file" accept=".json" id="wallet-file" style="display:none" onchange="onWalletFile(this)">
          <button style="width:100%;background:var(--surface2);color:var(--text);border:1px solid var(--border)" onclick="document.getElementById('wallet-file').click()">
            Charger un fichier wallet (.json)
          </button>
        </label>
      </div>
      <div id="wallet-err" class="msg err"></div>
      <div class="wallet-loaded" id="wallet-loaded">
        <div class="wallet-info">
          <div class="wallet-addr" id="w-addr"></div>
          <div class="wallet-bal" id="w-bal">—</div>
          <div class="wallet-nonce" id="w-nonce">Nonce: —</div>
          <button class="danger" onclick="clearWallet()">Déconnecter</button>
        </div>
      </div>
    </div>
  </div>

  <!-- Actions (visible when wallet loaded) -->
  <div class="card wallet-loaded" id="actions-card">
    <div class="card-header">Envoyer / Staker</div>
    <div class="card-body">
      <div class="tabs">
        <div class="tab on" id="tab-transfer" onclick="switchTab('transfer')">Transfer</div>
        <div class="tab" id="tab-stake" onclick="switchTab('stake')">Staker</div>
        <div class="tab" id="tab-unstake" onclick="switchTab('unstake')">Unstaker</div>
      </div>

      <!-- Transfer -->
      <div id="form-transfer">
        <div class="row" style="margin-bottom:8px">
          <input type="text" class="mono" id="tx-to" placeholder="Adresse destinataire vinx1…" />
        </div>
        <div class="row">
          <input type="number" id="tx-amount" placeholder="Montant (VINX)" min="0" step="any" oninput="updateFee()" />
          <button onclick="sendTx('Transfer')">Envoyer</button>
        </div>
        <div class="fee-hint" id="fee-display-transfer">Fee : —</div>
      </div>

      <!-- Stake -->
      <div id="form-stake" style="display:none">
        <div class="row">
          <input type="number" id="stake-amount" placeholder="Montant à staker (VINX)" min="0" step="any" oninput="updateFee()" />
          <button onclick="sendTx('Stake')">Staker</button>
        </div>
        <div class="fee-hint" id="fee-display-stake">Fee : —</div>
      </div>

      <!-- Unstake -->
      <div id="form-unstake" style="display:none">
        <div class="row">
          <input type="number" id="unstake-amount" placeholder="Montant à libérer (VINX)" min="0" step="any" oninput="updateFee()" />
          <button onclick="sendTx('Unstake')">Unstaker</button>
        </div>
        <div class="fee-hint" id="fee-display-unstake">Fee : —</div>
      </div>

      <div id="action-result" class="action-result"></div>
    </div>
  </div>

  <!-- Account lookup (read-only) -->
  <div class="card">
    <div class="card-header">Compte</div>
    <div class="card-body">
      <div class="row">
        <input type="text" class="mono" id="acc-in" placeholder="vinx1…" />
        <button onclick="lookupAccount()">Chercher</button>
      </div>
      <div id="acc-result"></div>
    </div>
  </div>

  <!-- Block explorer -->
  <div class="card">
    <div class="card-header">Explorateur de blocs</div>
    <div class="card-body">
      <div class="bnav">
        <button class="ghost" onclick="prevBlock()" id="btn-prev">←</button>
        <input type="number" id="blk-in" placeholder="Hauteur" min="0" onchange="lookupBlock()" />
        <button class="ghost" onclick="nextBlock()">→</button>
        <button onclick="lookupBlock()">Afficher</button>
        <button class="ghost" onclick="goLatest()">Dernier bloc</button>
      </div>
      <div id="blk-result"></div>
    </div>
  </div>

</main>

<script>
// ─── Constants ────────────────────────────────────────────────────────────────
const BASE = window.location.origin;
const DECIMAL = 1_000_000_000_000_000_000n;
const FEE_FLOOR = DECIMAL / 100n; // 0.01 VINX

// ─── State ────────────────────────────────────────────────────────────────────
let netHeight = 0;
let blockCursor = null;
let wallet = null; // { address, secretKey64, publicKey32 }
let activeTab = 'transfer';

// ─── Utility ──────────────────────────────────────────────────────────────────
const hexToBytes = h => Uint8Array.from(h.match(/.{2}/g), x => parseInt(x, 16));
const bytesToHex = b => Array.from(b, x => x.toString(16).padStart(2,'0')).join('');

function parseVinx(s) {
  s = s.trim().replace(',', '.');
  const [w, f = ''] = s.split('.');
  const frac = (f + '0'.repeat(18)).slice(0, 18);
  return BigInt(w || 0) * DECIMAL + BigInt(frac);
}

function fmtVinx(atoms) {
  const w = atoms / DECIMAL;
  const c = (atoms % DECIMAL) / (DECIMAL / 100n);
  return `${w}.${String(c).padStart(2,'0')} VINX`;
}

function calcFee(amountAtoms) {
  const pct = amountAtoms * 5n / 10000n;
  return pct > FEE_FLOOR ? pct : FEE_FLOOR;
}

function bigIntTo16BE(v) {
  const b = new Uint8Array(16);
  for (let i = 15; i >= 0; i--) { b[i] = Number(v & 0xffn); v >>= 8n; }
  return b;
}

function bigIntTo8BE(v) {
  const b = new Uint8Array(8);
  for (let i = 7; i >= 0; i--) { b[i] = Number(v & 0xffn); v >>= 8n; }
  return b;
}

// ─── Network status (auto-refresh) ───────────────────────────────────────────
async function refreshStatus() {
  try {
    const h = await (await fetch(BASE + '/health')).json();
    netHeight = h.height;
    document.getElementById('s-h').textContent = h.height.toLocaleString();
    document.getElementById('s-s').innerHTML = `<span class="tag g">${h.status}</span>`;
    document.getElementById('s-m').textContent = h.mempool_pending;
    document.getElementById('hd').className = 'dot';
    document.getElementById('hs').textContent = 'http://127.0.0.1:8545';
  } catch {
    document.getElementById('s-h').textContent = '—';
    document.getElementById('s-s').innerHTML = `<span class="tag r">offline</span>`;
    document.getElementById('s-m').textContent = '—';
    document.getElementById('hd').className = 'dot off';
    document.getElementById('hs').textContent = 'Nœud injoignable';
  }
}
setInterval(refreshStatus, 3000);
refreshStatus();

// ─── Wallet ───────────────────────────────────────────────────────────────────
function onWalletFile(input) {
  const file = input.files[0];
  if (!file) return;
  const reader = new FileReader();
  reader.onload = e => {
    try {
      const json = JSON.parse(e.target.result);
      if (!json.secret_key_hex || !json.address) throw new Error('Champs manquants');
      const seed = hexToBytes(json.secret_key_hex);
      if (seed.length !== 32) throw new Error('Clé secrète invalide (doit faire 32 octets)');
      const kp = nacl.sign.keyPair.fromSeed(seed);
      wallet = { address: json.address, secretKey64: kp.secretKey, publicKey32: kp.publicKey };
      document.getElementById('wallet-err').textContent = '';
      showWallet();
    } catch (err) {
      document.getElementById('wallet-err').textContent = 'Erreur : ' + err.message;
    }
    input.value = '';
  };
  reader.readAsText(file);
}

function showWallet() {
  document.querySelectorAll('.wallet-loaded').forEach(el => el.style.display = 'block');
  document.getElementById('w-addr').textContent = wallet.address;
  refreshWalletBalance();
}

async function refreshWalletBalance() {
  if (!wallet) return;
  try {
    const a = await (await fetch(`${BASE}/account/${wallet.address}`)).json();
    document.getElementById('w-bal').textContent = a.balance;
    document.getElementById('w-nonce').textContent = `Nonce: ${a.nonce}`;
    wallet.nonce = a.nonce;
  } catch {
    document.getElementById('w-bal').textContent = 'Compte introuvable';
    wallet.nonce = 0;
  }
}

function clearWallet() {
  wallet = null;
  document.querySelectorAll('.wallet-loaded').forEach(el => el.style.display = 'none');
  document.getElementById('wallet-file').value = '';
}

// ─── Tab switching ─────────────────────────────────────────────────────────────
function switchTab(tab) {
  activeTab = tab;
  ['transfer', 'stake', 'unstake'].forEach(t => {
    document.getElementById('tab-' + t).className = 'tab' + (t === tab ? ' on' : '');
    document.getElementById('form-' + t).style.display = t === tab ? '' : 'none';
  });
  document.getElementById('action-result').innerHTML = '';
  updateFee();
}

// ─── Fee hint ────────────────────────────────────────────────────────────────
function updateFee() {
  const ids = { transfer: 'tx-amount', stake: 'stake-amount', unstake: 'unstake-amount' };
  const el = document.getElementById(ids[activeTab]);
  const display = document.getElementById('fee-display-' + activeTab);
  if (!el || !el.value) { display.innerHTML = 'Fee : —'; return; }
  try {
    const atoms = parseVinx(el.value);
    const fee = calcFee(atoms);
    display.innerHTML = `Fee estimé : <span>${fmtVinx(fee)}</span>`;
  } catch { display.innerHTML = 'Fee : —'; }
}

// ─── Send transaction ─────────────────────────────────────────────────────────
async function sendTx(txType) {
  if (!wallet) return;
  const result = document.getElementById('action-result');
  result.innerHTML = '';

  // Get amount
  const amountIds = { Transfer: 'tx-amount', Stake: 'stake-amount', Unstake: 'unstake-amount' };
  const amountStr = document.getElementById(amountIds[txType]).value.trim();
  if (!amountStr) { result.innerHTML = `<p class="msg err">Entrez un montant.</p>`; return; }

  let amountAtoms;
  try { amountAtoms = parseVinx(amountStr); } catch { result.innerHTML = `<p class="msg err">Montant invalide.</p>`; return; }
  if (amountAtoms <= 0n) { result.innerHTML = `<p class="msg err">Le montant doit être positif.</p>`; return; }

  // Get destination
  let to = wallet.address; // stake/unstake → self
  if (txType === 'Transfer') {
    to = document.getElementById('tx-to').value.trim();
    if (!to.startsWith('vinx1')) { result.innerHTML = `<p class="msg err">Adresse destinataire invalide (doit commencer par vinx1).</p>`; return; }
  }

  const feeAtoms = calcFee(amountAtoms);

  // Refresh nonce
  result.innerHTML = `<p class="msg">Signature en cours…</p>`;
  try {
    const acc = await (await fetch(`${BASE}/account/${wallet.address}`)).json();
    wallet.nonce = acc.nonce ?? 0;
  } catch { wallet.nonce = wallet.nonce ?? 0; }

  const nonce = wallet.nonce;

  // Build signing bytes (matches Rust signing_bytes())
  const discriminants = { Transfer: 0x01, Stake: 0x02, Unstake: 0x03 };
  const enc = new TextEncoder();
  const fromB = enc.encode(wallet.address);
  const toB = enc.encode(to);
  const sigBytes = new Uint8Array(1 + 1 + fromB.length + 1 + toB.length + 16 + 16 + 8);
  let i = 0;
  sigBytes[i++] = discriminants[txType];
  sigBytes[i++] = fromB.length; sigBytes.set(fromB, i); i += fromB.length;
  sigBytes[i++] = toB.length;   sigBytes.set(toB, i);   i += toB.length;
  sigBytes.set(bigIntTo16BE(amountAtoms), i); i += 16;
  sigBytes.set(bigIntTo16BE(feeAtoms), i);    i += 16;
  sigBytes.set(bigIntTo8BE(BigInt(nonce)), i);

  const signature = nacl.sign.detached(sigBytes, wallet.secretKey64);

  // Build JSON manually — Amount is u128 → raw integer in JSON (not quoted)
  const pubKeyArr = '[' + Array.from(wallet.publicKey32).join(',') + ']';
  const sigHex = bytesToHex(signature);
  const body = `{"tx_type":"${txType}","from":"${wallet.address}","to":"${to}","amount":${amountAtoms},"fee":${feeAtoms},"nonce":${nonce},"pub_key":${pubKeyArr},"signature":"${sigHex}"}`;

  try {
    const resp = await fetch(BASE + '/tx/submit', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body,
    });
    const json = await resp.json();
    if (!resp.ok) throw new Error(json.error || `HTTP ${resp.status}`);
    result.innerHTML = `
      <div style="margin-top:12px;background:rgba(63,185,80,.08);border:1px solid rgba(63,185,80,.3);border-radius:6px;padding:12px 16px">
        <div style="color:var(--green);font-weight:700;margin-bottom:6px">✓ Transaction acceptée</div>
        <div style="font-family:'Courier New',monospace;font-size:12px;color:var(--muted);word-break:break-all">${json.tx_hash}</div>
      </div>`;
    refreshWalletBalance();
  } catch (e) {
    result.innerHTML = `<p class="msg err">Erreur : ${e.message}</p>`;
  }
}

// ─── Account lookup ───────────────────────────────────────────────────────────
async function lookupAccount() {
  const addr = document.getElementById('acc-in').value.trim();
  const out = document.getElementById('acc-result');
  if (!addr) return;
  out.innerHTML = `<p class="msg">Chargement…</p>`;
  try {
    const a = await (await fetch(`${BASE}/account/${addr}`)).json();
    out.innerHTML = `
      <div class="rg">
        <div class="ri full"><div class="l">Adresse</div><div class="v mono">${a.address}</div></div>
        <div class="ri"><div class="l">Solde</div><div class="v">${a.balance}</div></div>
        <div class="ri"><div class="l">Staké</div><div class="v">${a.staked}</div></div>
        <div class="ri"><div class="l">Nonce</div><div class="v">${a.nonce}</div></div>
        <div class="ri"><div class="l">Statut</div><div class="v">${a.frozen ? '<span class="tag r">Gelé</span>' : '<span class="tag g">Actif</span>'}</div></div>
      </div>`;
  } catch (e) {
    out.innerHTML = `<p class="msg err">Erreur : ${e.message}</p>`;
  }
}
document.getElementById('acc-in').addEventListener('keydown', e => { if (e.key==='Enter') lookupAccount(); });

// ─── Block explorer ───────────────────────────────────────────────────────────
const txTag = t => { const m={Transfer:'b',Emission:'g',Stake:'o',Unstake:'o'}; return `<span class="tag ${m[t]||'b'}">${t}</span>`; };
const shortA = a => a && a.length>16 ? a.slice(0,10)+'…'+a.slice(-6) : a;

async function lookupBlock() {
  const v = document.getElementById('blk-in').value;
  if (v === '') return;
  const h = parseInt(v, 10);
  if (isNaN(h)) return;
  blockCursor = h;
  document.getElementById('btn-prev').disabled = h <= 0;
  const out = document.getElementById('blk-result');
  out.innerHTML = `<p class="msg">Chargement…</p>`;
  try {
    const b = await (await fetch(`${BASE}/block/${h}`)).json();
    const txs = b.transactions.length === 0
      ? `<p class="msg" style="margin-top:12px">Aucune transaction.</p>`
      : `<div class="tx-list">${b.transactions.map(tx=>`
          <div class="tx-row">
            <div>${txTag(tx.tx_type)}</div>
            <div>
              <div style="font-family:'Courier New',monospace;font-size:11px;color:var(--muted)">${tx.hash.slice(0,24)}…</div>
              <div style="font-size:12px;color:var(--muted);margin-top:2px">
                <span title="${tx.from}">${shortA(tx.from)}</span> → <span title="${tx.to}">${shortA(tx.to)}</span>
              </div>
            </div>
            <div style="text-align:right">
              <div style="font-weight:600">${tx.amount}</div>
              <div style="color:var(--muted);font-size:11px">fee ${tx.fee}</div>
            </div>
          </div>`).join('')}
        </div>`;
    out.innerHTML = `
      <div class="rg" style="margin-top:16px">
        <div class="ri"><div class="l">Hauteur</div><div class="v">${b.height}</div></div>
        <div class="ri"><div class="l">Timestamp</div><div class="v">${new Date(b.timestamp*1000).toLocaleString()}</div></div>
        <div class="ri"><div class="l">Transactions</div><div class="v">${b.tx_count}</div></div>
        <div class="ri"><div class="l">Validateur</div><div class="v mono" title="${b.validator}">${shortA(b.validator)}</div></div>
        <div class="ri full"><div class="l">Hash</div><div class="v mono">${b.hash}</div></div>
        <div class="ri full"><div class="l">Hash précédent</div><div class="v mono">${b.prev_hash}</div></div>
      </div>${txs}`;
  } catch (e) {
    out.innerHTML = `<p class="msg err">Erreur : ${e.message}</p>`;
  }
}

function prevBlock() { if (blockCursor !== null && blockCursor > 0) { document.getElementById('blk-in').value = blockCursor - 1; lookupBlock(); } }
function nextBlock() { const n = blockCursor === null ? 0 : blockCursor + 1; document.getElementById('blk-in').value = n; lookupBlock(); }
async function goLatest() {
  try { const h = await (await fetch(BASE+'/chain/height')).json(); document.getElementById('blk-in').value = h.height; lookupBlock(); } catch {}
}
document.getElementById('blk-in').addEventListener('keydown', e => { if (e.key==='Enter') lookupBlock(); });
</script>
</body>
</html>"####;
