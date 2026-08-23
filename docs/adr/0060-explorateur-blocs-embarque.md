# ADR 0060 — Explorateur de blocs embarqué

- **Statut :** Vérifié — ✅ implémenté avant la formalisation des ADRs
- **Date :** 2025 (pré-ADR) · Formalisé août 2026
- **Portée :** Interface — UI HTML statique embarquée dans le nœud pour explorer la chaîne.
- **Décideur :** VinX Labs.
- **Crate :** `crates/vinx-node` — `src/rpc/ui.rs`

---

## 1. Contexte

Pendant le développement et en testnet, il est utile de visualiser l'état de la chaîne
sans déployer un explorateur externe. Une UI légère directement dans le binaire du nœud
permet de valider rapidement le bon fonctionnement (blocs produits, transactions confirmées,
validateurs actifs).

## 2. Décision

**UI HTML statique** générée en Rust et servie par la route `GET /` de l'API Axum.

**Fonctionnalités :**
- Affichage du bloc le plus récent (height, hash, nb tx, proposeur, timestamp).
- Liste des derniers blocs (scroll).
- Solde d'une adresse (input + requête Ajax vers `/balance/:address`).
- Transactions d'un bloc (click sur un bloc).
- Statistiques : validateurs actifs, taille du mempool, `finalized_height`.

**Implémentation :**
- HTML/CSS/JS **inline** dans `ui.rs` (template Rust avec `include_str!` ou génération directe).
- Aucune dépendance externe (pas de CDN, pas de framework JS) → fonctionne sans réseau.
- Rafraîchissement automatique toutes les 12 s (block time).

## 3. Limites assumées

- UI non optimisée pour la production (pas de pagination infinie, pas de search avancée).
- Pas de support des modules ni des Appchains (futur).
- Pas de thème sombre/clair (cosmétique, non prioritaire).

Un explorateur de blocs production complet (indexeur dédié, API GraphQL, etc.) est hors scope
de ce ADR — il fera l'objet d'un ADR séparé quand VinX sera en testnet public.

## 4. Critères de validation

- [x] `GET /` retourne `200` avec du HTML valide.
- [x] Les données affichées correspondent aux données de l'API REST (`/block/latest`, `/validators`).
- [x] L'UI fonctionne sans réseau (aucune requête externe).
- [x] `cargo test --workspace` vert.
