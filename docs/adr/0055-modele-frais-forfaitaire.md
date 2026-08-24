# ADR 0055 — Modèle de frais forfaitaire (flat fee)

- **Statut :** Vérifié — ✅ implémenté avant la formalisation des ADRs
- **Date :** 2025 (pré-ADR) · Formalisé août 2026
- **Portée :** Économie — mécanisme de frais de transaction L1.
- **Décideur :** VinX Labs.
- **Crate :** `crates/vinx-core` — `src/transaction.rs` ; `crates/vinx-state`

---

## 1. Contexte

La plupart des blockchains utilisent un marché de frais (gas en Ethereum, fee bidding en
Bitcoin). Ces modèles sont complexes à prévoir pour l'utilisateur et favorisent les
transactions avec gros budgets. VinX choisit un modèle plus simple, adapté à une monnaie pure.

## 2. Décision — Frais forfaitaires + plancher base-fee

**Modèle : frais forfaitaire (flat fee) par type de transaction.**

- `BASE_FEE = 100_000_000_000_000 atoms` (= 0,0001 VINX) — plancher immuable, gravé dans
  la genèse.
- Chaque transaction doit inclure `fee >= BASE_FEE` (sauf exemptions ci-dessous).
- Il n'y a **pas de composante proportionnelle à la taille** du payload (ni gas, ni fee/octet).
- Les frais vont **intégralement au producteur du bloc** immédiatement (hors pool d'époque).

**Exemptions :**
- `Stake (0x02)` et `Unstake (0x03)` : fee ZERO (ADR 0009 — décision assumée, anti-spam par
  plafond de déliaisons par compte à la place).
- `SlashValidator (0x07)` : fee ZERO (le slash est récompensé par 10 % du bond slashé).

**Admission dans le mempool (`admission_cost_atoms`) :**
- Transfer : `fee`
- Stake : `amount + fee` (le bond est préservé)
- Unstake : `fee`
- Autres : `fee`

Le mempool vérifie que `account.balance >= admission_cost_atoms` avant d'accepter une tx.

## 3. Pourquoi le forfait plat ?

| Critère | Forfait plat | Gas-market | Fee/octet |
|---|---|---|---|
| Prévisibilité pour l'utilisateur | ✅ Excellent | ❌ Variable | 🟡 Partiel |
| Résistance aux payloads géants | ❌ Faible (→ ADR 0035) | ✅ Cher | ✅ Naturelle |
| Simplicité d'implémentation | ✅ | ❌ | 🟡 |
| UX (simple transfer) | ✅ | ❌ (gas estimation) | ✅ |

Le forfait plat convient parfaitement au cas d'usage principal (transfert VINX). La résistance
aux gros payloads sera ajoutée par ADR 0035 (bornes de ressources par tx).

## 4. Évolution prévue

- **ADR 0035** : plafonds de taille de payload + composante fee/octet pour les gros payloads
  (sans casser le forfait pour les transferts normaux).
- **Congestion** : la congestion est gérée par le base-fee (ajustement de la valeur `BASE_FEE`
  par gouvernance), pas par des blocs rapprochés (ADR 0043).

## 5. Critères de validation

- [x] `Transfer` avec `fee < BASE_FEE` rejeté.
- [x] `Stake`/`Unstake` avec `fee = 0` acceptés.
- [x] Les frais vont au compte du proposeur à chaque bloc.
- [x] L'invariant de supply est maintenu (frais transférés, non détruits).
- [x] `cargo test --workspace` vert.
