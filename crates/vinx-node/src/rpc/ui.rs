use axum::response::Html;

pub async fn index() -> Html<&'static str> {
    Html(HTML)
}

/// Admin console (`GET /admin`): dashboard + governance actions signed in-browser
/// with the admin key. Read-only until a key matching the on-chain admin loads.
pub async fn admin() -> Html<&'static str> {
    Html(ADMIN_HTML)
}

const HTML: &str = r####"<!DOCTYPE html>
<html lang="fr">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>VinX Ledger — Explorateur</title>
<script src="https://cdn.jsdelivr.net/npm/tweetnacl@1.0.3/nacl-fast.min.js"></script>
<style>
  :root {
    --bg:#0d1117;--surface:#161b22;--surface2:#21262d;--border:#30363d;
    --text:#e6edf3;--muted:#8b949e;--accent:#58a6ff;--green:#3fb950;
    --red:#f85149;--orange:#d29922;--purple:#bc8cff;--radius:8px;
  }
  *{box-sizing:border-box;margin:0;padding:0}
  body{background:var(--bg);color:var(--text);font-family:'Segoe UI',system-ui,sans-serif;font-size:14px}

  header{background:var(--surface);border-bottom:1px solid var(--border);padding:12px 24px;display:flex;align-items:center;gap:12px;position:sticky;top:0;z-index:10}
  .logo{font-size:18px;font-weight:700;color:var(--accent);white-space:nowrap}
  .logo span{color:var(--muted);font-weight:400;font-size:12px;margin-left:6px}
  .search-wrap{flex:1;max-width:480px;margin:0 auto}
  .search-wrap input{width:100%;background:var(--bg);border:1px solid var(--border);border-radius:6px;padding:7px 12px;color:var(--text);font-size:13px;outline:none;font-family:monospace}
  .search-wrap input:focus{border-color:var(--accent)}
  .dot{width:8px;height:8px;border-radius:50%;background:var(--green)}
  .dot.off{background:var(--red)}
  .dot-label{color:var(--muted);font-size:11px}

  main{max-width:1080px;margin:0 auto;padding:20px 16px;display:grid;gap:16px}
  .two-col{display:grid;grid-template-columns:1fr 1fr;gap:16px}

  .card{background:var(--surface);border:1px solid var(--border);border-radius:var(--radius);overflow:hidden}
  .card-header{padding:10px 18px;border-bottom:1px solid var(--border);font-weight:600;font-size:11px;color:var(--muted);text-transform:uppercase;letter-spacing:.6px;display:flex;align-items:center;gap:8px}
  .card-body{padding:18px}

  .stats{display:grid;grid-template-columns:repeat(3,1fr);gap:12px}
  .stats.s4{grid-template-columns:repeat(4,1fr)}
  .stats.s5{grid-template-columns:repeat(5,1fr)}
  .stat-v{font-size:24px;font-weight:700;color:var(--accent);font-variant-numeric:tabular-nums;line-height:1.2}
  .stat-v.sm{font-size:16px}
  .stat-l{color:var(--muted);font-size:11px;margin-top:3px}

  .row{display:flex;gap:8px;align-items:stretch}
  input[type=text],input[type=number]{flex:1;background:var(--bg);border:1px solid var(--border);border-radius:6px;padding:8px 12px;color:var(--text);font-size:13px;outline:none;transition:border-color .15s;font-family:inherit}
  input[type=text].mono{font-family:'Courier New',monospace;font-size:12px}
  input:focus{border-color:var(--accent)}
  input:disabled{opacity:.5}

  button{background:var(--accent);color:#fff;border:none;border-radius:6px;padding:8px 16px;font-size:13px;font-weight:600;cursor:pointer;white-space:nowrap;transition:opacity .15s}
  button:hover{opacity:.85}
  button:disabled{opacity:.35;cursor:default}
  button.ghost{background:var(--surface2);color:var(--text);border:1px solid var(--border)}
  button.ghost:hover{border-color:var(--accent);color:var(--accent);opacity:1}
  button.danger{background:transparent;color:var(--red);border:1px solid var(--border)}
  button.danger:hover{border-color:var(--red);opacity:1}
  button.green{background:#238636}
  button.green:hover{opacity:.85}

  .tag{display:inline-block;padding:2px 7px;border-radius:4px;font-size:11px;font-weight:600}
  .tag.g{background:rgba(63,185,80,.15);color:var(--green)}
  .tag.r{background:rgba(248,81,73,.15);color:var(--red)}
  .tag.b{background:rgba(88,166,255,.15);color:var(--accent)}
  .tag.o{background:rgba(210,153,34,.15);color:var(--orange)}
  .tag.p{background:rgba(188,140,255,.15);color:var(--purple)}

  .rg{display:grid;grid-template-columns:1fr 1fr;gap:8px;margin-top:12px}
  .ri{background:var(--bg);border:1px solid var(--border);border-radius:6px;padding:9px 13px}
  .ri .l{color:var(--muted);font-size:10px;text-transform:uppercase;letter-spacing:.5px;margin-bottom:2px}
  .ri .v{font-weight:600;word-break:break-all;font-size:13px}
  .ri .v.mono{font-family:'Courier New',monospace;font-size:11px}
  .ri.full{grid-column:1/-1}

  .wallet-loaded{display:none}
  .wallet-info{display:flex;align-items:center;gap:12px;flex-wrap:wrap;margin-top:12px;background:var(--bg);border:1px solid var(--border);border-radius:6px;padding:12px 14px}
  .wallet-addr{font-family:'Courier New',monospace;font-size:11px;flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
  .wallet-bal{color:var(--accent);font-weight:700;white-space:nowrap}
  .wallet-nonce{color:var(--muted);font-size:12px;white-space:nowrap}

  .tabs{display:flex;border-bottom:1px solid var(--border);margin-bottom:16px}
  .tab{padding:8px 14px;cursor:pointer;color:var(--muted);font-size:12px;border-bottom:2px solid transparent;margin-bottom:-1px;transition:all .15s;user-select:none}
  .tab:hover{color:var(--text)}
  .tab.on{color:var(--accent);border-bottom-color:var(--accent)}

  .fee-hint{font-size:12px;color:var(--muted);margin-top:6px}
  .fee-hint span{color:var(--orange);font-weight:600}

  .tx-list{margin-top:10px;display:grid;gap:6px}
  .tx-row{background:var(--bg);border:1px solid var(--border);border-radius:6px;padding:10px 14px;display:grid;grid-template-columns:auto 1fr auto;gap:10px;align-items:center}

  .bnav{display:flex;align-items:center;gap:8px;flex-wrap:wrap}
  .bnav input{max-width:100px}

  .msg{color:var(--muted);font-size:12px;margin-top:10px}
  .msg.err{color:var(--red)}
  .msg.ok{color:var(--green);font-weight:600}

  .sec-notice{background:rgba(210,153,34,.08);border:1px solid rgba(210,153,34,.25);border-radius:6px;padding:9px 13px;font-size:12px;color:var(--orange);margin-bottom:12px}

  .live{display:inline-flex;align-items:center;gap:4px;font-size:10px;color:var(--green);margin-left:auto}
  .live::before{content:'';width:5px;height:5px;border-radius:50%;background:var(--green);animation:pulse 2s infinite}
  @keyframes pulse{0%,100%{opacity:1}50%{opacity:.3}}

  canvas#fee-chart{display:block;border-radius:4px;background:var(--bg);width:100%}

  .pagination{display:flex;align-items:center;gap:8px;margin-top:12px}
  .pagination span{color:var(--muted);font-size:12px}

  .faucet-result{margin-top:12px}

  @media(max-width:720px){
    .two-col{grid-template-columns:1fr}
    .stats,.stats.s4,.stats.s5{grid-template-columns:1fr 1fr}
    .rg{grid-template-columns:1fr}
    .tx-row{grid-template-columns:1fr}
    .search-wrap{max-width:100%}
    header{flex-wrap:wrap}
  }
</style>
</head>
<body>

<header>
  <div class="logo">VinX Ledger <span>DEVNET</span></div>
  <div class="search-wrap">
    <input type="text" id="global-search" placeholder="Recherche : adresse vinx1… / hash de tx ou bloc…" onkeydown="if(event.key==='Enter')globalSearch()" />
  </div>
  <div class="dot off" id="hd"></div>
  <div class="dot-label" id="hs">…</div>
</header>

<main>

<!-- ── Ligne 1 : Réseau + Économie ──────────────────────────────────────── -->
<div class="two-col">

  <div class="card">
    <div class="card-header">Réseau <span class="live">Live</span></div>
    <div class="card-body">
      <div class="stats">
        <div><div class="stat-v" id="s-h">—</div><div class="stat-l">Hauteur</div></div>
        <div><div class="stat-v" id="s-m">—</div><div class="stat-l">Mempool</div></div>
        <div><div class="stat-v" id="s-s">—</div><div class="stat-l">Statut</div></div>
      </div>
      <div style="margin-top:16px">
        <div style="font-size:10px;color:var(--muted);text-transform:uppercase;letter-spacing:.5px;margin-bottom:6px">
          Base fee — 30 derniers blocs
        </div>
        <canvas id="fee-chart" height="56"></canvas>
        <div id="fee-range" style="font-size:10px;color:var(--muted);margin-top:4px;text-align:right"></div>
      </div>
    </div>
  </div>

  <div class="card">
    <div class="card-header">Économie <span class="live">Live</span></div>
    <div class="card-body">
      <div class="stats" style="grid-template-columns:1fr 1fr">
        <div><div class="stat-v sm" id="e-fee">—</div><div class="stat-l">Base fee (atoms)</div></div>
        <div><div class="stat-v sm" id="e-supply">—</div><div class="stat-l">En circulation</div></div>
        <div><div class="stat-v sm" id="e-remaining">—</div><div class="stat-l">Non encore émis</div></div>
      </div>
      <div style="margin-top:14px;padding-top:14px;border-top:1px solid var(--border)">
        <div style="font-size:10px;color:var(--muted);text-transform:uppercase;letter-spacing:.5px;margin-bottom:10px">Faucet testnet</div>
        <div class="row" style="margin-bottom:6px">
          <input type="text" class="mono" id="faucet-addr" placeholder="Adresse destinataire vinx1…" />
          <button class="green" onclick="requestFaucet()">Obtenir</button>
        </div>
        <div id="faucet-result" class="faucet-result"></div>
      </div>
    </div>
  </div>

</div>

<!-- ── Wallet ────────────────────────────────────────────────────────────── -->
<div class="card">
  <div class="card-header">Wallet</div>
  <div class="card-body">
    <div class="sec-notice">Votre clé secrète ne quitte jamais le navigateur — la signature est effectuée localement.</div>
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

<!-- ── Actions ───────────────────────────────────────────────────────────── -->
<div class="card wallet-loaded" id="actions-card">
  <div class="card-header">Envoyer / Staker</div>
  <div class="card-body">
    <div class="tabs">
      <div class="tab on" id="tab-transfer" onclick="switchTab('transfer')">Transfer</div>
      <div class="tab" id="tab-stake" onclick="switchTab('stake')">Staker</div>
      <div class="tab" id="tab-unstake" onclick="switchTab('unstake')">Unstaker</div>
    </div>
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
    <div id="form-stake" style="display:none">
      <div class="row">
        <input type="number" id="stake-amount" placeholder="Montant à staker (VINX)" min="0" step="any" oninput="updateFee()" />
        <button onclick="sendTx('Stake')">Staker</button>
      </div>
      <div class="fee-hint" id="fee-display-stake">Fee : —</div>
    </div>
    <div id="form-unstake" style="display:none">
      <div class="row">
        <input type="number" id="unstake-amount" placeholder="Montant à libérer (VINX)" min="0" step="any" oninput="updateFee()" />
        <button onclick="sendTx('Unstake')">Unstaker</button>
      </div>
      <div class="fee-hint" id="fee-display-unstake">Fee : —</div>
    </div>
    <div id="action-result" style="margin-top:12px"></div>
  </div>
</div>

<!-- ── Ligne 2 : Compte + Bloc ──────────────────────────────────────────── -->
<div class="two-col">

  <!-- Account -->
  <div class="card">
    <div class="card-header">Compte</div>
    <div class="card-body">
      <div class="row">
        <input type="text" class="mono" id="acc-in" placeholder="vinx1…" />
        <button onclick="lookupAccount()">Chercher</button>
      </div>
      <div id="acc-result"></div>
      <div id="acc-txs"></div>
    </div>
  </div>

  <!-- Block explorer -->
  <div class="card">
    <div class="card-header">Bloc</div>
    <div class="card-body">
      <div class="bnav">
        <button class="ghost" onclick="prevBlock()" id="btn-prev">←</button>
        <input type="number" id="blk-in" placeholder="Hauteur" min="0" onchange="lookupBlock()" />
        <button class="ghost" onclick="nextBlock()">→</button>
        <button onclick="goLatest()">Dernier</button>
      </div>
      <div id="blk-result"></div>
    </div>
  </div>

</div>

<!-- ── Ligne 3 : TX + Validateurs + Protocole ───────────────────────────── -->
<div class="two-col">

  <div class="card">
    <div class="card-header">Transaction</div>
    <div class="card-body">
      <div class="row">
        <input type="text" class="mono" id="tx-hash-in" placeholder="Hash hexadécimal…" />
        <button onclick="lookupTx()">Chercher</button>
      </div>
      <div id="tx-result"></div>
    </div>
  </div>

  <div class="card">
    <div class="card-header">Validateurs <span class="live" id="vs-live" style="display:none">Live</span></div>
    <div class="card-body">
      <div id="vs-result"><p class="msg">Chargement…</p></div>
    </div>
  </div>

</div>

<div class="card">
  <div class="card-header">Protocole</div>
  <div class="card-body">
    <div id="proto-result"><p class="msg">Chargement…</p></div>
  </div>
</div>

</main>

<script>
// ─── Constants ────────────────────────────────────────────────────────────────
const BASE = window.location.origin;
const DECIMAL = 1_000_000_000_000_000_000n;
const FEE_FLOOR = DECIMAL / 100n;

// ─── State ────────────────────────────────────────────────────────────────────
let netHeight = 0;
let blockCursor = null;
let wallet = null;
let activeTab = 'transfer';
let feeHistory = []; // [{height, fee}]
let accTxOffset = 0;
const ACC_TX_PAGE = 10;

// ─── Utilities ────────────────────────────────────────────────────────────────
const hexToBytes = h => Uint8Array.from(h.match(/.{2}/g), x => parseInt(x, 16));
const bytesToHex = b => Array.from(b, x => x.toString(16).padStart(2,'0')).join('');

function parseVinx(s) {
  s = s.trim().replace(',', '.');
  const [w, f = ''] = s.split('.');
  const frac = (f + '0'.repeat(18)).slice(0, 18);
  return BigInt(w || 0) * DECIMAL + BigInt(frac);
}

function fmtVinx(atoms) {
  const a = BigInt(atoms);
  const w = a / DECIMAL;
  const c = (a % DECIMAL) / (DECIMAL / 100n);
  return `${w.toLocaleString()}.${String(c).padStart(2,'0')} VINX`;
}

function fmtAtoms(s) {
  try { return fmtVinx(BigInt(s)); } catch { return s; }
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
function u32To4BE(n) {
  const b = new Uint8Array(4);
  b[0] = (n >>> 24) & 0xff; b[1] = (n >>> 16) & 0xff; b[2] = (n >>> 8) & 0xff; b[3] = n & 0xff;
  return b;
}
// Decode a bech32 `vinx1...` address to its raw 20-byte payload — the canonical
// form the node signs and hashes over. Mirrors vinx-crypto's Address encoding.
const BECH32_CHARSET = 'qpzry9x8gf2tvdw0s3jn54khce6mua7l';
function bech32Decode20(addr) {
  const s = addr.toLowerCase();
  const pos = s.lastIndexOf('1');
  if (pos < 1) throw new Error('adresse bech32 invalide');
  const data = s.slice(pos + 1);
  const values = [];
  for (const ch of data) {
    const v = BECH32_CHARSET.indexOf(ch);
    if (v === -1) throw new Error('caractère bech32 invalide');
    values.push(v);
  }
  // Drop the 6-symbol checksum, then convert 5-bit groups to 8-bit bytes.
  const words = values.slice(0, values.length - 6);
  let acc = 0, bits = 0;
  const out = [];
  for (const w of words) {
    acc = (acc << 5) | w; bits += 5;
    while (bits >= 8) { bits -= 8; out.push((acc >> bits) & 0xff); }
  }
  if (out.length !== 20) throw new Error('charge utile bech32 != 20 octets');
  return new Uint8Array(out);
}

const txTag = t => {
  const m = { Transfer:'b', Stake:'o', Unstake:'o', AdminAction:'p', SlashValidator:'r', AddValidator:'g', RemoveValidator:'r', AnnounceUpgrade:'p' };
  return `<span class="tag ${m[t]||'b'}">${t}</span>`;
};
const shortA = a => a && a.length > 20 ? a.slice(0,10)+'…'+a.slice(-6) : (a||'');
const shortH = h => h && h.length > 16 ? h.slice(0,12)+'…' : (h||'');

// ─── Global search ────────────────────────────────────────────────────────────
async function globalSearch() {
  const q = document.getElementById('global-search').value.trim();
  if (!q) return;
  if (q.startsWith('vinx1')) {
    document.getElementById('acc-in').value = q;
    document.getElementById('acc-in').scrollIntoView({ behavior:'smooth', block:'center' });
    lookupAccount();
    return;
  }
  // Try as tx hash first, then block height
  if (/^[0-9a-fA-F]{10,}$/.test(q)) {
    try {
      const r = await fetch(`${BASE}/tx/${q}`);
      if (r.ok) {
        document.getElementById('tx-hash-in').value = q;
        document.getElementById('tx-hash-in').scrollIntoView({ behavior:'smooth', block:'center' });
        lookupTx();
        return;
      }
    } catch {}
  }
  const n = parseInt(q, 10);
  if (!isNaN(n)) {
    document.getElementById('blk-in').value = n;
    document.getElementById('blk-in').scrollIntoView({ behavior:'smooth', block:'center' });
    lookupBlock();
  }
}

// ─── Network status ───────────────────────────────────────────────────────────
async function refreshStatus() {
  try {
    const h = await (await fetch(BASE + '/health')).json();
    netHeight = h.height;
    document.getElementById('s-h').textContent = h.height.toLocaleString();
    document.getElementById('s-s').innerHTML = `<span class="tag g">${h.status}</span>`;
    document.getElementById('s-m').textContent = h.mempool_pending;
    document.getElementById('hd').className = 'dot';
    document.getElementById('hs').textContent = 'connecté';
  } catch {
    document.getElementById('s-h').textContent = '—';
    document.getElementById('s-s').innerHTML = `<span class="tag r">offline</span>`;
    document.getElementById('s-m').textContent = '—';
    document.getElementById('hd').className = 'dot off';
    document.getElementById('hs').textContent = 'hors-ligne';
  }
}

// ─── Economic stats ───────────────────────────────────────────────────────────
async function refreshEconStats() {
  try {
    const s = await (await fetch(BASE + '/network/stats')).json();
    document.getElementById('e-fee').textContent       = Number(BigInt(s.base_fee_atoms)).toLocaleString();
    document.getElementById('e-supply').textContent    = fmtAtoms(s.circulating_supply.split(' ')[0] + '000000000000000000').replace('.00 VINX','');
    document.getElementById('e-remaining').textContent = s.remaining_supply;
  } catch {}
}

// ─── Fee chart ────────────────────────────────────────────────────────────────
async function updateFeeChart() {
  try {
    const hResp = await (await fetch(BASE + '/chain/height')).json();
    const from  = Math.max(0, hResp.height - 29);
    const sync  = await (await fetch(`${BASE}/chain/sync?from=${from}&limit=30`)).json();
    feeHistory  = sync.blocks.map(b => ({ height: b.height, fee: b.base_fee }));
    drawFeeChart();
  } catch {}
}

function drawFeeChart() {
  const canvas = document.getElementById('fee-chart');
  if (!canvas || !feeHistory.length) return;
  const W = canvas.clientWidth || canvas.parentElement.clientWidth || 400;
  const H = 56;
  canvas.width  = W;
  canvas.height = H;
  const ctx = canvas.getContext('2d');

  const fees   = feeHistory.map(f => f.fee);
  const minFee = Math.min(...fees);
  const maxFee = Math.max(...fees);
  const range  = maxFee - minFee || 1;
  const n      = fees.length;
  const barW   = Math.max(2, Math.floor((W - n + 1) / n));
  const step   = barW + 1;

  ctx.clearRect(0, 0, W, H);

  for (let i = 0; i < n; i++) {
    const norm = (fees[i] - minFee) / range;
    const barH = Math.max(3, Math.round(norm * (H - 6)) + 3);
    const x    = i * step;
    const y    = H - barH;
    // Gradient: green (at floor) → orange (surge)
    const r = Math.round(63  + norm * (210 - 63));
    const g = Math.round(185 - norm * (185 - 153));
    const b = Math.round(80  - norm * (80  - 34));
    ctx.fillStyle = `rgb(${r},${g},${b})`;
    ctx.fillRect(x, y, barW, barH);
  }

  // Range label
  const label = minFee === maxFee
    ? `${minFee.toLocaleString()} atoms`
    : `${minFee.toLocaleString()} – ${maxFee.toLocaleString()} atoms`;
  document.getElementById('fee-range').textContent = label;
}

// ─── SSE ─────────────────────────────────────────────────────────────────────
(function startSSE() {
  let es;
  function connect() {
    es = new EventSource('/events');
    es.onmessage = e => {
      try {
        const evt = JSON.parse(e.data);
        if (evt.type === 'new_block') {
          netHeight = evt.height;
          document.getElementById('s-h').textContent = evt.height.toLocaleString();
          // Add to fee history (base_fee not in event, so refresh chart periodically)
          refreshStatus();
          refreshEconStats();
        }
      } catch {}
    };
    es.onerror = () => { es.close(); setTimeout(connect, 5000); };
  }
  connect();
})();

// Periodic refresh
setInterval(() => { refreshStatus(); refreshEconStats(); }, 15000);
setInterval(updateFeeChart, 30000);
refreshStatus();
refreshEconStats();
updateFeeChart();

// ─── Faucet ───────────────────────────────────────────────────────────────────
async function requestFaucet() {
  const addr = document.getElementById('faucet-addr').value.trim();
  const out  = document.getElementById('faucet-result');
  if (!addr) { out.innerHTML = `<p class="msg err">Entrez une adresse.</p>`; return; }
  if (!addr.startsWith('vinx1')) { out.innerHTML = `<p class="msg err">Adresse invalide (doit commencer par vinx1).</p>`; return; }
  out.innerHTML = `<p class="msg">Envoi en cours…</p>`;
  try {
    const resp = await fetch(BASE + '/faucet/request', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ address: addr }),
    });
    const json = await resp.json();
    if (!resp.ok) {
      out.innerHTML = `<p class="msg err">${json.error || `Erreur ${resp.status}`}</p>`;
      return;
    }
    out.innerHTML = `
      <div style="margin-top:8px;background:rgba(35,134,54,.12);border:1px solid rgba(63,185,80,.3);border-radius:6px;padding:10px 13px">
        <div style="color:var(--green);font-weight:700;font-size:12px;margin-bottom:4px">✓ Tokens envoyés</div>
        <div style="font-size:11px;color:var(--muted)">Montant : ${fmtAtoms(json.amount_atoms)}</div>
        <div style="font-family:monospace;font-size:10px;color:var(--muted);margin-top:3px;word-break:break-all">tx: ${json.tx_hash}</div>
      </div>`;
  } catch (e) {
    out.innerHTML = `<p class="msg err">Erreur réseau : ${e.message}</p>`;
  }
}

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
      if (seed.length !== 32) throw new Error('Clé invalide (32 octets requis)');
      const kp = nacl.sign.keyPair.fromSeed(seed);
      wallet = { address: json.address, secretKey64: kp.secretKey, publicKey32: kp.publicKey, chainId: 42 };
      // Learn the node's chain ID so signatures commit to the right network.
      fetch(`${BASE}/health`).then(r => r.json()).then(h => {
        if (wallet && typeof h.chain_id === 'number') wallet.chainId = h.chain_id;
      }).catch(() => {});
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
    const resp = await fetch(`${BASE}/account/${wallet.address}`);
    if (!resp.ok) {
      document.getElementById('w-bal').textContent = 'Compte vide';
      document.getElementById('w-nonce').textContent = 'Nonce: 0';
      wallet.nonce = 0; return;
    }
    const a = await resp.json();
    document.getElementById('w-bal').textContent = a.balance;
    document.getElementById('w-nonce').textContent = `Nonce: ${a.nonce}`;
    wallet.nonce = a.nonce;
  } catch { wallet.nonce = wallet.nonce ?? 0; }
}

function clearWallet() {
  wallet = null;
  document.querySelectorAll('.wallet-loaded').forEach(el => el.style.display = 'none');
}

// ─── Tab switching ────────────────────────────────────────────────────────────
function switchTab(tab) {
  activeTab = tab;
  ['transfer','stake','unstake'].forEach(t => {
    document.getElementById('tab-'+t).className = 'tab' + (t === tab ? ' on' : '');
    document.getElementById('form-'+t).style.display = t === tab ? '' : 'none';
  });
  document.getElementById('action-result').innerHTML = '';
  updateFee();
}

function updateFee() {
  const ids = { transfer:'tx-amount', stake:'stake-amount', unstake:'unstake-amount' };
  const el = document.getElementById(ids[activeTab]);
  const display = document.getElementById('fee-display-' + activeTab);
  if (!el || !el.value) { display.innerHTML = 'Fee : —'; return; }
  try {
    const atoms = parseVinx(el.value);
    display.innerHTML = `Fee estimé : <span>${fmtVinx(calcFee(atoms))}</span>`;
  } catch { display.innerHTML = 'Fee : —'; }
}

// ─── Send transaction ─────────────────────────────────────────────────────────
async function sendTx(txType) {
  if (!wallet) return;
  const result = document.getElementById('action-result');
  result.innerHTML = '';
  const amountIds = { Transfer:'tx-amount', Stake:'stake-amount', Unstake:'unstake-amount' };
  const amountStr = document.getElementById(amountIds[txType]).value.trim();
  if (!amountStr) { result.innerHTML = `<p class="msg err">Entrez un montant.</p>`; return; }
  let amountAtoms;
  try { amountAtoms = parseVinx(amountStr); } catch { result.innerHTML = `<p class="msg err">Montant invalide.</p>`; return; }
  if (amountAtoms <= 0n) { result.innerHTML = `<p class="msg err">Montant doit être positif.</p>`; return; }
  let to = wallet.address;
  if (txType === 'Transfer') {
    to = document.getElementById('tx-to').value.trim();
    if (!to.startsWith('vinx1')) { result.innerHTML = `<p class="msg err">Adresse invalide.</p>`; return; }
  }
  const feeAtoms = calcFee(amountAtoms);
  result.innerHTML = `<p class="msg">Signature…</p>`;
  try {
    const acc = await (await fetch(`${BASE}/account/${wallet.address}`)).json();
    wallet.nonce = acc.nonce ?? 0;
  } catch { wallet.nonce = wallet.nonce ?? 0; }
  const nonce = wallet.nonce;
  const chainId = wallet.chainId ?? 42;
  const discriminants = { Transfer:0x01, Stake:0x02, Unstake:0x03 };
  // Canonical signing bytes — must match vinx-core Transaction::signing_bytes():
  // disc(1) ‖ from(20) ‖ to(20) ‖ amount(16 BE) ‖ fee(16 BE) ‖ nonce(8 BE)
  // ‖ chain_id(4 BE) ‖ expiry(1=0x00) ‖ payload(empty) ‖ sponsor(1=0x00)
  let fromB, toB;
  try { fromB = bech32Decode20(wallet.address); toB = bech32Decode20(to); }
  catch (e) { result.innerHTML = `<p class="msg err">Adresse invalide : ${e.message}</p>`; return; }
  const sigBytes = new Uint8Array(1+20+20+16+16+8+4+1+1);
  let i = 0;
  sigBytes[i++] = discriminants[txType];
  sigBytes.set(fromB, i); i += 20;
  sigBytes.set(toB, i);   i += 20;
  sigBytes.set(bigIntTo16BE(amountAtoms), i); i += 16;
  sigBytes.set(bigIntTo16BE(feeAtoms), i);    i += 16;
  sigBytes.set(bigIntTo8BE(BigInt(nonce)), i); i += 8;
  sigBytes.set(u32To4BE(chainId), i);          i += 4;
  sigBytes[i++] = 0x00; // expires_at_height: None
  sigBytes[i++] = 0x00; // sponsor: None
  const signature = nacl.sign.detached(sigBytes, wallet.secretKey64);
  const pubKeyArr = '['+Array.from(wallet.publicKey32).join(',')+']';
  const sigHex = bytesToHex(signature);
  const body = `{"tx_type":"${txType}","from":"${wallet.address}","to":"${to}","amount":${amountAtoms},"fee":${feeAtoms},"nonce":${nonce},"chain_id":${chainId},"payload":[],"pub_key":${pubKeyArr},"signature":"${sigHex}"}`;
  try {
    const resp = await fetch(BASE+'/tx/submit', { method:'POST', headers:{'Content-Type':'application/json'}, body });
    const json = await resp.json();
    if (!resp.ok) throw new Error(json.error || `HTTP ${resp.status}`);
    result.innerHTML = `
      <div style="background:rgba(63,185,80,.08);border:1px solid rgba(63,185,80,.3);border-radius:6px;padding:10px 14px">
        <div style="color:var(--green);font-weight:700;margin-bottom:4px">✓ Transaction acceptée</div>
        <div style="font-family:monospace;font-size:11px;color:var(--muted);word-break:break-all">${json.tx_hash}</div>
      </div>`;
    refreshWalletBalance();
  } catch (e) { result.innerHTML = `<p class="msg err">Erreur : ${e.message}</p>`; }
}

// ─── Account lookup + TX history ─────────────────────────────────────────────
async function lookupAccount() {
  const addr = document.getElementById('acc-in').value.trim();
  const out  = document.getElementById('acc-result');
  const txOut = document.getElementById('acc-txs');
  if (!addr) return;
  accTxOffset = 0;
  out.innerHTML = `<p class="msg">Chargement…</p>`;
  txOut.innerHTML = '';
  try {
    const resp = await fetch(`${BASE}/account/${addr}`);
    if (!resp.ok) { out.innerHTML = `<p class="msg err">Compte introuvable.</p>`; return; }
    const a = await resp.json();
    out.innerHTML = `
      <div class="rg">
        <div class="ri full"><div class="l">Adresse</div><div class="v mono">${a.address}</div></div>
        <div class="ri"><div class="l">Solde</div><div class="v">${a.balance}</div></div>
        <div class="ri"><div class="l">Staké</div><div class="v">${a.staked}</div></div>
        <div class="ri"><div class="l">Nonce</div><div class="v">${a.nonce}</div></div>
      </div>`;
    loadAccTxs(addr);
  } catch (e) { out.innerHTML = `<p class="msg err">${e.message}</p>`; }
}

async function loadAccTxs(addr, offset = 0) {
  const txOut = document.getElementById('acc-txs');
  txOut.innerHTML = `<p class="msg" style="margin-top:10px">Transactions…</p>`;
  try {
    const r = await fetch(`${BASE}/account/${addr}/txs?limit=${ACC_TX_PAGE}&offset=${offset}`);
    if (!r.ok) { txOut.innerHTML = ''; return; }
    const data = await r.json();
    if (!data.txs.length && offset === 0) { txOut.innerHTML = `<p class="msg" style="margin-top:8px">Aucune transaction.</p>`; return; }
    const txsHtml = data.txs.map(tx => `
      <div class="tx-row" onclick="document.getElementById('tx-hash-in').value='${tx.hash}';lookupTx()" style="cursor:pointer">
        <div>${txTag(tx.tx_type)}</div>
        <div>
          <div style="font-family:monospace;font-size:10px;color:var(--muted)">${shortH(tx.hash)}</div>
          <div style="font-size:11px;color:var(--muted);margin-top:1px">
            Bloc #${tx.block_height} — <span title="${tx.from}">${shortA(tx.from)}</span> → <span title="${tx.to}">${shortA(tx.to)}</span>
          </div>
        </div>
        <div style="text-align:right;white-space:nowrap">
          <div style="font-weight:600;font-size:12px">${tx.amount}</div>
          <div style="color:var(--muted);font-size:10px">fee ${tx.fee}</div>
        </div>
      </div>`).join('');
    const paginationHtml = `
      <div class="pagination">
        ${offset > 0 ? `<button class="ghost" onclick="loadAccTxs('${addr}', ${offset - ACC_TX_PAGE})" style="padding:5px 10px;font-size:12px">← Précédent</button>` : ''}
        <span>${offset + 1}–${Math.min(offset + ACC_TX_PAGE, data.total)} sur ${data.total}</span>
        ${offset + ACC_TX_PAGE < data.total ? `<button class="ghost" onclick="loadAccTxs('${addr}', ${offset + ACC_TX_PAGE})" style="padding:5px 10px;font-size:12px">Suivant →</button>` : ''}
      </div>`;
    txOut.innerHTML = `
      <div style="font-size:10px;color:var(--muted);text-transform:uppercase;letter-spacing:.5px;margin-top:14px;margin-bottom:6px">Transactions récentes</div>
      <div class="tx-list">${txsHtml}</div>
      ${paginationHtml}`;
  } catch { txOut.innerHTML = ''; }
}

document.getElementById('acc-in').addEventListener('keydown', e => { if (e.key==='Enter') lookupAccount(); });

// ─── Block explorer ───────────────────────────────────────────────────────────
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
    const resp = await fetch(`${BASE}/block/${h}`);
    if (!resp.ok) { out.innerHTML = `<p class="msg err">Bloc introuvable.</p>`; return; }
    const b = await resp.json();
    const txsHtml = b.transactions.length === 0
      ? `<p class="msg" style="margin-top:10px">Aucune transaction.</p>`
      : `<div class="tx-list" style="margin-top:10px">${b.transactions.map(tx=>`
          <div class="tx-row" onclick="document.getElementById('tx-hash-in').value='${tx.hash}';lookupTx()" style="cursor:pointer">
            <div>${txTag(tx.tx_type)}</div>
            <div>
              <div style="font-family:monospace;font-size:10px;color:var(--muted)">${shortH(tx.hash)}</div>
              <div style="font-size:11px;color:var(--muted);margin-top:1px">
                <span title="${tx.from}">${shortA(tx.from)}</span> → <span title="${tx.to}">${shortA(tx.to)}</span>
              </div>
            </div>
            <div style="text-align:right;white-space:nowrap">
              <div style="font-weight:600;font-size:12px">${tx.amount}</div>
              <div style="color:var(--muted);font-size:10px">fee ${tx.fee}</div>
            </div>
          </div>`).join('')}
        </div>`;
    out.innerHTML = `
      <div class="rg" style="margin-top:12px">
        <div class="ri"><div class="l">Hauteur</div><div class="v">${b.height.toLocaleString()}</div></div>
        <div class="ri"><div class="l">Timestamp</div><div class="v">${new Date(b.timestamp*1000).toLocaleString()}</div></div>
        <div class="ri"><div class="l">Transactions</div><div class="v">${b.tx_count}</div></div>
        <div class="ri"><div class="l">Base fee</div><div class="v">${b.base_fee.toLocaleString()} atoms</div></div>
        <div class="ri"><div class="l">Signatures</div><div class="v">${b.signatures_count??'—'} ${b.finalized?'<span class="tag g">finalisé</span>':'<span class="tag o">en attente</span>'}</div></div>
        <div class="ri"><div class="l">Validateur</div><div class="v mono" title="${b.validator}">${shortA(b.validator)}</div></div>
        <div class="ri full"><div class="l">Hash</div><div class="v mono">${b.hash}</div></div>
        <div class="ri full"><div class="l">Prev Hash</div><div class="v mono">${b.prev_hash}</div></div>
        <div class="ri full"><div class="l">State Root</div><div class="v mono">${b.state_root??'—'}</div></div>
      </div>${txsHtml}`;
  } catch (e) { out.innerHTML = `<p class="msg err">${e.message}</p>`; }
}

function prevBlock() { if (blockCursor !== null && blockCursor > 0) { document.getElementById('blk-in').value = blockCursor-1; lookupBlock(); } }
function nextBlock() { const n = blockCursor===null ? 0 : blockCursor+1; document.getElementById('blk-in').value = n; lookupBlock(); }
async function goLatest() {
  try { const r = await (await fetch(BASE+'/chain/height')).json(); document.getElementById('blk-in').value = r.height; lookupBlock(); } catch {}
}
document.getElementById('blk-in').addEventListener('keydown', e => { if (e.key==='Enter') lookupBlock(); });

// ─── Transaction lookup ───────────────────────────────────────────────────────
async function lookupTx() {
  const hash = document.getElementById('tx-hash-in').value.trim();
  const out  = document.getElementById('tx-result');
  if (!hash) return;
  out.innerHTML = `<p class="msg">Chargement…</p>`;
  try {
    const resp = await fetch(`${BASE}/tx/${hash}`);
    if (!resp.ok) { out.innerHTML = `<p class="msg err">Transaction introuvable.</p>`; return; }
    const tx = await resp.json();
    out.innerHTML = `
      <div class="rg" style="margin-top:12px">
        <div class="ri"><div class="l">Type</div><div class="v">${txTag(tx.tx_type)}</div></div>
        <div class="ri"><div class="l">Bloc</div><div class="v"><a href="#" onclick="document.getElementById('blk-in').value=${tx.block_height};lookupBlock();return false" style="color:var(--accent)">#${tx.block_height}</a></div></div>
        <div class="ri"><div class="l">Montant</div><div class="v">${tx.amount}</div></div>
        <div class="ri"><div class="l">Fee</div><div class="v">${tx.fee}</div></div>
        <div class="ri"><div class="l">Nonce</div><div class="v">${tx.nonce}</div></div>
        <div class="ri full"><div class="l">Hash</div><div class="v mono">${tx.hash}</div></div>
        <div class="ri full"><div class="l">De</div><div class="v mono"><a href="#" onclick="document.getElementById('acc-in').value='${tx.from}';lookupAccount();return false" style="color:var(--accent)">${tx.from}</a></div></div>
        <div class="ri full"><div class="l">Vers</div><div class="v mono"><a href="#" onclick="document.getElementById('acc-in').value='${tx.to}';lookupAccount();return false" style="color:var(--accent)">${tx.to}</a></div></div>
        <div class="ri full"><div class="l">Hash du bloc</div><div class="v mono">${tx.block_hash}</div></div>
      </div>`;
  } catch (e) { out.innerHTML = `<p class="msg err">${e.message}</p>`; }
}
document.getElementById('tx-hash-in').addEventListener('keydown', e => { if (e.key==='Enter') lookupTx(); });

// ─── Validators ───────────────────────────────────────────────────────────────
async function refreshValidators() {
  const out = document.getElementById('vs-result');
  try {
    const vs = await (await fetch(BASE+'/validators')).json();
    document.getElementById('vs-live').style.display = '';
    const list = vs.validators.map((v,i) =>
      `<div class="ri full"><div class="l">Validateur ${i+1}</div><div class="v mono">${v}</div></div>`
    ).join('');
    out.innerHTML = `
      <div class="rg">
        <div class="ri"><div class="l">Total</div><div class="v">${vs.count}</div></div>
        <div class="ri"><div class="l">Quorum</div><div class="v">${vs.quorum}/${vs.count}</div></div>
        ${list}
      </div>`;
  } catch { out.innerHTML = `<p class="msg err">Impossible de charger.</p>`; }
}
refreshValidators();

// ─── Protocol ─────────────────────────────────────────────────────────────────
async function refreshProtocol() {
  const out = document.getElementById('proto-result');
  try {
    const p = await (await fetch(BASE+'/protocol/version')).json();
    const upg = p.pending_upgrade
      ? `<div class="ri"><div class="l">Upgrade prévu</div><div class="v"><span class="tag o">${p.pending_upgrade.version}</span> le ${new Date(p.pending_upgrade.activation_ts*1000).toLocaleString()}</div></div>`
      : `<div class="ri"><div class="l">Upgrade prévu</div><div class="v"><span class="tag g">aucun</span></div></div>`;
    out.innerHTML = `
      <div class="rg">
        <div class="ri"><div class="l">Version</div><div class="v"><span class="tag b">${p.current_version}</span></div></div>
        ${upg}
      </div>`;
  } catch { out.innerHTML = `<p class="msg err">Impossible de charger.</p>`; }
}
refreshProtocol();

// ─── Resize handler for chart ─────────────────────────────────────────────────
window.addEventListener('resize', drawFeeChart);
</script>
</body>
</html>"####;

const ADMIN_HTML: &str = r####"<!DOCTYPE html>
<html lang="fr">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>VinX Ledger — Console Admin</title>
<script src="https://cdn.jsdelivr.net/npm/tweetnacl@1.0.3/nacl-fast.min.js"></script>
<style>
  :root{
    --bg:#0d1117;--surface:#161b22;--surface2:#21262d;--border:#30363d;
    --text:#e6edf3;--muted:#8b949e;--accent:#58a6ff;--green:#3fb950;
    --red:#f85149;--orange:#d29922;--purple:#bc8cff;--radius:8px;
  }
  *{box-sizing:border-box;margin:0;padding:0}
  body{background:var(--bg);color:var(--text);font-family:'Segoe UI',system-ui,sans-serif;font-size:14px}
  header{background:var(--surface);border-bottom:1px solid var(--border);padding:12px 24px;display:flex;align-items:center;gap:12px;position:sticky;top:0;z-index:10}
  .logo{font-size:18px;font-weight:700;color:var(--accent);white-space:nowrap}
  .logo span{color:var(--muted);font-weight:400;font-size:12px;margin-left:6px}
  header a{color:var(--muted);font-size:12px;text-decoration:none;margin-left:auto}
  header a:hover{color:var(--accent)}
  main{max-width:960px;margin:0 auto;padding:20px 16px;display:grid;gap:16px}
  .card{background:var(--surface);border:1px solid var(--border);border-radius:var(--radius);overflow:hidden}
  .card-header{padding:10px 18px;border-bottom:1px solid var(--border);font-weight:600;font-size:11px;color:var(--muted);text-transform:uppercase;letter-spacing:.6px;display:flex;align-items:center;gap:8px}
  .card-body{padding:18px}
  .stats{display:grid;grid-template-columns:repeat(3,1fr);gap:12px}
  .stat-v{font-size:20px;font-weight:700;color:var(--accent);font-variant-numeric:tabular-nums;line-height:1.2;word-break:break-word}
  .stat-v.sm{font-size:15px}
  .stat-l{color:var(--muted);font-size:11px;margin-top:3px}
  .row{display:flex;gap:8px;align-items:stretch;flex-wrap:wrap;margin-top:10px}
  input[type=text],input[type=number]{flex:1;min-width:120px;background:var(--bg);border:1px solid var(--border);border-radius:6px;padding:8px 12px;color:var(--text);font-size:13px;outline:none;font-family:inherit}
  input.mono{font-family:'Courier New',monospace;font-size:12px}
  input:focus{border-color:var(--accent)}
  button{background:var(--accent);color:#fff;border:none;border-radius:6px;padding:8px 16px;font-size:13px;font-weight:600;cursor:pointer;white-space:nowrap;transition:opacity .15s}
  button:hover{opacity:.85}
  button:disabled{opacity:.35;cursor:not-allowed}
  button.ghost{background:var(--surface2);color:var(--text);border:1px solid var(--border)}
  button.ghost:hover{border-color:var(--accent);color:var(--accent);opacity:1}
  button.danger{background:transparent;color:var(--red);border:1px solid var(--border)}
  button.danger:hover{border-color:var(--red);opacity:1}
  button.sm{padding:5px 10px;font-size:12px}
  .tag{display:inline-block;padding:2px 7px;border-radius:4px;font-size:11px;font-weight:600}
  .tag.g{background:rgba(63,185,80,.15);color:var(--green)}
  .tag.r{background:rgba(248,81,73,.15);color:var(--red)}
  .tag.b{background:rgba(88,166,255,.15);color:var(--accent)}
  .tag.o{background:rgba(210,153,34,.15);color:var(--orange)}
  .tag.muted{background:var(--surface2);color:var(--muted)}
  .list{display:grid;gap:6px;margin-top:10px}
  .item{background:var(--bg);border:1px solid var(--border);border-radius:6px;padding:10px 14px;display:flex;align-items:center;gap:10px;flex-wrap:wrap}
  .item .addr{font-family:'Courier New',monospace;font-size:12px;flex:1;min-width:160px;word-break:break-all}
  .item .spacer{flex:1}
  .msg{color:var(--muted);font-size:12px;margin-top:10px}
  .msg.err{color:var(--red)}
  .msg.ok{color:var(--green);font-weight:600}
  .sec-notice{background:rgba(210,153,34,.08);border:1px solid rgba(210,153,34,.25);border-radius:6px;padding:9px 13px;font-size:12px;color:var(--orange);margin-bottom:12px}
  .banner{background:rgba(248,81,73,.08);border:1px solid rgba(248,81,73,.25);border-radius:6px;padding:9px 13px;font-size:12px;color:var(--red);margin-bottom:12px}
  .identity{background:var(--bg);border:1px solid var(--border);border-radius:6px;padding:12px 14px;margin-top:10px;display:none}
  .identity .a{font-family:'Courier New',monospace;font-size:11px;word-break:break-all}
  .sub{font-size:11px;color:var(--muted);margin:2px 0 8px}
  .live{display:inline-flex;align-items:center;gap:4px;font-size:10px;color:var(--green);margin-left:auto}
  .live::before{content:'';width:5px;height:5px;border-radius:50%;background:var(--green);animation:pulse 2s infinite}
  @keyframes pulse{0%,100%{opacity:1}50%{opacity:.3}}
  @media(max-width:620px){.stats{grid-template-columns:1fr 1fr}}
</style>
</head>
<body>
<header>
  <div class="logo">VinX Ledger <span>CONSOLE ADMIN</span></div>
  <a href="/">← Explorateur public</a>
</header>
<main>

  <!-- Accès -->
  <div class="card">
    <div class="card-header">Accès administrateur</div>
    <div class="card-body">
      <div class="sec-notice">🔐 La clé admin ne quitte <b>jamais</b> ce navigateur : chaque action est signée localement puis diffusée. Le token opérateur (optionnel) n'est utilisé que pour les endpoints protégés du nœud.</div>
      <div class="row">
        <input type="file" id="key-file" accept=".json" style="display:none" onchange="onKeyFile(this)">
        <button class="ghost" onclick="document.getElementById('key-file').click()">📂 Charger la clé admin (.json)</button>
        <button class="danger" onclick="clearKey()">Déconnecter</button>
      </div>
      <div class="row">
        <input type="text" id="op-token" class="mono" placeholder="Token opérateur (Bearer) — optionnel" oninput="opToken=this.value.trim();refreshAll()">
      </div>
      <div class="identity" id="identity">
        <div class="a" id="id-addr">—</div>
        <div id="id-status" style="margin-top:6px"></div>
      </div>
      <div id="auth-msg"></div>
    </div>
  </div>

  <!-- Tableau de bord -->
  <div class="card">
    <div class="card-header">Tableau de bord<span class="live">live</span></div>
    <div class="card-body">
      <div class="stats">
        <div><div class="stat-v" id="d-height">—</div><div class="stat-l">Hauteur</div></div>
        <div><div class="stat-v" id="d-mempool">—</div><div class="stat-l">Mempool</div></div>
        <div><div class="stat-v sm" id="d-version">—</div><div class="stat-l">Version protocole</div></div>
        <div><div class="stat-v sm" id="d-remaining">—</div><div class="stat-l">Non encore émis</div></div>
        <div><div class="stat-v sm" id="d-circ">—</div><div class="stat-l">Circulation</div></div>
        <div><div class="stat-v sm" id="d-fee">—</div><div class="stat-l">Base fee</div></div>
      </div>
      <div class="sub" id="d-upgrade" style="margin-top:12px"></div>
    </div>
  </div>

  <!-- Validateurs -->
  <div class="card">
    <div class="card-header">Validateurs</div>
    <div class="card-body">
      <div class="banner" id="gate-banner">Lecture seule — chargez la clé de l'admin courant pour activer les actions.</div>
      <div class="sub" id="v-quorum">—</div>
      <div class="list" id="v-list"></div>
      <div class="row">
        <input type="text" id="add-val" class="mono" placeholder="Adresse validateur vinx1…">
        <button data-admin onclick="addValidator()">Ajouter validateur</button>
      </div>
      <div id="v-msg"></div>
      <div class="card-header" style="border:0;padding:14px 0 4px">Demandes en attente</div>
      <div class="sub">Nécessite le token opérateur.</div>
      <div class="list" id="pending-box"></div>
    </div>
  </div>

  <!-- Mises à jour -->
  <div class="card">
    <div class="card-header">Mises à jour de protocole</div>
    <div class="card-body">
      <div class="sub" id="u-current">—</div>
      <div class="row">
        <input type="number" id="u-major" placeholder="major" min="0" style="max-width:90px">
        <input type="number" id="u-minor" placeholder="minor" min="0" style="max-width:90px">
        <input type="number" id="u-patch" placeholder="patch" min="0" style="max-width:90px">
        <input type="number" id="u-height" placeholder="Hauteur d'activation" min="1">
        <button data-admin onclick="scheduleUpgrade()">Planifier</button>
      </div>
      <div class="sub">Préavis minimum imposé par le protocole : patch 7 j · minor 30 j · major 90 j (en blocs).</div>
      <div id="u-msg"></div>
    </div>
  </div>

  <!-- Maintenance -->
  <div class="card">
    <div class="card-header">Maintenance</div>
    <div class="card-body">
      <div class="row">
        <input type="number" id="keep-last" placeholder="Conserver les N derniers blocs (défaut 1000)" min="1">
        <button class="ghost" onclick="doCompact()">Compacter le stockage</button>
      </div>
      <div class="sub">Compactage : nécessite le token opérateur. Supprime les tx/signatures des vieux blocs (garde les en-têtes).</div>
      <div id="c-msg"></div>
      <div class="row">
        <input type="text" id="faucet-addr" class="mono" placeholder="Adresse destinataire vinx1…">
        <button class="green" onclick="doFaucet()" style="background:#238636">Faucet</button>
      </div>
      <div id="f-msg"></div>
    </div>
  </div>

</main>
<script>
const BASE = window.location.origin;
const DECIMAL = 1_000_000_000_000_000_000n;
let wallet = null, isAdmin = false, adminAddress = null, opToken = '';

// ─── Byte / hex helpers ───────────────────────────────────────────────────────
function hexToBytes(hex){const a=new Uint8Array(hex.length/2);for(let i=0;i<a.length;i++)a[i]=parseInt(hex.substr(i*2,2),16);return a;}
function bytesToHex(b){return Array.from(b).map(x=>x.toString(16).padStart(2,'0')).join('');}
function bigIntTo16BE(v){const b=new Uint8Array(16);for(let i=15;i>=0;i--){b[i]=Number(v&0xffn);v>>=8n;}return b;}
function bigIntTo8BE(v){const b=new Uint8Array(8);for(let i=7;i>=0;i--){b[i]=Number(v&0xffn);v>>=8n;}return b;}
function u32To4BE(n){const b=new Uint8Array(4);b[0]=(n>>>24)&0xff;b[1]=(n>>>16)&0xff;b[2]=(n>>>8)&0xff;b[3]=n&0xff;return b;}
function fmtAtoms(s){try{const a=BigInt(s);const w=a/DECIMAL;const c=(a%DECIMAL)/(DECIMAL/100n);return `${w.toLocaleString()}.${String(c).padStart(2,'0')} VINX`;}catch{return s;}}
const shortA = a => a && a.length>20 ? a.slice(0,12)+'…'+a.slice(-6) : (a||'');

const BECH32_CHARSET='qpzry9x8gf2tvdw0s3jn54khce6mua7l';
function bech32Decode20(addr){
  const s=addr.toLowerCase();const pos=s.lastIndexOf('1');
  if(pos<1)throw new Error('adresse bech32 invalide');
  const data=s.slice(pos+1);const values=[];
  for(const ch of data){const v=BECH32_CHARSET.indexOf(ch);if(v===-1)throw new Error('caractère bech32 invalide');values.push(v);}
  const words=values.slice(0,values.length-6);let acc=0,bits=0;const out=[];
  for(const w of words){acc=(acc<<5)|w;bits+=5;while(bits>=8){bits-=8;out.push((acc>>bits)&0xff);}}
  if(out.length!==20)throw new Error('charge utile bech32 != 20 octets');
  return new Uint8Array(out);
}

// ─── Auth ─────────────────────────────────────────────────────────────────────
function setAuthMsg(t,err){const el=document.getElementById('auth-msg');el.className='msg'+(err?' err':'');el.textContent=t;}
function authHeaders(){return opToken?{'Authorization':'Bearer '+opToken}:{};}

function onKeyFile(input){
  const file=input.files[0];if(!file)return;
  const reader=new FileReader();
  reader.onload=e=>{
    try{
      const json=JSON.parse(e.target.result);
      if(!json.secret_key_hex||!json.address)throw new Error('Champs manquants (secret_key_hex, address)');
      const seed=hexToBytes(json.secret_key_hex);
      if(seed.length!==32)throw new Error('Clé invalide (32 octets requis)');
      const kp=nacl.sign.keyPair.fromSeed(seed);
      wallet={address:json.address,secretKey64:kp.secretKey,publicKey32:kp.publicKey,chainId:42};
      fetch(BASE+'/health').then(r=>r.json()).then(h=>{if(wallet&&typeof h.chain_id==='number')wallet.chainId=h.chain_id;}).catch(()=>{});
      setAuthMsg('');
      verifyAdmin();
    }catch(err){setAuthMsg('Erreur : '+err.message,true);}
    input.value='';
  };
  reader.readAsText(file);
}

async function verifyAdmin(){
  try{const stats=await (await fetch(BASE+'/network/stats')).json();adminAddress=stats.admin_address||null;}
  catch{adminAddress=null;}
  isAdmin=!!(wallet&&adminAddress&&wallet.address===adminAddress);
  const box=document.getElementById('identity');box.style.display='block';
  document.getElementById('id-addr').textContent=wallet?wallet.address:'—';
  const st=document.getElementById('id-status');
  if(isAdmin){st.innerHTML='<span class="tag g">✓ ADMIN VÉRIFIÉ</span> Actions activées.';}
  else if(!adminAddress){st.innerHTML='<span class="tag muted">AUCUN ADMIN ON-CHAIN</span> Ce nœud n\'a pas d\'adresse admin configurée.';}
  else{st.innerHTML='<span class="tag r">LECTURE SEULE</span> La clé chargée n\'est pas l\'admin courant ('+shortA(adminAddress)+').';}
  updateGate();
}

function clearKey(){wallet=null;isAdmin=false;document.getElementById('identity').style.display='none';setAuthMsg('');updateGate();}

function updateGate(){
  document.querySelectorAll('[data-admin]').forEach(b=>b.disabled=!isAdmin);
  document.getElementById('gate-banner').style.display=isAdmin?'none':'block';
}

// ─── Governance signing ───────────────────────────────────────────────────────
// Canonical signing bytes — must match vinx-core Transaction::signing_bytes():
// disc(1) ‖ from(20) ‖ to(20) ‖ amount(16 BE) ‖ fee(16 BE) ‖ nonce(8 BE)
// ‖ chain_id(4 BE) ‖ expiry(1=0x00) ‖ payload ‖ sponsor(1=0x00)
async function submitGov(txName, disc, toAddr, payloadBytes){
  if(!wallet||!isAdmin)throw new Error('Clé admin requise.');
  let nonce=0;
  try{const a=await (await fetch(BASE+'/account/'+wallet.address)).json();nonce=a.nonce??0;}catch{}
  const chainId=wallet.chainId??42;
  const fromB=bech32Decode20(wallet.address), toB=bech32Decode20(toAddr);
  const payload=payloadBytes||new Uint8Array(0);
  const sig=new Uint8Array(1+20+20+16+16+8+4+1+payload.length+1);
  let i=0;
  sig[i++]=disc;
  sig.set(fromB,i);i+=20;
  sig.set(toB,i);i+=20;
  sig.set(bigIntTo16BE(0n),i);i+=16; // amount 0
  sig.set(bigIntTo16BE(0n),i);i+=16; // fee 0
  sig.set(bigIntTo8BE(BigInt(nonce)),i);i+=8;
  sig.set(u32To4BE(chainId),i);i+=4;
  sig[i++]=0x00; // expiry: None
  sig.set(payload,i);i+=payload.length;
  sig[i++]=0x00; // sponsor: None
  const signature=nacl.sign.detached(sig,wallet.secretKey64);
  const pubKeyArr='['+Array.from(wallet.publicKey32).join(',')+']';
  const payloadArr='['+Array.from(payload).join(',')+']';
  const body=`{"tx_type":"${txName}","from":"${wallet.address}","to":"${toAddr}","amount":0,"fee":0,"nonce":${nonce},"chain_id":${chainId},"payload":${payloadArr},"pub_key":${pubKeyArr},"signature":"${bytesToHex(signature)}"}`;
  const resp=await fetch(BASE+'/tx/submit',{method:'POST',headers:{'Content-Type':'application/json'},body});
  const json=await resp.json();
  if(!resp.ok)throw new Error(json.error||('HTTP '+resp.status));
  return json;
}

// ADR 0007: validator-set changes go through AdminAction (0x08). The payload is
// bincode(GovernanceAction): a little-endian u32 variant tag (AddValidator=0,
// RemoveValidator=1) followed by the 20-byte address. `to` is the admin (self).
function govValidatorPayload(variant,addr){
  const a=bech32Decode20(addr);
  const b=new Uint8Array(24);
  b[0]=variant&0xff;b[1]=(variant>>8)&0xff;b[2]=(variant>>16)&0xff;b[3]=(variant>>24)&0xff;
  b.set(a,4);
  return b;
}

// ADR 0006: payload = major(2) || minor(2) || patch(2) || activation_ts(8, unix seconds).
function upgradePayload(major,minor,patch,activationTs){
  const b=new Uint8Array(14);
  b[0]=(major>>8)&0xff;b[1]=major&0xff;
  b[2]=(minor>>8)&0xff;b[3]=minor&0xff;
  b[4]=(patch>>8)&0xff;b[5]=patch&0xff;
  b.set(bigIntTo8BE(BigInt(activationTs)),6);
  return b;
}

function okMsg(id,tx){document.getElementById(id).innerHTML='<p class="msg ok">✓ Transaction diffusée — tx '+tx.tx_hash.slice(0,16)+'…</p>';}
function errMsg(id,e){document.getElementById(id).innerHTML='<p class="msg err">'+e.message+'</p>';}

// ─── Actions ──────────────────────────────────────────────────────────────────
async function addValidator(){
  const addr=document.getElementById('add-val').value.trim();
  if(!addr.startsWith('vinx1'))return errMsg('v-msg',new Error('Adresse invalide.'));
  try{const tx=await submitGov('AdminAction',0x08,wallet.address,govValidatorPayload(0,addr));okMsg('v-msg',tx);document.getElementById('add-val').value='';setTimeout(refreshAll,600);}
  catch(e){errMsg('v-msg',e);}
}
async function removeValidator(addr){
  if(!confirm('Retirer le validateur '+shortA(addr)+' ?'))return;
  try{const tx=await submitGov('AdminAction',0x08,wallet.address,govValidatorPayload(1,addr));okMsg('v-msg',tx);setTimeout(refreshAll,600);}
  catch(e){errMsg('v-msg',e);}
}
async function scheduleUpgrade(){
  const M=parseInt(document.getElementById('u-major').value||'0',10);
  const m=parseInt(document.getElementById('u-minor').value||'0',10);
  const p=parseInt(document.getElementById('u-patch').value||'0',10);
  // ADR 0006: activation is a Unix timestamp (seconds), not a block height.
  const h=document.getElementById('u-height').value.trim();
  if(!h||BigInt(h)<=0n)return errMsg('u-msg',new Error('Timestamp d\'activation (unix, secondes) requis.'));
  try{const tx=await submitGov('AnnounceUpgrade',0x04,wallet.address,upgradePayload(M,m,p,h));okMsg('u-msg',tx);setTimeout(refreshAll,600);}
  catch(e){errMsg('u-msg',e);}
}
async function doCompact(){
  const keep=document.getElementById('keep-last').value||'1000';
  try{
    const resp=await fetch(BASE+'/admin/compact?keep_last='+keep,{method:'POST',headers:authHeaders()});
    const json=await resp.json();
    if(!resp.ok)throw new Error(json.error||('HTTP '+resp.status));
    document.getElementById('c-msg').innerHTML='<p class="msg ok">✓ Compacté (conservé '+json.kept_last+' derniers, tip '+json.tip_height+').</p>';
  }catch(e){errMsg('c-msg',e);}
}
async function doFaucet(){
  const addr=document.getElementById('faucet-addr').value.trim();
  if(!addr.startsWith('vinx1'))return errMsg('f-msg',new Error('Adresse invalide.'));
  try{
    const resp=await fetch(BASE+'/faucet/request',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({address:addr})});
    const json=await resp.json();
    if(!resp.ok)throw new Error(json.error||('HTTP '+resp.status));
    document.getElementById('f-msg').innerHTML='<p class="msg ok">✓ Envoyé '+fmtAtoms(json.amount_atoms)+'.</p>';
  }catch(e){errMsg('f-msg',e);}
}

// ─── Reads ────────────────────────────────────────────────────────────────────
function set(id,v){const el=document.getElementById(id);if(el)el.textContent=v;}
async function refreshDash(){
  try{
    const [h,s,p]=await Promise.all([
      fetch(BASE+'/health').then(r=>r.json()),
      fetch(BASE+'/network/stats').then(r=>r.json()),
      fetch(BASE+'/protocol/version').then(r=>r.json()),
    ]);
    set('d-height',h.height);set('d-mempool',h.mempool_pending);
    set('d-version',p.current_version);
    set('d-remaining',s.remaining_supply);set('d-circ',s.circulating_supply);
    set('d-fee',fmtAtoms(s.base_fee_atoms));
    const up=document.getElementById('u-current');
    up.textContent='Version courante : '+p.current_version+(p.pending_upgrade?' · upgrade planifiée v'+p.pending_upgrade.version+' le '+new Date(p.pending_upgrade.activation_ts*1000).toLocaleString():' · aucune upgrade en attente');
    document.getElementById('d-upgrade').textContent=p.pending_upgrade?('⏳ Upgrade v'+p.pending_upgrade.version+' activée le '+new Date(p.pending_upgrade.activation_ts*1000).toLocaleString()):'Aucune mise à jour en attente.';
  }catch(e){/* silencieux */}
}
async function loadValidators(){
  try{
    const d=await fetch(BASE+'/validators').then(r=>r.json());
    document.getElementById('v-quorum').textContent=d.count+' validateur(s) · quorum '+d.quorum+' · prochain leader '+shortA(d.next_leader);
    document.getElementById('v-list').innerHTML=d.validators.map(v=>{
      const tags=[v.is_next_leader?'<span class="tag b">leader</span>':'',v.online?'<span class="tag g">online</span>':'<span class="tag muted">offline</span>',v.suspended?'<span class="tag r">suspendu</span>':''].join(' ');
      const rm=`<button class="danger sm" data-admin onclick="removeValidator('${v.address}')">Retirer</button>`;
      return `<div class="item"><span class="addr">${v.address}</span>${tags}<span class="spacer"></span>${rm}</div>`;
    }).join('');
    updateGate();
  }catch(e){document.getElementById('v-list').innerHTML='<p class="msg err">'+e.message+'</p>';}
}
async function loadPending(){
  const box=document.getElementById('pending-box');
  try{
    const resp=await fetch(BASE+'/validators/pending',{headers:authHeaders()});
    if(resp.status===401){box.innerHTML='<p class="msg">Token opérateur requis pour voir les demandes.</p>';return;}
    const d=await resp.json();
    if(!d.count){box.innerHTML='<p class="msg">Aucune demande en attente.</p>';return;}
    box.innerHTML=d.requests.map(r=>{
      const btn=`<button class="sm" data-admin onclick="approve('${r.address}')">Approuver</button>`;
      const meta=r.message?('<span class="tag muted">'+r.message.slice(0,40)+'</span>'):'';
      return `<div class="item"><span class="addr">${r.address}</span>${meta}<span class="spacer"></span>${btn}</div>`;
    }).join('');
    updateGate();
  }catch(e){box.innerHTML='<p class="msg err">'+e.message+'</p>';}
}
async function approve(addr){
  try{const tx=await submitGov('AdminAction',0x08,wallet.address,govValidatorPayload(0,addr));okMsg('v-msg',tx);setTimeout(refreshAll,600);}
  catch(e){errMsg('v-msg',e);}
}

function refreshAll(){refreshDash();loadValidators();loadPending();}

refreshAll();
setInterval(refreshDash,6000);
</script>
</body>
</html>"####;
