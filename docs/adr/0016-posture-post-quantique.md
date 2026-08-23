# ADR 0016 — Posture post-quantique

- **Statut :** Proposé — 🟢 future (mainnet horizon)
- **Date :** Août 2026
- **Portée :** Cryptographie — chemin de migration vers des primitives résistantes aux ordinateurs quantiques.
- **Décideur :** VinX Labs.

---

## 1. Problème

Ed25519 et BLS12-381 ne sont **pas résistants aux ordinateurs quantiques** (algorithme de
Shor). À horizon 10-15 ans, un ordinateur quantique suffisamment puissant pourrait casser
les clés publiques exposées sur la chaîne.

VinX expose les clés publiques Ed25519 des validateurs et des utilisateurs (dans les
en-têtes et les transactions) — elles sont stockées de manière permanente sur la chaîne.

## 2. Contexte favorable (ce qui est déjà là)

- Le **versioning d'adresse** est possible via Bech32 (ADR 0051) — l'HRP peut évoluer,
  une nouvelle version d'adresse peut coexister.
- Le **schéma de signature est enfichable** conceptuellement (le type `VinxSignature` est
  dans `vinx-crypto` et pourrait être étendu pour supporter d'autres schémas).
- NIST a standardisé des candidats PQ en 2024 : **ML-DSA (CRYSTALS-Dilithium)** pour les
  signatures, **ML-KEM (CRYSTALS-Kyber)** pour l'échange de clé.

## 3. Direction

- **Ne pas implémenter maintenant.** Ed25519 reste la meilleure option pratique aujourd'hui
  (vitesse, taille, maturité des bibliothèques).
- **Documenter le chemin de migration** :
  1. Introduire un discriminant de type de signature dans l'en-tête de transaction.
  2. Supporter ML-DSA comme type alternatif (`SignatureType::MLDSA`).
  3. Période de transition : les deux types acceptés en même temps.
  4. Sunset Ed25519 après migration du set de validateurs.
- La migration BLS12-381 (ADR 0046) est une étape intermédiaire utile (signature agrégée,
  différente topologie d'exposition des clés).
- Surveiller la maturité des libs Rust (dilithium3, ml-dsa-rust).

## 4. Critères de validation (à écrire avant Accepté)

- [ ] Un discriminant de type de signature est introduit sans casser la sérialisation existante.
- [ ] ML-DSA fonctionne end-to-end (génération, signature, vérification) dans `vinx-crypto`.
- [ ] L'ancien schéma (Ed25519) et le nouveau (ML-DSA) peuvent coexister sur le même réseau.
- [ ] `cargo test --workspace` vert.

## 5. Dépendances

- Phase 5 (mainnet) — non urgent avant.
- ADR 0051 (primitives crypto) : base de migration.
