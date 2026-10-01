use axum::response::Html;

/// Signing and hashing libraries, served by the node itself (no CDN): the UI works
/// offline and a compromised CDN cannot tamper with the code that holds the keys.
pub async fn nacl_js() -> impl axum::response::IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/javascript")],
        include_str!("../../assets/nacl.min.js"),
    )
}

pub async fn blake3_js() -> impl axum::response::IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/javascript")],
        include_str!("../../assets/blake3.min.js"),
    )
}

/// Wallet, explorer and network views (single page, no external resource).
pub async fn index() -> Html<&'static str> {
    Html(include_str!("../../assets/index.html"))
}

/// Admin console (`GET /admin`): dashboard + governance actions signed in-browser
/// with the admin key. Read-only until a key matching the on-chain admin loads.
pub async fn admin() -> Html<&'static str> {
    Html(ADMIN_HTML)
}

const ADMIN_HTML: &str = r####"<!DOCTYPE html>
<html lang="fr">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>VinX Ledger — Console Admin</title>
<script src="/assets/nacl.min.js"></script>
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
const DECIMAL = 1_000_000_000n; // 9 decimals (ADR 0081)
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
// disc(1) ‖ key_type(1=0x00 Ed25519) ‖ pub_key(32) ‖ to(20) ‖ amount(16 BE) ‖ fee(16 BE) ‖ nonce(8 BE)
// ‖ chain_id(4 BE) ‖ expiry(1=0x00) ‖ payload_len(4 BE) ‖ payload ‖ sponsor(1=0x00)
async function submitGov(txName, disc, toAddr, payloadBytes){
  if(!wallet||!isAdmin)throw new Error('Clé admin requise.');
  let nonce=0;
  try{const a=await (await fetch(BASE+'/account/'+wallet.address)).json();nonce=a.nonce??0;}catch{}
  const chainId=wallet.chainId??42;
  const toB=bech32Decode20(toAddr);
  const payload=payloadBytes||new Uint8Array(0);
  const sig=new Uint8Array(1+1+32+20+16+16+8+4+1+4+payload.length+1);
  let i=0;
  sig[i++]=disc;
  sig[i++]=0x00; // key type: Ed25519 (ADR 0081 D6)
  sig.set(wallet.publicKey32,i);i+=32;
  sig.set(toB,i);i+=20;
  sig.set(bigIntTo16BE(0n),i);i+=16; // amount 0
  sig.set(bigIntTo16BE(0n),i);i+=16; // fee 0
  sig.set(bigIntTo8BE(BigInt(nonce)),i);i+=8;
  sig.set(u32To4BE(chainId),i);i+=4;
  sig[i++]=0x00; // expiry: None
  sig.set(u32To4BE(payload.length),i);i+=4; // payload_len (VINX-12: always prefixed)
  sig.set(payload,i);i+=payload.length;
  sig[i++]=0x00; // sponsor: None
  const signature=nacl.sign.detached(sig,wallet.secretKey64);
  const pubKeyArr='['+Array.from(wallet.publicKey32).join(',')+']';
  const payloadArr='['+Array.from(payload).join(',')+']';
  const body=`{"tx_type":"${txName}","to":"${toAddr}","amount":0,"fee":0,"nonce":${nonce},"chain_id":${chainId},"payload":${payloadArr},"pub_key":${pubKeyArr},"signature":"${bytesToHex(signature)}"}`;
  const resp=await fetch(BASE+'/tx/submit',{method:'POST',headers:{'Content-Type':'application/json'},body});
  const json=await resp.json();
  if(!resp.ok)throw new Error(json.error||('HTTP '+resp.status));
  return json;
}

// ADR 0007: validator-set changes go through AdminAction (0x08). The payload is
// borsh(GovernanceAction): a one-byte variant tag (AddValidator=0, RemoveValidator=1)
// followed by the 20-byte address (ADR 0081 D7c). `to` is the admin (self).
function govValidatorPayload(variant,addr){
  const a=bech32Decode20(addr);
  const b=new Uint8Array(21);
  b[0]=variant&0xff;
  b.set(a,1);
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
