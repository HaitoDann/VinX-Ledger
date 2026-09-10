# ADR 0051 — Primitives cryptographiques de base (Ed25519, Bech32, SHA-256, Merkle)

- **Statut :** Vérifié — ✅ implémenté avant la formalisation des ADRs
- **Date :** 2025 (pré-ADR) · Formalisé août 2026
- **Portée :** Fondation cryptographique — identité, hachage, preuves d'appartenance.
- **Décideur :** VinX Labs.
- **Crate :** `crates/vinx-crypto`
- **Révisé par :** [ADR 0069](./0069-blake3-remplace-sha256.md) — la primitive de hachage **n'est plus SHA-256 mais BLAKE3**, décidée et implémentée avant toute genèse publique.

---

> ⚠️ **Révision (septembre 2026, ADR 0069).** Les §2.2, §2.3 et §2.4 ci-dessous décrivent
> SHA-256 comme fonction de hachage — c'était vrai jusqu'au pré-mainnet. **Le protocole hache
> désormais en BLAKE3** (`vinx_crypto::hash256`, adossé au crate `blake3`). Concrètement :
> - **Dérivation d'adresse** : `address = BLAKE3(public_key)[..20]` (et non SHA-256).
> - **Arbre de Merkle** : feuilles `BLAKE3(leaf)`, nœuds `BLAKE3(left ‖ right)`.
> - **Tous les `block_hash`, `tx_hash`, `state_root`, tiebreakers** passent à BLAKE3.
>
> Le reste de cet ADR (Ed25519, Bech32, structure de l'arbre de Merkle) reste valide.
> `STORAGE_VERSION` est passé à 20 : les bases pré-BLAKE3 sont refusées (pas de migration
> possible — les empreintes stockées appartiennent à un autre protocole).

## 1. Contexte

VinX est une monnaie pure Rust. Le choix des primitives cryptographiques est fondateur :
il détermine la taille des clés, la vitesse de vérification, la compatibilité avec les
standards blockchain et le chemin de migration post-quantique (ADR 0016).

## 2. Décisions (4 primitives, 1 crate)

### 2.1 Signature — Ed25519 (ed25519_dalek)

**Pourquoi Ed25519 ?**
- Standard bien établi (RFC 8032), bibliothèque `ed25519_dalek` auditée.
- Vérification rapide (~100 µs), clés courtes (32 o), signatures compactes (64 o).
- Supporte la vérification par lots (batch verify) — utile pour ADR 0015 (parallélisme).
- Rejeté : secp256k1 (Ethereum) — pas de vérification par lots native ; RSA — trop lourd.

**Structures :**
```
KeyPair    → { signing_key: ed25519_dalek::SigningKey }
PublicKey  → [u8; 32]  (sérialisé en JSON comme hex)
VinxSignature → [u8; 64] (sérialisé en JSON comme hex)
```

### 2.2 Adresse — Bech32 + SHA-256(pubkey)[..20]

**Pourquoi Bech32 ?**
- Format humainement lisible avec checksum intégré (détecte les erreurs de copier-coller).
- HRP `vinx` → toutes les adresses commencent par `vinx1...`.
- Même approche qu'Atom/Cosmos (format éprouvé pour les clés Ed25519).
- Rejeté : hex (sans checksum) ; base58check (plus difficile à taper, moins compatible).

**Dérivation :**
```
address = SHA-256(public_key_bytes)[..20]    // 20 octets = 160 bits
display  = bech32_encode("vinx", address)    // "vinx1..."
```

**Structure :** `Address([u8; 20])` — `Copy`, 20 octets inline, pas d'allocation heap.

### 2.3 Hachage — SHA-256 → Hash32

**Pourquoi SHA-256 ?**
- Standard universel, implémentation hardware accélérée sur x86/ARM.
- `Hash32 = [u8; 32]` — utilisé partout (block_hash, tx_hash, anchor_head, merkle nodes).
- Rejeté : Blake3 (moins de tooling côté navigateur/SDK) ; Keccak (identité Ethereum, pas bénéfique ici).

### 2.4 Preuve d'appartenance — Arbre de Merkle

**Structure :** arbre de Merkle binaire avec feuilles `SHA-256(leaf_data)` et nœuds
`SHA-256(left ‖ right)`. Fonctions :
- `merkle_root(leaves)` → `Hash32`
- `merkle_proof_for(leaves, index)` → `Vec<Hash32>`
- `verify_merkle_proof(root, leaf, proof)` → `bool`

Utilisé pour : `state_root` du WorldState, balance proofs des modules (ADR 0010), preuves
light-client (ADR 0014), commitment des Appchains (ADR 0050).

## 3. Critères de validation

- [x] Ed25519 : signature + vérification round-trip sans erreur (tests unitaires).
- [x] Bech32 : encode/decode idempotent ; mauvais HRP détecté.
- [x] SHA-256 : vecteurs de test NIST passent.
- [x] Merkle : 8 feuilles de test, proof de chaque feuille vérifiée.
- [x] `cargo test --workspace` vert.

## 4. Conséquences

- **Positif :** primitives bien connues, auditées, rapides.
- **Positif :** crate isolée `vinx-crypto` — testable indépendamment de l'état et du consensus.
- **Compromis :** Ed25519 n'est pas post-quantique (voir ADR 0016 pour le chemin de migration).
- **Compromis :** SHA-256 hash d'adresse → les clés publiques sont exposées en clair dans
  les tx (implication PQ : la clé est connue dès la première tx de l'adresse).
