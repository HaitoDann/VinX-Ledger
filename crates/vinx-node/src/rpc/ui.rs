use axum::response::Html;

pub async fn index() -> Html<&'static str> {
    Html(HTML)
}

const HTML: &str = r#"<!DOCTYPE html>
<html lang="fr">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>VinX Ledger — Dashboard</title>
<style>
  :root {
    --bg: #0d1117;
    --surface: #161b22;
    --border: #30363d;
    --text: #e6edf3;
    --muted: #8b949e;
    --accent: #58a6ff;
    --green: #3fb950;
    --red: #f85149;
    --orange: #d29922;
    --radius: 8px;
  }
  * { box-sizing: border-box; margin: 0; padding: 0; }
  body { background: var(--bg); color: var(--text); font-family: 'Segoe UI', system-ui, sans-serif; font-size: 14px; }

  header {
    background: var(--surface);
    border-bottom: 1px solid var(--border);
    padding: 16px 32px;
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .logo { font-size: 20px; font-weight: 700; color: var(--accent); letter-spacing: -0.5px; }
  .logo span { color: var(--muted); font-weight: 400; font-size: 13px; margin-left: 8px; }
  .status-dot { width: 8px; height: 8px; border-radius: 50%; background: var(--green); margin-left: auto; }
  .status-dot.offline { background: var(--red); }
  .status-label { color: var(--muted); font-size: 12px; }

  main { max-width: 1100px; margin: 0 auto; padding: 24px 24px; display: grid; gap: 24px; }

  /* Cards */
  .card { background: var(--surface); border: 1px solid var(--border); border-radius: var(--radius); overflow: hidden; }
  .card-header { padding: 14px 20px; border-bottom: 1px solid var(--border); font-weight: 600; font-size: 13px; color: var(--muted); text-transform: uppercase; letter-spacing: 0.5px; display: flex; align-items: center; gap: 8px; }
  .card-body { padding: 20px; }

  /* Network status */
  .stats-grid { display: grid; grid-template-columns: repeat(3, 1fr); gap: 16px; }
  .stat { text-align: center; }
  .stat-value { font-size: 28px; font-weight: 700; color: var(--accent); font-variant-numeric: tabular-nums; }
  .stat-label { color: var(--muted); font-size: 12px; margin-top: 4px; }

  /* Form */
  .form-row { display: flex; gap: 10px; }
  input[type="text"], input[type="number"] {
    flex: 1;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 9px 13px;
    color: var(--text);
    font-size: 14px;
    font-family: 'Courier New', monospace;
    outline: none;
    transition: border-color 0.15s;
  }
  input:focus { border-color: var(--accent); }
  button {
    background: var(--accent);
    color: #fff;
    border: none;
    border-radius: 6px;
    padding: 9px 20px;
    font-size: 14px;
    font-weight: 600;
    cursor: pointer;
    white-space: nowrap;
    transition: opacity 0.15s;
  }
  button:hover { opacity: 0.85; }
  button:disabled { opacity: 0.4; cursor: default; }

  /* Result panels */
  .result { margin-top: 16px; }
  .result-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 10px; margin-top: 12px; }
  .result-item { background: var(--bg); border: 1px solid var(--border); border-radius: 6px; padding: 12px 16px; }
  .result-item .label { color: var(--muted); font-size: 11px; text-transform: uppercase; letter-spacing: 0.5px; margin-bottom: 4px; }
  .result-item .value { font-weight: 600; word-break: break-all; }
  .result-item .value.mono { font-family: 'Courier New', monospace; font-size: 12px; }
  .result-item.full { grid-column: 1 / -1; }

  /* Tag */
  .tag { display: inline-block; padding: 2px 8px; border-radius: 4px; font-size: 11px; font-weight: 600; }
  .tag.green { background: rgba(63,185,80,0.15); color: var(--green); }
  .tag.red { background: rgba(248,81,73,0.15); color: var(--red); }
  .tag.blue { background: rgba(88,166,255,0.15); color: var(--accent); }
  .tag.orange { background: rgba(210,153,34,0.15); color: var(--orange); }

  /* Transactions table */
  .tx-list { margin-top: 12px; display: grid; gap: 8px; }
  .tx-row { background: var(--bg); border: 1px solid var(--border); border-radius: 6px; padding: 12px 16px; display: grid; grid-template-columns: auto 1fr auto; gap: 12px; align-items: center; }
  .tx-hash { font-family: 'Courier New', monospace; font-size: 11px; color: var(--muted); }
  .tx-route { font-size: 12px; color: var(--muted); margin-top: 2px; }
  .tx-addr { font-family: 'Courier New', monospace; }

  /* Error/empty */
  .msg { color: var(--muted); font-size: 13px; margin-top: 12px; }
  .msg.error { color: var(--red); }

  /* Tabs for block explorer */
  .tabs { display: flex; gap: 0; border-bottom: 1px solid var(--border); margin-bottom: 20px; }
  .tab { padding: 10px 18px; cursor: pointer; color: var(--muted); font-size: 13px; border-bottom: 2px solid transparent; margin-bottom: -1px; transition: all 0.15s; }
  .tab:hover { color: var(--text); }
  .tab.active { color: var(--accent); border-bottom-color: var(--accent); }

  /* Live badge */
  .live { display: inline-flex; align-items: center; gap: 5px; font-size: 11px; color: var(--green); }
  .live::before { content: ''; width: 6px; height: 6px; border-radius: 50%; background: var(--green); animation: pulse 2s infinite; }
  @keyframes pulse { 0%,100%{opacity:1} 50%{opacity:0.3} }

  /* Block nav */
  .block-nav { display: flex; align-items: center; gap: 8px; }
  .block-nav button { padding: 7px 14px; font-size: 13px; }
  .block-nav input { max-width: 120px; }

  /* Responsive */
  @media(max-width:600px) {
    .stats-grid { grid-template-columns: 1fr; }
    .result-grid { grid-template-columns: 1fr; }
    .tx-row { grid-template-columns: 1fr; }
    header { padding: 12px 16px; }
    main { padding: 16px; }
  }
</style>
</head>
<body>

<header>
  <div class="logo">VinX Ledger <span>DEVNET</span></div>
  <div id="header-dot" class="status-dot offline"></div>
  <div id="header-status" class="status-label">Connexion...</div>
</header>

<main>

  <!-- Network Status -->
  <div class="card">
    <div class="card-header">
      <span>Réseau</span>
      <span class="live" style="margin-left:auto">Live</span>
    </div>
    <div class="card-body">
      <div class="stats-grid">
        <div class="stat">
          <div class="stat-value" id="stat-height">—</div>
          <div class="stat-label">Hauteur de bloc</div>
        </div>
        <div class="stat">
          <div class="stat-value" id="stat-status">—</div>
          <div class="stat-label">Statut</div>
        </div>
        <div class="stat">
          <div class="stat-value" id="stat-mempool">—</div>
          <div class="stat-label">Mempool (en attente)</div>
        </div>
      </div>
    </div>
  </div>

  <!-- Account Lookup -->
  <div class="card">
    <div class="card-header">Compte</div>
    <div class="card-body">
      <div class="form-row">
        <input type="text" id="acc-input" placeholder="vinx1..." />
        <button onclick="lookupAccount()">Chercher</button>
      </div>
      <div id="acc-result"></div>
    </div>
  </div>

  <!-- Block Explorer -->
  <div class="card">
    <div class="card-header">Explorateur de blocs</div>
    <div class="card-body">
      <div class="block-nav">
        <button onclick="prevBlock()" id="btn-prev">←</button>
        <input type="number" id="block-input" placeholder="Hauteur" min="0" onchange="lookupBlock()" />
        <button onclick="nextBlock()">→</button>
        <button onclick="lookupBlock()">Afficher</button>
        <button onclick="goToLatest()" style="background:var(--surface);color:var(--accent);border:1px solid var(--border)">Dernier bloc</button>
      </div>
      <div id="block-result"></div>
    </div>
  </div>

</main>

<script>
const BASE = window.location.origin;
let currentHeight = 0;
let currentBlockHeight = null;

// ── Fetch helpers ──────────────────────────────────────────────────────────

async function get(path) {
  const r = await fetch(BASE + path);
  if (!r.ok) {
    const j = await r.json().catch(() => ({ error: `HTTP ${r.status}` }));
    throw new Error(j.error || `HTTP ${r.status}`);
  }
  return r.json();
}

// ── Network status (auto-refresh) ─────────────────────────────────────────

async function refreshStatus() {
  try {
    const h = await get('/health');
    currentHeight = h.height;
    document.getElementById('stat-height').textContent = h.height.toLocaleString();
    document.getElementById('stat-status').innerHTML = `<span class="tag green">${h.status}</span>`;
    document.getElementById('stat-mempool').textContent = h.mempool_pending;
    document.getElementById('header-dot').className = 'status-dot';
    document.getElementById('header-status').textContent = `http://127.0.0.1:8545`;
  } catch {
    document.getElementById('stat-height').textContent = '—';
    document.getElementById('stat-status').innerHTML = `<span class="tag red">offline</span>`;
    document.getElementById('stat-mempool').textContent = '—';
    document.getElementById('header-dot').className = 'status-dot offline';
    document.getElementById('header-status').textContent = 'Nœud injoignable';
  }
}

setInterval(refreshStatus, 3000);
refreshStatus();

// ── Account lookup ─────────────────────────────────────────────────────────

async function lookupAccount() {
  const addr = document.getElementById('acc-input').value.trim();
  const out = document.getElementById('acc-result');
  if (!addr) return;
  out.innerHTML = `<p class="msg">Chargement…</p>`;
  try {
    const a = await get(`/account/${addr}`);
    const frozenTag = a.frozen
      ? `<span class="tag red">Gelé</span>`
      : `<span class="tag green">Actif</span>`;
    out.innerHTML = `
      <div class="result-grid">
        <div class="result-item full">
          <div class="label">Adresse</div>
          <div class="value mono">${a.address}</div>
        </div>
        <div class="result-item">
          <div class="label">Solde</div>
          <div class="value">${a.balance}</div>
        </div>
        <div class="result-item">
          <div class="label">Staké</div>
          <div class="value">${a.staked}</div>
        </div>
        <div class="result-item">
          <div class="label">Nonce</div>
          <div class="value">${a.nonce}</div>
        </div>
        <div class="result-item">
          <div class="label">Statut</div>
          <div class="value">${frozenTag}</div>
        </div>
      </div>`;
  } catch (e) {
    out.innerHTML = `<p class="msg error">Erreur : ${e.message}</p>`;
  }
}

document.getElementById('acc-input').addEventListener('keydown', e => {
  if (e.key === 'Enter') lookupAccount();
});

// ── Block explorer ─────────────────────────────────────────────────────────

function txTypeTag(type) {
  const map = { Transfer: 'blue', Emission: 'green', Stake: 'orange', Unstake: 'orange' };
  return `<span class="tag ${map[type] || 'blue'}">${type}</span>`;
}

function shortAddr(addr) {
  if (!addr || addr.length < 16) return addr;
  return addr.slice(0, 12) + '…' + addr.slice(-6);
}

async function lookupBlock() {
  const val = document.getElementById('block-input').value;
  const height = val === '' ? null : parseInt(val, 10);
  const out = document.getElementById('block-result');
  if (height === null || isNaN(height)) return;
  currentBlockHeight = height;
  document.getElementById('btn-prev').disabled = height <= 0;
  out.innerHTML = `<p class="msg">Chargement…</p>`;
  try {
    const b = await get(`/block/${height}`);
    const txRows = b.transactions.length === 0
      ? `<p class="msg" style="margin-top:12px">Aucune transaction dans ce bloc.</p>`
      : `<div class="tx-list">${b.transactions.map(tx => `
          <div class="tx-row">
            <div>${txTypeTag(tx.tx_type)}</div>
            <div>
              <div class="tx-hash">${tx.hash.slice(0,24)}…</div>
              <div class="tx-route">
                <span class="tx-addr" title="${tx.from}">${shortAddr(tx.from)}</span>
                → <span class="tx-addr" title="${tx.to}">${shortAddr(tx.to)}</span>
              </div>
            </div>
            <div style="text-align:right">
              <div style="font-weight:600">${tx.amount}</div>
              <div style="color:var(--muted);font-size:11px">fee ${tx.fee}</div>
            </div>
          </div>`).join('')}
        </div>`;

    out.innerHTML = `
      <div class="result-grid" style="margin-top:16px">
        <div class="result-item">
          <div class="label">Hauteur</div>
          <div class="value">${b.height}</div>
        </div>
        <div class="result-item">
          <div class="label">Timestamp</div>
          <div class="value">${new Date(b.timestamp * 1000).toLocaleString()}</div>
        </div>
        <div class="result-item">
          <div class="label">Transactions</div>
          <div class="value">${b.tx_count}</div>
        </div>
        <div class="result-item">
          <div class="label">Validateur</div>
          <div class="value mono" title="${b.validator}">${shortAddr(b.validator)}</div>
        </div>
        <div class="result-item full">
          <div class="label">Hash</div>
          <div class="value mono">${b.hash}</div>
        </div>
        <div class="result-item full">
          <div class="label">Hash précédent</div>
          <div class="value mono">${b.prev_hash}</div>
        </div>
      </div>
      ${txRows}`;
  } catch (e) {
    out.innerHTML = `<p class="msg error">Erreur : ${e.message}</p>`;
  }
}

function prevBlock() {
  if (currentBlockHeight === null || currentBlockHeight <= 0) return;
  document.getElementById('block-input').value = currentBlockHeight - 1;
  lookupBlock();
}

function nextBlock() {
  const next = currentBlockHeight === null ? 0 : currentBlockHeight + 1;
  document.getElementById('block-input').value = next;
  lookupBlock();
}

async function goToLatest() {
  try {
    const h = await get('/chain/height');
    document.getElementById('block-input').value = h.height;
    lookupBlock();
  } catch {}
}

document.getElementById('block-input').addEventListener('keydown', e => {
  if (e.key === 'Enter') lookupBlock();
});
</script>
</body>
</html>"#;
