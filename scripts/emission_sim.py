"""
VinX — Simulateur d'émission élastique à réservoir (ADR 0040).
Valeur retenue : r = 7 %/an. Usage : `python3 scripts/emission_sim.py`
(écrit fig1_comparaison_r.png et fig2_autoregulation.png dans le cwd).

Modèle :
    Invariant   : C + F = MAX            (circulation + Fonderie)
    Émission    : E = r * F              (fraction r de ce qui reste, par an)
    Melt        : M revient dans F       (recyclage, jamais détruit)
    Dynamique   : dC/dt = E - M = r*(MAX - C) - M

Équilibre (M constant) : C* = MAX - M/r ,  F* = M/r ,  E* = M
Constante de temps     : 1/r  (années)
"""
import numpy as np
import matplotlib.pyplot as plt

MAX = 100e9              # 100 milliards VINX (hard cap de circulation)
YEARS = 40
DT = 1.0 / 12           # pas mensuel
STEPS = int(YEARS / DT)
t = np.linspace(0, YEARS, STEPS)


def simulate(r, melt_fn, C0=0.0):
    """melt_fn(year, C, F) -> melt annuel M. Renvoie séries C, F, E(annuel), M(annuel)."""
    C, F = C0, MAX - C0
    Cs, Fs, Es, Ms = [], [], [], []
    for i in range(STEPS):
        E_year = r * F                         # débit d'émission annualisé
        M_year = melt_fn(t[i], C, F)           # débit de melt annualisé
        e, m = E_year * DT, M_year * DT        # flux sur le pas
        m = min(m, C)                          # on ne melt pas plus que la circulation
        e = min(e, F)                          # on n'émet pas plus que la Fonderie
        F += m - e
        C += e - m
        Cs.append(C); Fs.append(F); Es.append(E_year); Ms.append(M_year)
    return map(np.array, (Cs, Fs, Es, Ms))


# --- Scénarios d'usage (melt annuel) ---
def no_melt(year, C, F):                 # distribution pure, aucun usage
    return 0.0

def const_melt(M):                        # usage constant (Md/an) après coup
    return lambda year, C, F: M

def prop_melt(m):                         # usage ∝ circulation : m = fraction de C melt/an
    return lambda year, C, F: m * C

def adoption_then_shock(m_max, t_half=8, k=0.5, crash_year=22, crash=0.35):
    """Adoption logistique de l'intensité d'usage, puis krach à crash_year."""
    def f(year, C, F):
        m = m_max / (1 + np.exp(-k * (year - t_half)))     # montée en S
        if year >= crash_year:
            m *= crash                                      # l'usage s'effondre
        return m * C
    return f


# ===================== FIG 1 : comparaison de r (usage réaliste ∝ C) =====================
r_values = [0.05, 0.07, 0.10, 0.20]
R_CHOSEN = 0.07                           # valeur retenue (ADR 0040)
m_use = 0.15                              # 15 % de la circulation melt / an
colors = {0.05: "#2563eb", 0.07: "#7c3aed", 0.10: "#059669", 0.20: "#d97706"}

# population de mineurs : croissance logistique 200 -> 50 000
def miners(year):
    return 200 + (50000 - 200) / (1 + np.exp(-0.4 * (year - 10)))
N = np.array([miners(x) for x in t])

fig, ax = plt.subplots(2, 2, figsize=(14, 9))
fig.suptitle("VinX — Émission élastique  E = r·F   |   usage ∝ circulation (m = 15 %/an)",
             fontsize=14, weight="bold")

for r in r_values:
    C, F, E, M = simulate(r, prop_melt(m_use))
    star = "  ★ RETENU" if r == R_CHOSEN else ""
    lbl = f"r = {int(r*100)} %/an  (τ = {1/r:.0f} ans){star}"
    c = colors[r]
    lw = 3.0 if r == R_CHOSEN else 1.3
    ax[0, 0].plot(t, C / 1e9, color=c, label=lbl, lw=lw)
    ax[0, 1].plot(t, F / 1e9, color=c, label=lbl, lw=lw)
    ax[1, 0].plot(t, E / 1e9, color=c, label=lbl, lw=lw)
    ax[1, 1].plot(t, E / N / 1e3, color=c, label=lbl, lw=lw)   # émission par mineur (milliers VINX/an)
    # équilibre théorique
    Ceq = r * MAX / (r + m_use)
    ax[0, 0].axhline(Ceq / 1e9, color=c, ls=":", lw=0.8, alpha=0.6)

ax[0, 0].set_title("Circulation (Md VINX)"); ax[0, 0].set_ylabel("Md VINX")
ax[0, 1].set_title("Fonderie restante (Md VINX)")
ax[1, 0].set_title("Émission annuelle (Md VINX/an)"); ax[1, 0].set_xlabel("années")
ax[1, 1].set_title("Émission par mineur (k VINX/an)"); ax[1, 1].set_xlabel("années")
for a in ax.flat:
    a.legend(fontsize=8); a.grid(alpha=0.25); a.set_xlim(0, YEARS)
plt.tight_layout()
plt.savefig("fig1_comparaison_r.png", dpi=110)
print("écrit fig1_comparaison_r.png")


# ===================== FIG 2 : auto-régulation (r fixe, scénarios d'usage) =====================
r = 0.10
scenarios = [
    ("Aucun usage (M=0)",            no_melt,                       "#6b7280"),
    ("Usage constant 3 Md/an",       const_melt(3e9),               "#2563eb"),
    ("Usage ∝ circ. (15 %/an)",      prop_melt(0.15),               "#059669"),
    ("Adoption en S puis krach",     adoption_then_shock(0.20),     "#dc2626"),
]

fig2, ax2 = plt.subplots(1, 2, figsize=(14, 5))
fig2.suptitle(f"VinX — Auto-régulation à r = {int(r*100)} %/an   (le melt = thermostat)",
              fontsize=14, weight="bold")
for name, fn, c in scenarios:
    C, F, E, M = simulate(r, fn)
    ax2[0].plot(t, C / 1e9, color=c, label=name)
    ax2[1].plot(t, E / 1e9, color=c, label=name)
ax2[0].set_title("Circulation (Md VINX)"); ax2[0].set_ylabel("Md VINX"); ax2[0].set_xlabel("années")
ax2[1].set_title("Émission annuelle (Md VINX/an)"); ax2[1].set_xlabel("années")
for a in ax2:
    a.legend(fontsize=9); a.grid(alpha=0.25); a.set_xlim(0, YEARS)
plt.tight_layout()
plt.savefig("fig2_autoregulation.png", dpi=110)
print("écrit fig2_autoregulation.png")


# ===================== Résumé chiffré =====================
print("\n================  ÉQUILIBRES (usage ∝ circulation, m = 15 %/an)  ================")
print(f"{'r':>8} | {'τ (ans)':>8} | {'Circ. éq. C* (Md)':>18} | {'Émission éq. (Md/an)':>20} | "
      f"{'90 % de C* atteint à':>20}")
for r in r_values:
    C, F, E, M = simulate(r, prop_melt(m_use))
    Ceq = r * MAX / (r + m_use)
    Eeq = r * (MAX - Ceq)
    # temps pour atteindre 90 % de l'équilibre de circulation
    idx = np.argmax(C >= 0.9 * Ceq)
    t90 = t[idx] if C[idx] >= 0.9 * Ceq else float("nan")
    print(f"{int(r*100):>6} % | {1/r:>8.0f} | {Ceq/1e9:>18.1f} | {Eeq/1e9:>20.2f} | {t90:>18.1f} ans")

print("\n================  VALEUR RETENUE : r = 7 %/an (ADR 0040)  ================")
C, F, E, M = simulate(R_CHOSEN, prop_melt(m_use))
Ceq = R_CHOSEN * MAX / (R_CHOSEN + m_use)
Eeq = R_CHOSEN * (MAX - Ceq)
idx = np.argmax(C >= 0.9 * Ceq)
print(f"  tau = 1/r                     : {1/R_CHOSEN:.1f} ans")
print(f"  circulation d'equilibre C*    : {Ceq/1e9:.1f} Md VINX  (Fonderie {(MAX-Ceq)/1e9:.1f} Md)")
print(f"  emission d'equilibre E* = M*  : {Eeq/1e9:.2f} Md VINX/an")
print(f"  90 % de C* atteint a          : {t[idx]:.1f} ans")
print(f"  distribue en annee 1 (M=0)    : {C[11]/1e9:.2f} Md VINX (amorçage sans usage prealable)")

print("\n================  ANNÉE 1 (amorçage : Fonderie pleine, usage encore nul)  ================")
for r in r_values:
    C, F, E, M = simulate(r, prop_melt(m_use))
    # émission cumulée sur la 1re année
    emitted_y1 = C[11]  # ~12e pas
    print(f"r = {int(r*100):>2} %/an  ->  ~{emitted_y1/1e9:5.2f} Md VINX distribués la 1re année "
          f"(sans dépendre d'aucun usage préalable)")
