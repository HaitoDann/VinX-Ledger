# ADR 0069 — BLAKE3 remplace SHA-256

- **Statut :** Décidé — à implémenter avant genesis block 0
- **Date :** Septembre 2026
- **Portée :** Protocole — fonction de hachage de base
- **Décideur :** VinX Labs
- **Crates :** `crates/vinx-core` — `src/block.rs`, `src/transaction.rs`, `src/merkle.rs` (si existe)
- **Liens :** ADR 0020 (sérialisation canonique — base du hachage) ; ADR 0051 (primitives cryptographiques — mis à jour par cet ADR)
- **⚠️ Contrainte absolue :** ce changement modifie tous les hashes produits. Il doit être fusionné **avant** que tout nœud produise le genesis block. Après le premier bloc, c'est un hard fork.

---

## 1. Contexte

SHA-256 est utilisé pour le hachage des blocs, des transactions et de l'arbre de Merkle. C'est la fonction par défaut héritée de l'initialisation du projet.

BLAKE3 est une fonction de hachage moderne (2019, IETF draft) conçue pour la performance sur du matériel moderne. Elle exploite le parallélisme SIMD (AVX2, NEON) et produit des sorties 256-bit.

| Critère | SHA-256 | BLAKE3 |
|---------|---------|--------|
| Vitesse (x86-64 AVX2) | ~600 MB/s | ~3 000-5 000 MB/s |
| Vitesse (ARM NEON) | ~400 MB/s | ~2 000 MB/s |
| Sécurité (collision) | 128 bits | 128 bits |
| Audit formel | Oui (NIST) | Oui (2019, Aumasson et al.) |
| Construction | Merkle-Damgård | Merkle tree interne |
| Résistance length extension | Non | Oui |

**Pourquoi maintenant et pas plus tard :** BLAKE3 change tous les hashes produits (hashes de blocs, de transactions, Merkle roots). C'est un changement de protocole breaking. Il ne peut être fait qu'avant le genesis block — après, tout nœud ayant le genesis hash incorrect ne peut pas rejoindre le réseau.

**Pourquoi pas Poseidon :** Poseidon est une fonction de hachage ZK-native (optimisée pour les circuits arithmétiques). VinX est un rail de paiement sans ZK (ADR 0064) — utiliser Poseidon serait de la complexité sans contrepartie.

## 2. Décision

Remplacer SHA-256 par **BLAKE3** dans tous les sites de hachage du protocole VinX.

### 2.1 Dépendance

```toml
# crates/vinx-core/Cargo.toml
[dependencies]
blake3 = "1"
```

SHA-256 (`sha2`) peut être retiré si aucun autre usage n'en dépend (à vérifier par grep).

### 2.2 Sites de remplacement

| Fichier | Ligne indicative | Usage |
|---------|-----------------|-------|
| `crates/vinx-core/src/block.rs` | :31-32 | Hash de l'en-tête de bloc |
| `crates/vinx-core/src/transaction.rs` | :145, :174 | Hash de transaction |
| `crates/vinx-core/src/merkle.rs` | (si existe) | Merkle root des txs |

Pattern de remplacement :

```rust
// Avant (SHA-256)
use sha2::{Sha256, Digest};
let hash: [u8; 32] = Sha256::digest(&data).into();

// Après (BLAKE3)
let hash: [u8; 32] = blake3::hash(&data).into();
```

Pour le hachage incrémental (zéro-alloc, ADR compagnon) :

```rust
let mut hasher = blake3::Hasher::new();
hasher.update(&part1);
hasher.update(&part2);
let hash: [u8; 32] = hasher.finalize().into();
```

### 2.3 Ce qui ne change pas

- Le format de sérialisation des blocs et transactions (Borsh/serde) — inchangé.
- Les signatures Ed25519 — inchangées (Ed25519 signe les bytes de la transaction, pas leur hash SHA-256).
- Les signatures BLS — inchangées.
- La logique de consensus — inchangée.

Seule la **valeur des hashes** change. Les structures de données qui stockent ces hashes (`block.hash`, `tx.hash`, `state_root`) ont le même type `[u8; 32]`.

## 3. Migration

Ce changement est **uniquement possible avant le genesis block**.

Procédure :
1. Implémenter le remplacement.
2. Mettre à jour `STORAGE_VERSION` dans `storage.rs` pour invalider tout état stocké avec SHA-256.
3. Fusionner dans main.
4. Tous les opérateurs du testnet reconstruisent depuis zéro (aucun nœud n'a encore de données — le testnet n'est pas encore lancé).
5. Distribuer le `genesis-testnet.json` après ce changement — le genesis hash sera calculé avec BLAKE3.

Si le testnet a déjà produit des blocs quand cette ADR est lue : ne pas implémenter. Documenter la décision comme « reportée au prochain réseau ».

## 4. Conséquences

**Positif**
- 5-8x plus rapide que SHA-256 sur du matériel moderne.
- Gain concret sur le hot path : hash de chaque tx à l'admission mempool, hash de l'en-tête à chaque bloc produit, Merkle root à chaque bloc.
- Résistance naturelle aux attaques length-extension (propriété de BLAKE3, absente de SHA-256).

**Coûts / compromis**
- Changement de protocole breaking — impraticable après genesis.
- BLAKE3 est moins connu que SHA-256 dans le domaine blockchain — quelques explications supplémentaires dans le whitepaper.
- La crate `sha2` peut être retirée si BLAKE3 est le seul hasher — à vérifier (d'autres crates de l'arbre de dépendances peuvent dépendre de sha2).

## 5. Critères de validation

- [ ] `blake3::hash(data)` produit un résultat de 32 octets — même type que `Sha256::digest(data)`.
- [ ] Le hash du genesis block est stable entre plusieurs démarrages à partir du même `genesis-testnet.json`.
- [ ] `cargo test --workspace` vert.
- [ ] `STORAGE_VERSION` incrémenté — aucun nœud ne peut démarrer avec des données SHA-256 sans être détecté.
