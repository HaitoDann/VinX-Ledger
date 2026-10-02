// VinX bureau — interface. Toute la logique sensible (clés, signatures, nœud) est dans
// le backend Rust ; ici, uniquement de l'affichage et des appels `invoke`.
'use strict';
const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);
const $ = (id) => document.getElementById(id);
const esc = (s) => String(s ?? '').replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
const FACTOR = 1000000000n;

let me = null, page = 'home', acct = null, val = null, net = null, confirmSend = null;

// ── Format ────────────────────────────────────────────────────────────────
function vinx(atoms, digits = 2) {
  let a = BigInt(atoms || 0), neg = a < 0n; if (neg) a = -a;
  const whole = (a / FACTOR).toString().replace(/\B(?=(\d{3})+(?!\d))/g, ' ');
  let frac = (a % FACTOR).toString().padStart(9, '0').slice(0, Math.max(digits, 0));
  if (digits > 2) frac = frac.replace(/0+$/, '').padEnd(2, '0');
  return (neg ? '−' : '') + whole + (frac ? ',' + frac : '');
}
const short = (a) => a ? a.slice(0, 10) + '…' + a.slice(-6) : '';
function ago(ts) {
  const s = Math.max(0, Math.floor(Date.now() / 1000 - ts));
  return s < 60 ? `il y a ${s} s` : s < 3600 ? `il y a ${Math.floor(s / 60)} min` : `il y a ${Math.floor(s / 3600)} h`;
}
function msg(id, kind, text) { $(id).innerHTML = text ? `<div class="msg ${kind}">${esc(text)}</div>` : ''; }
function toast(t) { const e = $('toast'); e.textContent = t; e.hidden = false; clearTimeout(e._t); e._t = setTimeout(() => e.hidden = true, 2200); }

// ── Écrans ────────────────────────────────────────────────────────────────
const screens = ['s-welcome', 's-create', 's-password', 's-restore', 's-unlock', 's-app'];
function show(id) { screens.forEach((s) => $(s).hidden = s !== id); }
document.querySelectorAll('[data-go]').forEach((b) => b.onclick = () => {
  if (b.dataset.go === 's-create') return startCreate();
  show(b.dataset.go);
});

async function boot() {
  try {
    const t = localStorage.getItem('vinx-theme');
    if (t) document.documentElement.dataset.theme = t;
  } catch (_) {}
  const info = await invoke('info');
  $('t-ver').textContent = 'VinX ' + info.version;
  $('t-entry').value = info.settings.entry || '';
  if (!info.has_wallet) return show('s-welcome');
  me = info.address;
  if (!info.unlocked) {
    $('u-addr').textContent = me;
    $('u-val').hidden = !info.settings.validator;
    show('s-unlock'); setTimeout(() => $('u-pass').focus(), 50);
    return;
  }
  enterApp();
}

// Création
async function startCreate() {
  const r = await invoke('create_wallet');
  $('c-words').innerHTML = r.phrase.split(' ').map((w, i) => `<div><span>${i + 1}</span>${esc(w)}</div>`).join('');
  $('c-ok').checked = false; $('c-next').disabled = true;
  show('s-create');
}
$('c-ok').onchange = () => $('c-next').disabled = !$('c-ok').checked;
$('c-next').onclick = () => { $('p-1').value = $('p-2').value = ''; msg('p-msg'); show('s-password'); setTimeout(() => $('p-1').focus(), 50); };
$('p-go').onclick = async () => {
  if ($('p-1').value !== $('p-2').value) return msg('p-msg', 'err', 'Les deux mots de passe sont différents.');
  $('p-go').disabled = true; msg('p-msg', 'wait', 'Chiffrement du portefeuille…');
  try { me = await invoke('confirm_wallet', { password: $('p-1').value }); enterApp(); }
  catch (e) { msg('p-msg', 'err', e); }
  $('p-go').disabled = false;
};
// Restauration
$('r-go').onclick = async () => {
  if ($('r-1').value !== $('r-2').value) return msg('r-msg', 'err', 'Les deux mots de passe sont différents.');
  $('r-go').disabled = true; msg('r-msg', 'wait', 'Restauration…');
  try { me = await invoke('restore_wallet', { phrase: $('r-phrase').value, password: $('r-1').value }); $('r-phrase').value = ''; enterApp(); }
  catch (e) { msg('r-msg', 'err', e); }
  $('r-go').disabled = false;
};
// Déverrouillage
async function doUnlock() {
  $('u-go').disabled = true; msg('u-msg', 'wait', 'Ouverture…');
  try { me = await invoke('unlock', { password: $('u-pass').value }); $('u-pass').value = ''; msg('u-msg'); enterApp(); }
  catch (e) { msg('u-msg', 'err', e); }
  $('u-go').disabled = false;
}
$('u-go').onclick = doUnlock;
$('u-pass').onkeydown = (e) => { if (e.key === 'Enter') doUnlock(); };

// ── Application ───────────────────────────────────────────────────────────
function enterApp() {
  show('s-app');
  $('q-addr').textContent = $('t-addr').textContent = me;
  invoke('receive_qr').then((svg) => $('q-code').innerHTML = svg).catch(() => {});
  go('home'); refresh();
}
function go(p) {
  page = p;
  document.querySelectorAll('aside nav a').forEach((a) => a.classList.toggle('on', a.dataset.page === p));
  ['home', 'send', 'recv', 'val', 'set'].forEach((x) => $('p-' + x).hidden = x !== p);
  if (p === 'send') { resetSend(); setTimeout(() => $('s-to').focus(), 50); }
  refresh();
}
document.querySelectorAll('[data-page]').forEach((a) => a.onclick = () => go(a.dataset.page));
document.querySelectorAll('[data-copy]').forEach((b) => b.onclick = () => {
  navigator.clipboard.writeText($(b.dataset.copy).textContent).then(() => toast('Adresse copiée'));
});

async function refresh() {
  if ($('s-app').hidden) return;
  try { net = await invoke('status'); } catch (_) { net = null; }
  renderNet();
  try { acct = await invoke('account'); renderHome(); } catch (_) {}
  if (page === 'val' || !val) { try { val = await invoke('validator'); renderVal(); } catch (_) {} }
}
setInterval(refresh, 4000);

function renderNet() {
  const dot = $('n-dot');
  if (!net || !net.online) { dot.className = 'dot'; $('n-txt').textContent = 'Hors ligne'; $('n-sub').textContent = 'Vérifiez le réseau dans Réglages'; return; }
  const age = net.tip_timestamp ? Date.now() / 1000 - net.tip_timestamp : 0;
  const late = age > 6 * (net.block_time_secs || 12);
  dot.className = 'dot ' + (late ? 'warn' : 'ok');
  $('n-txt').textContent = (late ? 'Réseau à l\'arrêt' : 'Connecté') + ' · bloc ' + Number(net.height).toLocaleString('fr-FR');
  const local = net.local;
  $('n-sub').textContent = local ? `Validateur en marche · ${local.peers ?? 0} pair(s)` : (net.chain_id === 7 ? 'Testnet public' : 'Réseau ' + net.chain_id);
}

function renderHome() {
  const a = acct.account || {};
  $('h-bal').innerHTML = `${vinx(a.balance_atoms)}<small>VINX</small>`;
  const staked = BigInt(a.staked_atoms || 0);
  $('h-kv').innerHTML = staked > 0n ? `<span>Garantie de validateur <b>${vinx(staked)} VINX</b></span>` : '';
  $('h-faucet').hidden = !(net && net.chain_id !== 1);
  $('s-avail').textContent = `Disponible : ${vinx(a.balance_atoms)} VINX`;
  const txs = acct.txs || [];
  if (!txs.length) { $('h-txs').innerHTML = '<div class="empty">Aucune opération pour l\'instant.</div>'; return; }
  $('h-txs').innerHTML = txs.map((t) => {
    const incoming = t.to === me && t.from !== me;
    let title = incoming ? 'Reçu' : 'Envoyé', who = incoming ? 'de ' + short(t.from) : 'à ' + short(t.to), icon = incoming ? 'i-recv' : 'i-send';
    if (t.tx_type === 'Stake') { title = 'Garantie déposée'; who = 'validateur'; icon = 'i-stake'; }
    if (t.tx_type === 'Unstake') { title = 'Garantie retirée'; who = 'disponible après le délai'; icon = 'i-stake'; }
    const memo = t.memo ? ' · ' + esc(t.memo) : '';
    return `<div class="item"><div class="ico ${incoming ? 'in' : ''}"><svg class="i"><use href="#${icon}"/></svg></div>
      <div class="grow"><div class="t">${title}</div><div class="s">${esc(who)}${memo} · bloc ${t.block_height}</div></div>
      <div class="amt ${incoming ? 'in' : ''}">${incoming ? '+' : '−'}${vinx(t.amount_atoms)}</div></div>`;
  }).join('');
}

$('h-faucet').onclick = async () => {
  msg('h-msg', 'wait', 'Demande au faucet du réseau de test…');
  try { await invoke('faucet'); msg('h-msg', 'ok', 'Demande acceptée : les VINX arrivent au prochain bloc.'); setTimeout(refresh, 3000); }
  catch (e) { msg('h-msg', 'err', e); }
};

// ── Envoyer ───────────────────────────────────────────────────────────────
function resetSend() { confirmSend = null; $('s-sum').hidden = true; $('s-go').textContent = 'Vérifier'; msg('s-msg'); }
['s-to', 's-amt', 's-memo'].forEach((id) => $(id).oninput = resetSend);
$('s-max').onclick = () => {
  const bal = BigInt(acct?.account?.balance_atoms || 0) - 100000n; // garde les frais
  $('s-amt').value = bal > 0n ? vinx(bal, 9).replace(/ /g, '') : '0'; resetSend();
};
$('s-go').onclick = async () => {
  const to = $('s-to').value.trim(), amount = $('s-amt').value.trim().replace(',', '.').replace(/\s/g, ''), memo = $('s-memo').value;
  if (!to.startsWith('vinx1')) return msg('s-msg', 'err', 'L\'adresse doit commencer par vinx1.');
  if (!(Number(amount) > 0)) return msg('s-msg', 'err', 'Indiquez un montant.');
  if (!confirmSend) {
    confirmSend = { to, amount, memo };
    $('s-sum').innerHTML = `<div><span>À</span><span class="mono">${esc(short(to))}</span></div>
      ${memo ? `<div><span>Référence</span><span>${esc(memo)}</span></div>` : ''}
      <div><span>Frais du réseau</span><span class="mono">~0,0001 VINX</span></div>
      <div><span>Montant</span><span class="mono">${esc(amount.replace('.', ','))} VINX</span></div>`;
    $('s-sum').hidden = false; $('s-go').textContent = 'Confirmer l\'envoi';
    return;
  }
  $('s-go').disabled = true; msg('s-msg', 'wait', 'Envoi…');
  try {
    await invoke('send', confirmSend);
    msg('s-msg', 'ok', 'Envoyé. Le paiement sera définitif au prochain bloc.');
    $('s-to').value = $('s-amt').value = $('s-memo').value = ''; confirmSend = null; $('s-sum').hidden = true; $('s-go').textContent = 'Vérifier';
    setTimeout(refresh, 2500);
  } catch (e) { msg('s-msg', 'err', e); }
  $('s-go').disabled = false;
};

// ── Valider ───────────────────────────────────────────────────────────────
let valBusy = false;
function renderVal() {
  if (!val) return;
  const pool = val.pool || {}, st = pool.status || 'none', local = val.local;
  const min = pool.min_bond_atoms ? vinx(pool.min_bond_atoms) : '—';
  const active = st === 'active' && pool.in_set;
  $('n-val').hidden = !active;
  $('v-switch').classList.toggle('on', val.enabled);
  $('v-bond-txt').textContent = `Le réseau demande ${min} VINX, bloqués tant que vous validez. Ils restent à vous : vous les récupérez quand vous arrêtez (21 jours de délai).`;
  if (valBusy) return;
  let title = 'Validateur arrêté', sub = `Activez pour commencer. Il faut ${min} VINX de garantie.`;
  if (val.enabled) {
    if (!val.running) { title = 'Démarrage…'; sub = 'Le programme du validateur se lance en arrière-plan.'; }
    else if (active) { title = 'Validateur actif'; sub = 'Vous signez des blocs et recevez des récompenses.'; }
    else if (st === 'warmup') { title = 'Échauffement'; sub = 'Le réseau vous fait entrer bientôt.'; }
    else if (st === 'benched') { title = 'En réserve'; sub = 'Le set est complet : vous remontez selon votre présence.'; }
    else if (st === 'unbonding') { title = 'Garantie en cours de retrait'; sub = 'Disponible à la fin du délai de 21 jours.'; }
    else { title = 'En attente de la garantie'; sub = 'Le dépôt arrive au prochain bloc.'; }
  } else if (st === 'unbonding') { title = 'Validateur arrêté'; sub = 'Votre garantie sera disponible à la fin du délai de 21 jours.'; }
  $('v-title').textContent = title; $('v-sub').textContent = sub;

  if (!val.enabled) { $('v-track').innerHTML = ''; $('v-stats').hidden = true; return; }
  const h = local ? local.height : 0, top = Math.max(h, net?.entry_height || 0);
  const synced = local && h >= top - 2;
  const next = pool.next_epoch_ts ? new Date(pool.next_epoch_ts * 1000).toLocaleTimeString('fr-FR', { hour: '2-digit', minute: '2-digit' }) : '';
  const steps = [
    { t: 'Nœud démarré', d: !local ? 'Lancement…' : synced ? `Synchronisé · bloc ${h.toLocaleString('fr-FR')}` : `Synchronisation · ${h} / ${top}`, done: synced, now: !synced },
    { t: 'Garantie déposée', d: `${min} VINX bloqués`, done: st !== 'none', now: synced && st === 'none' },
    { t: 'Échauffement', d: st === 'warmup' ? `Encore ${pool.warmup_epochs_left} étape(s) d'une heure · prochaine à ${next}` : 'Environ 3 heures, pour protéger le réseau', done: ['active', 'benched'].includes(st), now: st === 'warmup' },
    { t: 'Actif', d: 'Vous signez des blocs et recevez des récompenses', done: active, now: false },
  ];
  $('v-track').innerHTML = steps.map((s, i) => `<div class="st ${s.done ? 'done' : s.now ? 'now' : ''}"><div class="n">${s.done ? '✓' : i + 1}</div><div><div class="t">${s.t}</div><div class="d">${esc(s.d)}</div></div></div>`).join('');
  $('v-stats').hidden = !(active || st === 'benched');
  $('v-cos').textContent = pool.cosigned_in_window ?? '—';
  $('v-pres').textContent = pool.eligible_in_window ? Math.round(100 * pool.cosigned_in_window / pool.eligible_in_window) + ' %' : '—';
  $('v-peers').textContent = local?.peers ?? '—';
}

$('v-switch').onclick = () => {
  if (valBusy) return;
  const min = val?.pool?.min_bond_atoms ? vinx(val.pool.min_bond_atoms) : '';
  if (!val?.enabled) {
    $('v-msg').innerHTML = `<div class="msg wait">Activer bloque ${min} VINX en garantie et lance le validateur sur cet ordinateur.
      <div class="row"><button id="v-yes">Activer</button><button class="ghost" id="v-no">Annuler</button></div></div>`;
    $('v-yes').onclick = async () => {
      valBusy = true; msg('v-msg', 'wait', 'Démarrage du validateur et synchronisation… (une à deux minutes la première fois)');
      $('v-switch').classList.add('on');
      try { await invoke('validator_start'); msg('v-msg', 'ok', 'C\'est parti : la garantie est déposée, l\'échauffement commence.'); }
      catch (e) { msg('v-msg', 'err', e); }
      valBusy = false; val = await invoke('validator').catch(() => val); renderVal();
    };
    $('v-no').onclick = () => msg('v-msg');
  } else {
    $('v-msg').innerHTML = `<div class="msg wait">Arrêter le validateur ?
      <div class="row"><button class="ghost" id="v-pause">Mettre en pause</button><button class="danger" id="v-out">Arrêter et récupérer la garantie</button><button class="ghost" id="v-no">Annuler</button></div>
      <div class="hint">En pause, la garantie reste bloquée et le validateur est écarté du set. La récupérer prend 21 jours.</div></div>`;
    const stop = async (withdraw) => {
      valBusy = true; msg('v-msg', 'wait', 'Arrêt…');
      try { await invoke('validator_stop', { withdraw }); msg('v-msg', 'ok', withdraw ? 'Validateur arrêté. Votre garantie sera disponible dans 21 jours.' : 'Validateur en pause.'); }
      catch (e) { msg('v-msg', 'err', e); }
      valBusy = false; val = await invoke('validator').catch(() => val); renderVal();
    };
    $('v-pause').onclick = () => stop(false);
    $('v-out').onclick = () => stop(true);
    $('v-no').onclick = () => msg('v-msg');
  }
};

// ── Réglages ──────────────────────────────────────────────────────────────
$('t-save').onclick = async () => { await invoke('set_entry', { entry: $('t-entry').value }); msg('t-msg', 'ok', 'Enregistré.'); refresh(); };
$('t-lock').onclick = async () => { await invoke('lock'); boot(); };
$('t-theme').onclick = () => {
  const t = document.documentElement.dataset.theme === 'light' ? 'dark' : 'light';
  document.documentElement.dataset.theme = t;
  try { localStorage.setItem('vinx-theme', t); } catch (_) {}
};

boot();
