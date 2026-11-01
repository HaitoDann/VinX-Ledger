// Portefeuille mobile VinX Ledger.
//
// Les clés ne quittent jamais le téléphone : la phrase de 12 mots est chiffrée
// par le mot de passe (PBKDF2-SHA-256 puis AES-GCM, WebCrypto) et gardée dans
// le stockage local. La signature se fait dans le module WebAssembly, compilé
// depuis le code Rust de la chaîne. Le nœud ne reçoit que des transactions
// déjà signées.

import init, { phrase, seed, account_address, sign, to_units, valid_address, qr_svg, scan_qr }
  from "./pkg/vinx_mobile.js";

const VERSION = "0.28.0";
const VAULT = "vinx.vault";
const DEFAULT_NODE = "https://vinx-ledger.vinxlabs.fr";
const PBKDF2_ROUNDS = 600000;
const LOCK_AFTER_MS = 5 * 60 * 1000;
const VINX_DECIMALS = 9;

const $ = (id) => document.getElementById(id);
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

let vault = null;      // { salt, iv, data, node, accounts: [{ index, label }] }
let seedHex = null;    // en mémoire seulement, tant que le portefeuille est ouvert
let current = 0;       // indice du compte affiché
let pendingWords = "";
let data = null;       // dernier état lu sur le nœud
let hiddenSince = 0;

/* ----- Stockage chiffré ----- */
const b64 = (u8) => btoa(String.fromCharCode(...u8));
const unb64 = (s) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
async function keyFrom(password, salt) {
  const base = await crypto.subtle.importKey("raw", new TextEncoder().encode(password), "PBKDF2", false, ["deriveKey"]);
  return crypto.subtle.deriveKey({ name: "PBKDF2", salt, iterations: PBKDF2_ROUNDS, hash: "SHA-256" },
    base, { name: "AES-GCM", length: 256 }, false, ["encrypt", "decrypt"]);
}
async function seal(words, password) {
  const salt = crypto.getRandomValues(new Uint8Array(16));
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const ct = await crypto.subtle.encrypt({ name: "AES-GCM", iv }, await keyFrom(password, salt), new TextEncoder().encode(words));
  return { salt: b64(salt), iv: b64(iv), data: b64(new Uint8Array(ct)) };
}
async function open(password) {
  try {
    const pt = await crypto.subtle.decrypt({ name: "AES-GCM", iv: unb64(vault.iv) },
      await keyFrom(password, unb64(vault.salt)), unb64(vault.data));
    return new TextDecoder().decode(pt);
  } catch (_) { throw new Error("mot de passe incorrect"); }
}
function loadVault() { try { vault = JSON.parse(localStorage.getItem(VAULT)); } catch (_) { vault = null; } }
function saveVault() { localStorage.setItem(VAULT, JSON.stringify(vault)); }

/* ----- Interface ----- */
function toast(msg, err = false) {
  const d = document.createElement("div");
  d.className = err ? "err" : ""; d.textContent = msg;
  $("toast").appendChild(d); setTimeout(() => d.remove(), err ? 8000 : 4000);
}
function dialog(title, html, ok = "Confirmer") {
  $("dlgTitle").textContent = title; $("dlgBody").innerHTML = html; $("dlgOk").textContent = ok;
  const d = $("dlg"); d.returnValue = ""; d.showModal();
  return new Promise((res) => d.addEventListener("close", () => res(d.returnValue === "ok"), { once: true }));
}
function show(view) {
  document.querySelectorAll(".view").forEach((v) => v.classList.toggle("on", v.id === "v-" + view));
  document.querySelectorAll("nav a").forEach((a) => a.classList.toggle("on", a.dataset.go === view));
  if (view !== "scan") stopScan();
  if (view === "scan") startScan();
  if (view === "receive") renderReceive();
  if (view === "history") renderHistory();
  window.scrollTo(0, 0);
}
const guard = (f) => async (...a) => { try { await f(...a); } catch (e) { toast(e.message || String(e), true); } };

/* ----- Montants ----- */
function fmt(units, decimals = VINX_DECIMALS) {
  let n; try { n = BigInt(units || "0"); } catch (_) { return "—"; }
  const one = 10n ** BigInt(decimals);
  const whole = (n / one).toString().replace(/\B(?=(\d{3})+(?!\d))/g, " ");
  const frac = decimals ? (n % one).toString().padStart(decimals, "0").replace(/0+$/, "") : "";
  return whole + (frac ? "," + frac : "");
}

/* ----- Nœud ----- */
const node = () => (vault?.node || DEFAULT_NODE).replace(/\/+$/, "");
async function get(path) {
  let r;
  try { r = await fetch(node() + path); } catch (_) { throw new Error("nœud injoignable : vérifie la connexion et l'adresse du nœud (Réglages)"); }
  if (r.status === 404) return null;
  const v = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(v.error || `le nœud a répondu ${r.status}`);
  return v;
}
async function submit(tx) {
  let r;
  try {
    r = await fetch(node() + "/transactions", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ tx }) });
  } catch (_) { throw new Error("nœud injoignable"); }
  const v = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(v.error || `le nœud a répondu ${r.status}`);
  return v.id;
}
const me = () => account_address(seedHex, current);

/* Prochain nonce, en comptant les transactions du compte encore en attente. */
async function nextNonce(address) {
  const acc = await get(`/accounts/${address}`);
  if (!acc) throw new Error("ce compte est vide : il doit d'abord recevoir des VINX (Recevoir)");
  const h = await get(`/accounts/${address}/transactions?limit=20`);
  const pending = (h?.transactions || []).filter((t) => t.status === "en attente" && t.sender === address).map((t) => t.nonce);
  const last = Math.max(acc.nonce, ...pending);
  return { nonce: last + 1, includeKey: acc.public_key === null && pending.length === 0, account: acc };
}

/* Adresse, ou nom.vinx résolu par le nœud. */
async function resolve(input) {
  const s = input.trim();
  if (!s) throw new Error("indique le destinataire");
  if (valid_address(s)) return { address: s, how: "adresse" };
  const name = s.toLowerCase().replace(/\.vinx$/, "");
  if (!/^[a-z0-9-]{3,32}$/.test(name)) throw new Error("adresse invalide : vérifie-la caractère par caractère");
  const n = await get(`/names/${name}`);
  if (!n?.owner) throw new Error(`${name}.vinx n'existe pas ou a expiré`);
  return { address: n.owner, how: `${name}.vinx` };
}

/* ----- Comptes ----- */
function fillAccounts() {
  const sel = $("account");
  sel.innerHTML = vault.accounts.map((a) => `<option value="${a.index}">${esc(a.label)}</option>`).join("");
  sel.value = String(current);
  sel.classList.toggle("hidden", vault.accounts.length < 2);
}

async function refresh() {
  if (!seedHex) return;
  const addr = me();
  const [status, acc] = await Promise.all([get("/status"), get(`/accounts/${addr}`)]);
  data = { status, acc, address: addr };
  $("netLabel").textContent = `${status.chain_id} · bloc ${status.height}`;
  $("balance").textContent = fmt(acc?.balance || "0");
  $("balance").classList.remove("skeleton");
  renderBalance(addr, BigInt(acc?.balance || "0")).catch(() => { $("chart").innerHTML = `<p class="muted">Historique indisponible.</p>`; });
  const tokens = acc?.assets || [];
  $("tokens").innerHTML = tokens.length
    ? tokens.map((t) => `<div><span><b>${esc(t.symbol)}</b> <span class="muted">${esc(t.name || "")}</span>${t.frozen ? ' <span class="warn">gelé</span>' : ""}</span><span>${fmt(t.units, t.decimals)}</span></div>`).join("")
    : `<p class="muted">Aucun jeton accepté.</p>`;
  if (status.protocol_version >= 6 && acc && BigInt(acc.reserve || "0") > 0n)
    $("tokens").innerHTML += `<p class="muted">Réserve immobilisée : ${fmt(acc.reserve)} VINX pour ${acc.objects ?? 0} objet(s), rendus quand tu les retires. Disponible : ${fmt(acc.spendable ?? acc.balance)} VINX.</p>`;
  const opts = `<option value="">VINX</option>` + tokens.map((t) => `<option value="${esc(t.symbol)}">${esc(t.symbol)}</option>`).join("");
  for (const id of ["sendCur", "reqCur"]) { const k = $(id).value; $(id).innerHTML = opts; $(id).value = tokens.some((t) => t.symbol === k) ? k : ""; }
}

/* ----- Évolution du solde ----- */
// Variation du solde VINX apportée par une opération, frais compris.
function vinxDelta(t, me) {
  const big = (x) => { try { return BigInt(x || "0"); } catch (_) { return 0n; } };
  let d = t.sender === me ? -big(t.fee) : 0n;
  if (t.asset && t.units) return d;
  if (t.kind === "prélèvement") return d + (t.payer === me ? -big(t.amount) : t.sender === me ? big(t.amount) : 0n);
  if (t.kind === "paiement groupé") {
    const list = t.payments || [];
    if (t.sender === me) return d - list.reduce((s, p) => s + big(p.amount), 0n);
    return d + list.filter((p) => p.to === me).reduce((s, p) => s + big(p.amount), 0n);
  }
  if (!t.amount) return d;
  if (t.sender === me && t.to !== me) return d - big(t.amount);
  if (t.to === me && t.sender !== me) return d + big(t.amount);
  return d;
}
async function renderBalance(addr, balance) {
  const h = await get(`/accounts/${addr}/transactions?limit=100`);
  const now = Date.now(), from = now - 30 * 86400000;
  const done = (h?.transactions || []).filter((t) => t.status !== "en attente" && t.timestamp).sort((a, b) => b.timestamp - a.timestamp);
  let bal = balance, inSum = 0n, outSum = 0n, count = 0;
  const pts = [[now, bal]];
  for (const t of done) {
    if (t.timestamp < from) break;
    const d = vinxDelta(t, addr);
    count++; if (d > 0n) inSum += d; else outSum -= d;
    pts.push([t.timestamp, bal]); bal -= d; pts.push([t.timestamp - 1, bal]);
  }
  pts.push([from, bal]);
  const series = pts.reverse();
  $("stats").innerHTML = [["Reçu (30 j)", `<span class="in">+ ${fmt(inSum)}</span>`], ["Envoyé (30 j)", `− ${fmt(outSum)}`], ["Opérations", count]]
    .map(([k, v]) => `<div><span>${k}</span><b>${v}</b></div>`).join("");
  const W = Math.max(280, $("chart").clientWidth || 320), H = 150, L = 8, B = 18, T = 10;
  const vals = series.map(([, v]) => Number(v) / 10 ** VINX_DECIMALS);
  const hi = Math.max(...vals) * 1.12 || 1;
  const x = (ts) => L + ((ts - from) / (now - from)) * (W - L - 6);
  const y = (v) => T + (1 - v / hi) * (H - T - B);
  const d = series.map(([ts], i) => `${i ? "L" : "M"}${x(ts).toFixed(1)},${y(vals[i]).toFixed(1)}`).join(" ");
  const day = (ts) => new Date(ts).toLocaleDateString("fr-FR", { day: "numeric", month: "short" });
  $("chart").innerHTML = `<svg viewBox="0 0 ${W} ${H}" role="img" aria-label="Évolution du solde">
    <defs><linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="var(--accent)" stop-opacity=".45"/><stop offset="1" stop-color="var(--accent)" stop-opacity="0"/></linearGradient></defs>
    <line x1="${L}" x2="${W}" y1="${H - B}" y2="${H - B}" stroke="var(--line)"/>
    <path d="${d} L${x(now)},${H - B} L${x(from)},${H - B}Z" fill="url(#g)"/>
    <path d="${d}" fill="none" stroke="var(--accent)" stroke-width="2" stroke-linejoin="round"/>
    <circle cx="${x(now)}" cy="${y(vals[vals.length - 1])}" r="4" fill="var(--accent)"/>
    <text x="${L}" y="${H - 3}">${day(from)}</text><text x="${W - 4}" y="${H - 3}" text-anchor="end">aujourd'hui</text></svg>`;
}

/* ----- Envoyer ----- */
async function send() {
  const amountText = $("sendAmount").value, memo = $("sendMemo").value.trim(), symbol = $("sendCur").value;
  if (memo && new TextEncoder().encode(memo).length > 32) throw new Error("mémo trop long : 32 octets au plus");
  const from = me();
  const [{ address: to, how }, fees, status] = await Promise.all([resolve($("sendTo").value), get("/fees"), get("/status")]);
  if (to === from) throw new Error("c'est l'adresse de ce compte");
  const fee = BigInt(fees.recommended_fee);
  const { nonce, includeKey, account } = await nextNonce(from);
  const recipient = await get(`/accounts/${to}`);
  if (recipient?.memo_required && !memo) throw new Error("ce destinataire exige un mémo (référence de facture, de client…)");
  const balance = BigInt(account.balance);
  const reserve = BigInt(status.params.min_reserve);
  let body, shown, total = fee;
  if (!symbol) {
    const amount = BigInt(to_units(amountText, VINX_DECIMALS));
    if (amount === 0n) throw new Error("montant nul");
    if (!recipient && amount < reserve) throw new Error(`ce destinataire n'a encore jamais reçu de VINX : le premier versement doit être d'au moins ${fmt(reserve)} VINX`);
    total = amount + fee;
    body = { kind: "transfer", to, amount: amount.toString(), memo };
    shown = `${fmt(amount)} VINX`;
  } else {
    const mine = (account.assets || []).find((t) => t.symbol === symbol);
    const amount = BigInt(to_units(amountText, mine.decimals));
    if (amount === 0n) throw new Error("montant nul");
    if (mine.frozen) throw new Error(`${symbol} est gelé sur ce compte`);
    if (amount > BigInt(mine.units)) throw new Error(`solde insuffisant : ${fmt(mine.units, mine.decimals)} ${symbol} disponibles`);
    if (!(recipient?.assets || []).some((t) => t.symbol === symbol)) throw new Error(`ce destinataire n'a pas encore accepté ${symbol}`);
    body = { kind: "transfer_asset", symbol, to, amount: amount.toString(), memo };
    shown = `${fmt(amount, mine.decimals)} ${symbol}`;
  }
  if (total > balance) throw new Error(`solde insuffisant : il faut ${fmt(total)} VINX frais compris`);
  const after = balance - total;
  // Protocole 6 : réserve du compte et de ses objets (jetons acceptés, mandats, règles).
  const locked = BigInt(account.reserve || "0");
  if (locked > 0n && after < locked) throw new Error(`il doit rester au moins ${fmt(locked)} VINX : réserve de ce compte et de ses ${account.objects} objet(s)`);
  if (after !== 0n && after < reserve) throw new Error(`il doit rester au moins ${fmt(reserve)} VINX sur le compte, ou rien du tout`);
  const ok = await dialog("Confirmer l'envoi", `<dl>
    <dt>À</dt><dd class="mono">${esc(to)}</dd><dt>Trouvé par</dt><dd>${esc(how)}</dd>
    <dt>Montant</dt><dd><b>${esc(shown)}</b></dd>${memo ? `<dt>Mémo</dt><dd>${esc(memo)}</dd>` : ""}
    <dt>Frais</dt><dd>${fmt(fee)} VINX</dd><dt>Solde VINX après</dt><dd>${fmt(after)}</dd></dl>
    ${how === "adresse" ? `<p class="warn">Vérifie l'adresse caractère par caractère : un envoi ne peut pas être annulé.</p>` : ""}`, "Envoyer");
  if (!ok) return;
  await signAndSubmit(nonce, includeKey, fee, body, status.chain_id);
  ["sendTo", "sendAmount", "sendMemo"].forEach((i) => $(i).value = "");
  toast("Envoyé : confirmation dans quelques secondes.");
  show("home"); setTimeout(guard(refresh), 6000);
}

async function signAndSubmit(nonce, includeKey, fee, body, chainId) {
  const req = { chain_id: chainId, nonce, fee: fee.toString(), include_key: includeKey, body };
  const { tx } = JSON.parse(sign(seedHex, current, JSON.stringify(req)));
  return submit(tx);
}

/* ----- Accepter un jeton ----- */
async function accept() {
  const symbol = $("acceptSym").value.trim().toUpperCase();
  if (!symbol) throw new Error("indique le symbole du jeton");
  const asset = await get(`/assets/${symbol}`);
  if (!asset) throw new Error(`aucun jeton ${symbol} sur ce réseau`);
  const from = me();
  const [fees, status, { nonce, includeKey, account }] = await Promise.all([get("/fees"), get("/status"), nextNonce(from)]);
  if ((account.assets || []).some((t) => t.symbol === symbol)) throw new Error(`ce compte a déjà accepté ${symbol}`);
  const fee = BigInt(fees.recommended_fee);
  const per = status.protocol_version >= 6 ? BigInt(status.params.object_reserve || "0") : 0n;
  if (per > 0n) {
    const objects = BigInt(account.objects || 0) + 1n;
    const need = per * objects + BigInt(status.params.min_reserve);
    if (BigInt(account.balance) - fee < need) throw new Error(`accepter un jeton immobilise ${fmt(per)} VINX : il faut garder ${fmt(need)} VINX sur ce compte`);
  }
  const p = asset.powers;
  const powers = [p.mint && "émettre de nouvelles unités", p.freeze && "geler ton solde", p.clawback && "reprendre des unités sur ton compte"].filter(Boolean);
  const ok = await dialog(`Accepter ${symbol}`, `<dl>
    <dt>Jeton</dt><dd>${esc(asset.name)}</dd><dt>Émetteur</dt><dd class="mono">${esc(asset.issuer)}</dd>
    <dt>En circulation</dt><dd>${fmt(asset.supply_units, asset.decimals)}</dd>
    <dt>L'émetteur peut</dt><dd>${powers.length ? esc(powers.join(", ")) : "rien : supply fixe"}</dd>
    <dt>Frais</dt><dd>${fmt(fee)} VINX</dd></dl>
    ${p.freeze || p.clawback ? `<p class="warn">N'accepte que des jetons d'émetteurs en qui tu as confiance.</p>` : ""}`, "Accepter");
  if (!ok) return;
  await signAndSubmit(nonce, includeKey, fee, { kind: "accept_asset", symbol }, status.chain_id);
  $("acceptSym").value = "";
  toast(`${symbol} accepté : il apparaîtra dans quelques secondes.`);
  show("home"); setTimeout(guard(refresh), 6000);
}

/* ----- Recevoir ----- */
function receiveLink() {
  const q = new URLSearchParams();
  const a = $("reqAmount").value.trim().replace(",", "."); const m = $("reqMemo").value.trim();
  const cur = $("reqCur")?.value || "";
  if (a) q.set("amount", a); if (cur) q.set("asset", cur); if (m) q.set("memo", m);
  return "vinx:" + me() + (q.toString() ? "?" + q.toString() : "");
}
function renderReceive() {
  if (!seedHex) return;
  $("myAddr").textContent = me();
  $("qr").innerHTML = qr_svg(receiveLink());
}

/* Lien vinx:adresse?amount=…&memo=… (même format que le portefeuille de bureau). */
function parseLink(text) {
  const m = /^vinx:([^?\s]+)(?:\?(\S*))?$/i.exec(String(text).trim());
  if (!m) return null;
  const q = new URLSearchParams(m[2] || "");
  return { to: decodeURIComponent(m[1]), amount: q.get("amount") || "", memo: q.get("memo") || "", asset: (q.get("asset") || "").toUpperCase() };
}
function useLink(text) {
  const r = parseLink(text);
  if (!r) return toast("Ce n'est pas un QR de paiement VinX.", true);
  if (r.asset && !(data?.acc?.assets || []).some((t) => t.symbol === r.asset))
    return toast(`Cette demande est en ${r.asset} : accepte d'abord ce jeton (onglet Jetons).`, true);
  $("sendTo").value = r.to; $("sendAmount").value = r.amount.replace(".", ","); $("sendMemo").value = r.memo; $("sendCur").value = r.asset || "";
  show("send");
  toast("Demande de paiement lue : vérifie puis envoie.");
}

/* ----- Scanner ----- */
let stream = null, timer = null;
async function startScan() {
  $("scanInfo").textContent = "Vise le QR code d'une demande de paiement.";
  try {
    stream = await navigator.mediaDevices.getUserMedia({ video: { facingMode: "environment" }, audio: false });
  } catch (_) {
    $("scanInfo").textContent = "Caméra indisponible ou refusée : colle le lien ci-dessous.";
    return;
  }
  const v = $("video"); v.srcObject = stream; await v.play().catch(() => {});
  const canvas = document.createElement("canvas"); const ctx = canvas.getContext("2d", { willReadFrequently: true });
  timer = setInterval(() => {
    if (!v.videoWidth) return;
    const scale = Math.min(1, 640 / Math.max(v.videoWidth, v.videoHeight));
    canvas.width = Math.round(v.videoWidth * scale); canvas.height = Math.round(v.videoHeight * scale);
    ctx.drawImage(v, 0, 0, canvas.width, canvas.height);
    const img = ctx.getImageData(0, 0, canvas.width, canvas.height);
    const text = scan_qr(canvas.width, canvas.height, new Uint8Array(img.data.buffer));
    if (text) { stopScan(); useLink(text); }
  }, 250);
}
function stopScan() {
  clearInterval(timer); timer = null;
  stream?.getTracks().forEach((t) => t.stop()); stream = null;
}

/* ----- Historique ----- */
async function renderHistory() {
  const addr = me();
  const h = await get(`/accounts/${addr}/transactions?limit=50`).catch(() => null);
  const list = h?.transactions || [];
  const decimals = Object.fromEntries((data?.acc?.assets || []).map((t) => [t.symbol, t.decimals]));
  $("history").innerHTML = list.length ? list.map((t) => {
    const out = t.sender === addr;
    const when = t.status === "en attente" ? "en attente" : new Date(t.timestamp).toLocaleString("fr-FR", { dateStyle: "short", timeStyle: "short" });
    const amount = t.asset ? (t.units ? `${fmt(t.units, decimals[t.asset] ?? 0)} ${esc(t.asset)}` : esc(t.asset)) : t.amount ? `${fmt(t.amount)} VINX` : "";
    const sign = amount && (t.kind === "paiement" || t.kind === "paiement en actif") ? (out ? "− " : "+ ") : "";
    return `<div><span>${esc(t.kind)}<br><small class="muted">${esc(when)}${t.memo ? " · " + esc(t.memo) : ""}</small></span><span class="${out ? "out" : "in"}">${sign}${amount}</span></div>`;
  }).join("") : `<p class="muted">Aucune opération récente.</p>`;
}

/* ----- Ouverture ----- */
function openWallet(words) {
  seedHex = seed(words);
  $("nav").classList.remove("hidden");
  fillAccounts();
  show("home");
  guard(refresh)();
}
function lock() {
  seedHex = null; stopScan();
  $("nav").classList.add("hidden"); $("account").classList.add("hidden");
  $("pw").value = ""; show("unlock");
}

async function main() {
  await init();
  $("version").textContent = `VinX mobile ${VERSION}`;
  loadVault();
  show(vault ? "unlock" : "welcome");
  $("node").value = node();

  $("goCreate").onclick = guard(() => {
    pendingWords = phrase();
    $("newWords").innerHTML = pendingWords.split(" ").map((w) => `<span>${esc(w)}</span>`).join("");
    $("wrote").checked = false; show("create");
  });
  $("goRestore").onclick = () => show("restore");
  $("createNext").onclick = () => $("wrote").checked ? show("password") : toast("Note d'abord les 12 mots sur papier.", true);
  $("restoreNext").onclick = guard(() => { seed($("restoreWords").value); pendingWords = $("restoreWords").value.trim().toLowerCase().split(/\s+/).join(" "); show("password"); });
  $("pwNext").onclick = guard(async () => {
    const p1 = $("pw1").value;
    if (p1.length < 8) throw new Error("au moins 8 caractères");
    if (p1 !== $("pw2").value) throw new Error("les deux mots de passe diffèrent");
    vault = { ...(await seal(pendingWords, p1)), node: DEFAULT_NODE, accounts: [{ index: 0, label: "Compte principal" }] };
    saveVault();
    const words = pendingWords; pendingWords = ""; $("pw1").value = $("pw2").value = ""; $("restoreWords").value = "";
    openWallet(words);
  });
  $("unlockBtn").onclick = guard(async () => { openWallet(await open($("pw").value)); $("pw").value = ""; });
  $("pw").addEventListener("keydown", (e) => { if (e.key === "Enter") $("unlockBtn").click(); });

  document.querySelectorAll("[data-go]").forEach((el) => el.addEventListener("click", (e) => { e.preventDefault(); show(el.dataset.go); }));
  $("account").onchange = () => { current = +$("account").value; guard(refresh)(); };
  $("sendBtn").onclick = guard(send);
  $("acceptBtn").onclick = guard(accept);
  $("pasteBtn").onclick = () => useLink($("pasteLink").value);
  ["reqAmount", "reqMemo"].forEach((i) => $(i).addEventListener("input", guard(renderReceive)));
  $("reqCur").addEventListener("change", guard(renderReceive));
  $("copyAddr").onclick = guard(async () => { await navigator.clipboard.writeText(me()); toast("Adresse copiée."); });

  $("saveNode").onclick = guard(async () => {
    const url = $("node").value.trim().replace(/\/+$/, "");
    if (!/^https?:\/\//.test(url)) throw new Error("adresse du nœud : http(s)://…");
    const old = vault.node; vault.node = url;
    try { await get("/status"); } catch (e) { vault.node = old; throw e; }
    saveVault(); toast("Nœud enregistré."); guard(refresh)();
  });
  $("addAccount").onclick = () => {
    const index = Math.max(...vault.accounts.map((a) => a.index)) + 1;
    vault.accounts.push({ index, label: `Compte ${index + 1}` }); saveVault();
    current = index; fillAccounts(); show("home"); guard(refresh)();
  };
  $("showWords").onclick = guard(async () => {
    const pw = prompt("Mot de passe"); if (!pw) return;
    const words = await open(pw);
    await dialog("Tes 12 mots", `<p class="warn">Ne les montre à personne.</p><div class="words">${words.split(" ").map((w) => `<span>${esc(w)}</span>`).join("")}</div>`, "Fermer");
  });
  $("lockBtn").onclick = lock;
  $("resetBtn").onclick = guard(async () => {
    if (!await dialog("Effacer le portefeuille ?", `<p class="warn">Sans tes 12 mots, l'argent de ces comptes sera perdu. Vérifie que tu les as notés.</p>`, "Effacer")) return;
    localStorage.removeItem(VAULT); vault = null; seedHex = null; location.reload();
  });

  // Verrouillage après quelques minutes en arrière-plan.
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) { hiddenSince = Date.now(); stopScan(); }
    else if (seedHex && Date.now() - hiddenSince > LOCK_AFTER_MS) lock();
    else if (seedHex) guard(refresh)();
  });
  setInterval(() => { if (seedHex && !document.hidden) guard(refresh)(); }, 15000);
}

main().catch((e) => { document.body.innerHTML = `<p style="padding:16px">Démarrage impossible : ${esc(e.message || e)}</p>`; });
