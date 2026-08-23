# ADR 0013 — Cycle de vie de l'état (loyer & expiration)

- **Statut :** Proposé — 🟢 future
- **Date :** Août 2026
- **Portée :** État & scaling — gestion à long terme de la croissance de l'état.
- **Décideur :** VinX Labs.

---

## 1. Problème

Le dépôt existentiel (ADR 0026) empêche la création de comptes à coût nul, mais les
comptes **existants** ne sont jamais expirés. Un compte peut rester dans le WorldState
avec exactement `EXISTENTIAL_DEPOSIT_ATOMS` indéfiniment → croissance non bornée de
l'état à très long terme, même honnête.

La chaîne est prunée (les anciens blocs) mais l'**état (WorldState)** ne l'est pas.

## 2. Options envisagées

- **Loyer d'état (state rent)** : prélèvement périodique sur les comptes, proportionnel
  à leur taille. Comptes dormants finissent par passer sous ED et sont reapés.
- **Expiration par inactivité** : comptes non-actifs depuis N blocs marqués expirés ;
  nœuds d'archive conservent l'histoire.
- **Compaction des comptes dormants** : réduction de la représentation (merkle leaf →
  résumé) pour les comptes à solde nul.
- **Snapshot + résurrection** : état compacté périodiquement ; un compte peut prouver
  son solde historique via Merkle et « ressusciter ».

## 3. Direction

- La **première tranche** est ADR 0026 (dépôt existentiel + reaping des comptes vidés à 0).
  Elle résout le cas dominant (comptes poussière).
- Ce ADR couvre le **loyer d'état** et/ou l'expiration des comptes dormants au-delà.
- Ne pas implémenter avant d'avoir des données réelles de croissance de l'état en testnet.

## 4. Critères de validation (à écrire avant Accepté)

- [ ] La croissance de l'état est bornée sur simulation à 30 ans, saturé.
- [ ] Un compte expiré peut être ressuscité avec preuve Merkle (si loyer/snapshot adopté).
- [ ] L'invariant de supply est maintenu (les fonds d'un compte expiré sont traités par l'ADR).
- [ ] `cargo test --workspace` vert.

## 5. Dépendances

- ADR 0026 (dépôt existentiel) : prérequis, déjà implémenté.
- ADR 0014 (standard light-client) : les preuves de compte expiré réutilisent le format Merkle.
- Phase 5 (mainnet) : pertinent seulement à grande échelle.
