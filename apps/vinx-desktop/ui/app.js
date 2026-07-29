// VinX Wallet — frontend glue. All logic (keys, signing, RPC) lives in the Rust
// backend; here we only invoke commands and render results.
const invoke = window.__TAURI__.core.invoke;

const $ = (id) => document.getElementById(id);
const nodeUrl = () => $("node-url").value.trim();
let myAddress = null;

function toast(msg, kind = "") {
  const t = $("toast");
  t.textContent = msg;
  t.className = "toast " + kind;
  clearTimeout(toast._t);
  toast._t = setTimeout(() => t.classList.add("hidden"), 4200);
}

// ─── tabs ──────────────────────────────────────────────────────────────────
document.querySelectorAll(".tab").forEach((tab) => {
  tab.addEventListener("click", () => {
    document.querySelectorAll(".tab").forEach((t) => t.classList.remove("active"));
    document.querySelectorAll(".tabpane").forEach((p) => p.classList.remove("active"));
    tab.classList.add("active");
    $("tab-" + tab.dataset.tab).classList.add("active");
    if (tab.dataset.tab === "admin") refreshAdmin();
  });
});

// ─── network / status ───────────────────────────────────────────────────────
async function connect() {
  try {
    const h = await invoke("node_health", { node: nodeUrl() });
    $("node-pill").textContent = "en ligne · #" + h.height;
    $("node-pill").className = "pill on";
    await refreshNetwork();
    if (myAddress) await refreshAccount();
    return true;
  } catch (e) {
    $("node-pill").textContent = "hors ligne";
    $("node-pill").className = "pill off";
    toast("Nœud injoignable : " + e, "err");
    return false;
  }
}

async function refreshNetwork() {
  try {
    const [h, s] = await Promise.all([
      invoke("node_health", { node: nodeUrl() }),
      invoke("network_stats", { node: nodeUrl() }),
    ]);
    $("net-height").textContent = h.height;
    $("net-mempool").textContent = h.mempool_pending;
    $("net-chain").textContent = h.chain_id;
    $("net-fee").textContent = s.base_fee_atoms + " atomes";
    $("net-circ").textContent = s.circulating_supply;
    $("net-foundry").textContent = s.foundry;
  } catch (e) {
    /* handled by connect() */
  }
}

// ─── wallet ─────────────────────────────────────────────────────────────────
function showWalletOpen(addr) {
  myAddress = addr;
  $("wallet-none").classList.add("hidden");
  $("wallet-open").classList.remove("hidden");
  $("my-address").textContent = addr;
  refreshAccount();
  loadHistory();
  updateAdminCheck();
}

function showWalletClosed() {
  myAddress = null;
  $("wallet-open").classList.add("hidden");
  $("wallet-none").classList.remove("hidden");
  $("my-balance").textContent = $("my-staked").textContent = $("my-nonce").textContent = "—";
  updateAdminCheck();
}

async function refreshAccount() {
  if (!myAddress) return;
  try {
    const a = await invoke("account", { node: nodeUrl(), address: myAddress });
    $("my-balance").textContent = a.balance;
    $("my-staked").textContent = a.staked;
    $("my-nonce").textContent = a.nonce;
  } catch (e) {
    toast("Solde indisponible : " + e, "err");
  }
}

async function loadHistory() {
  if (!myAddress) return;
  try {
    const h = await invoke("history", { node: nodeUrl(), address: myAddress, limit: 25, offset: 0 });
    const body = document.querySelector("#history tbody");
    if (!h.txs.length) {
      body.innerHTML = '<tr><td colspan="6" class="muted">Aucune transaction.</td></tr>';
      return;
    }
    const short = (a) => (a && a.length > 14 ? a.slice(0, 8) + "…" + a.slice(-4) : a);
    body.innerHTML = h.txs
      .map(
        (t) =>
          `<tr><td>#${t.block_height}</td><td>${t.tx_type}</td><td><code>${short(t.from)}</code></td>` +
          `<td><code>${short(t.to)}</code></td><td>${t.amount}</td><td>${t.fee}</td></tr>`
      )
      .join("");
  } catch (e) {
    /* history is best-effort */
  }
}

$("btn-open").onclick = async () => {
  try {
    const addr = await invoke("wallet_open", { path: $("wallet-path").value.trim() });
    showWalletOpen(addr);
    toast("Wallet ouvert.", "ok");
  } catch (e) {
    toast("Ouverture impossible : " + e, "err");
  }
};

$("btn-create").onclick = async () => {
  try {
    const addr = await invoke("wallet_create", { path: $("wallet-path").value.trim() });
    showWalletOpen(addr);
    toast("Nouveau wallet créé et sauvegardé.", "ok");
  } catch (e) {
    toast("Création impossible : " + e, "err");
  }
};

$("btn-lock").onclick = async () => {
  await invoke("wallet_close");
  showWalletClosed();
  toast("Wallet verrouillé.");
};

$("btn-copy").onclick = async () => {
  try {
    await navigator.clipboard.writeText(myAddress);
    toast("Adresse copiée.", "ok");
  } catch {
    toast("Copie impossible.", "err");
  }
};

$("btn-refresh").onclick = async () => {
  await refreshNetwork();
  await refreshAccount();
  await loadHistory();
};

// ─── actions ────────────────────────────────────────────────────────────────
async function afterTx(promise, label) {
  try {
    const hash = await promise;
    toast(label + " soumise — hash " + hash.slice(0, 12) + "…", "ok");
    setTimeout(() => {
      refreshAccount();
      loadHistory();
      refreshNetwork();
    }, 1200);
  } catch (e) {
    toast("Échec : " + e, "err");
  }
}

$("btn-send").onclick = () =>
  afterTx(
    invoke("send_transfer", { node: nodeUrl(), to: $("send-to").value.trim(), amount: $("send-amount").value.trim() }),
    "Transaction"
  );
$("btn-stake").onclick = () =>
  afterTx(invoke("stake", { node: nodeUrl(), amount: $("stake-amount").value.trim() }), "Stake");
$("btn-unstake").onclick = () =>
  afterTx(invoke("unstake", { node: nodeUrl(), amount: $("stake-amount").value.trim() }), "Déstake");

// ─── admin ──────────────────────────────────────────────────────────────────
async function updateAdminCheck() {
  const el = $("admin-check");
  if (!myAddress) {
    el.textContent = "Aucun wallet ouvert.";
    return;
  }
  try {
    const s = await invoke("network_stats", { node: nodeUrl() });
    el.textContent =
      s.admin_address === myAddress
        ? "✓ Le wallet ouvert est bien l'admin on-chain."
        : "⚠ Le wallet ouvert n'est PAS l'admin on-chain — les actions échoueront.";
  } catch {
    el.textContent = "";
  }
}

async function refreshAdmin() {
  updateAdminCheck();
  try {
    const [v, p] = await Promise.all([
      invoke("validators", { node: nodeUrl() }),
      invoke("protocol", { node: nodeUrl() }),
    ]);
    $("v-count").textContent = v.count;
    $("v-quorum").textContent = v.quorum;
    $("v-list").innerHTML = v.validators
      .map((x) => {
        const badges = [];
        if (x.is_next_leader) badges.push('<span class="badge leader">leader</span>');
        badges.push(x.suspended ? '<span class="badge susp">suspendu</span>' : x.online ? '<span class="badge online">en ligne</span>' : '<span class="badge offline">hors ligne</span>');
        return `<li><code>${x.address}</code>${badges.join("")}</li>`;
      })
      .join("");
    $("p-version").textContent = p.current_version;
    $("p-pending").textContent = p.pending_upgrade
      ? `${p.pending_upgrade.version} @ ${new Date(p.pending_upgrade.activation_ts * 1000).toISOString()}`
      : "aucun";
  } catch (e) {
    toast("Admin : " + e, "err");
  }
}

$("btn-refresh-admin").onclick = refreshAdmin;
$("btn-add-val").onclick = () =>
  afterTx(invoke("admin_add_validator", { node: nodeUrl(), validator: $("adm-validator").value.trim() }), "Ajout validateur").then(refreshAdmin);
$("btn-rm-val").onclick = () =>
  afterTx(invoke("admin_remove_validator", { node: nodeUrl(), validator: $("adm-validator").value.trim() }), "Retrait validateur").then(refreshAdmin);
$("btn-announce").onclick = () =>
  afterTx(
    invoke("admin_announce_upgrade", {
      node: nodeUrl(),
      version: $("adm-version").value.trim(),
      activationTs: parseInt($("adm-height").value, 10) || 0,
    }),
    "Annonce d'upgrade"
  ).then(refreshAdmin);

$("btn-connect").onclick = connect;

// ─── boot ───────────────────────────────────────────────────────────────────
(async function boot() {
  try {
    const p = await invoke("suggest_wallet_path");
    $("wallet-path").value = p;
  } catch {
    /* keep placeholder */
  }
  // If a wallet is already open in the backend (hot reload), reflect it.
  try {
    const addr = await invoke("wallet_address");
    if (addr) showWalletOpen(addr);
  } catch {}
  connect();
})();
