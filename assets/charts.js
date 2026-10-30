/* Graphiques VinX Ledger, communs au nœud et au portefeuille :
   échelle robuste aux pics et bulle au survol. */

/* Haut de l'échelle : un pic isolé (nœud arrêté, rattrapage) ne doit pas écraser la courbe.
   Renvoie { hi, capped } : capped = nombre de valeurs au-dessus de hi, dessinées tronquées. */
function chartScale(values, floor = 0) {
  const sorted = values.filter((v) => Number.isFinite(v)).sort((a, b) => a - b);
  const max = sorted[sorted.length - 1] || 0;
  const p90 = sorted[Math.floor(sorted.length * 0.9)] || 0;
  let hi = max;
  if (p90 > 0 && max > 4 * p90) hi = p90 * 2.5;
  hi = Math.max(hi, floor) * 1.12 || 1;
  return { hi, capped: values.filter((v) => v > hi).length };
}

/* Points de survol, rattachés au graphique par data-chart. */
const CHART_POINTS = {};
let chartSeq = 0;
function chartPoints(points) { const id = "c" + chartSeq++; CHART_POINTS[id] = points; return id; }

/* À appeler après avoir inséré des graphiques : ligne verticale et bulle sur le point le plus proche. */
function bindCharts(root = document) {
  root.querySelectorAll("svg[data-chart]:not([data-bound])").forEach((svg) => {
    const pts = CHART_POINTS[svg.dataset.chart];
    svg.dataset.bound = "1";
    if (!pts || pts.length < 2) return;
    const box = svg.parentElement;
    if (getComputedStyle(box).position === "static") box.style.position = "relative";
    const tip = document.createElement("div");
    tip.className = "tip"; tip.hidden = true; box.appendChild(tip);
    const ns = "http://www.w3.org/2000/svg", vb = svg.viewBox.baseVal;
    const line = document.createElementNS(ns, "line");
    line.setAttribute("class", "hover-line"); line.setAttribute("y1", 0); line.setAttribute("y2", vb.height);
    line.setAttribute("vector-effect", "non-scaling-stroke");
    svg.appendChild(line);
    const move = (e) => {
      const r = svg.getBoundingClientRect();
      const x = ((e.clientX - r.left) / r.width) * vb.width;
      let best = pts[0];
      for (const p of pts) if (Math.abs(p[0] - x) < Math.abs(best[0] - x)) best = p;
      line.setAttribute("x1", best[0]); line.setAttribute("x2", best[0]); line.style.display = "block";
      tip.innerHTML = best[2]; tip.hidden = false;
      const b = box.getBoundingClientRect();
      const px = r.left - b.left + (best[0] / vb.width) * r.width;
      const py = r.top - b.top + (best[1] / vb.height) * r.height;
      const half = tip.offsetWidth / 2 + 4;
      tip.style.left = Math.min(Math.max(px, half), box.clientWidth - half) + "px";
      tip.style.top = Math.max(py, tip.offsetHeight + 6) + "px";
    };
    svg.addEventListener("pointermove", move);
    svg.addEventListener("pointerdown", move);
    svg.addEventListener("pointerleave", () => { tip.hidden = true; line.style.display = "none"; });
  });
}

/* Squelettes de chargement : retirés au premier rendu réussi. */
function skeleton(selector) { document.querySelectorAll(selector).forEach((e) => e.classList.add("skeleton")); }
function loaded(root = document) { root.querySelectorAll(".skeleton").forEach((e) => e.classList.remove("skeleton")); }

/* ---------- Mise en forme commune ---------- */

/* Dates : temps relatif partout, date exacte au survol, mis à jour tout seul. */
function relTime(ms) {
  const s = Math.round((Date.now() - ms) / 1000);
  if (s < 5) return "à l'instant";
  if (s < 60) return `il y a ${s} s`;
  if (s < 3600) return `il y a ${Math.floor(s / 60)} min`;
  if (s < 86400) return `il y a ${Math.floor(s / 3600)} h`;
  if (s < 7 * 86400) return `il y a ${Math.floor(s / 86400)} j`;
  return new Date(ms).toLocaleDateString("fr-FR", { day: "numeric", month: "short", year: "numeric" });
}
function when(ms) {
  if (!ms) return "—";
  const full = new Date(ms).toLocaleString("fr-FR", { dateStyle: "long", timeStyle: "medium" });
  return `<time data-ts="${ms}" datetime="${new Date(ms).toISOString()}" title="${full}">${relTime(ms)}</time>`;
}
setInterval(() => document.querySelectorAll("time[data-ts]").forEach((t) => { t.textContent = relTime(+t.dataset.ts); }), 15000);

/* Montants : au plus 4 décimales à l'écran, la valeur exacte au survol.
   `text` est un montant déjà formaté (« 1 234,567891234 »). */
function shortAmount(text, unit = "") {
  const [whole, frac = ""] = String(text).split(",");
  const shown = frac.length > 4 ? `${whole},${frac.slice(0, 4)}…` : text;
  const u = unit ? ` ${unit}` : "";
  return shown === text ? `${text}${u}` : `<span class="amt" title="${text}${u}">${shown}${u}</span>`;
}

/* Bouton copier, à côté d'une adresse, d'un identifiant ou d'un hash. */
const COPY_ICON = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="9" y="9" width="12" height="12" rx="2"/><path d="M5 15V5a2 2 0 0 1 2-2h10"/></svg>';
function copyBtn(text, label = "Copier") {
  return `<button type="button" class="copy" data-copy="${String(text).replace(/"/g, "&quot;")}" title="${label}" aria-label="${label}">${COPY_ICON}</button>`;
}
// En phase de capture : un clic sur « copier » n'ouvre pas la ligne du tableau.
document.addEventListener("click", async (e) => {
  const b = e.target.closest("button.copy");
  if (!b) return;
  e.preventDefault(); e.stopPropagation();
  try {
    await navigator.clipboard.writeText(b.dataset.copy);
    b.classList.add("done"); b.title = "Copié";
    setTimeout(() => { b.classList.remove("done"); b.title = "Copier"; }, 1200);
  } catch (_) { b.title = "Copie impossible : sélectionne le texte à la main"; }
}, true);
