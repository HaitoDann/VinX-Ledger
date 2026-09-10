# ADR 0046 — BLS aggregate co-signatures for block finality

- **Statut :** 🔧 Implémenté (`vinx-crypto/bls.rs`, co-signatures BLS dans les blocs).
- **Catégorie :** Cryptographie · Consensus · **Priorité :** 🔴 haute
- **Date :** Août 2026 · durci septembre 2026
- **Liens :** remplace les N signatures Ed25519 individuelles dans les blocs par une
  signature BLS12-381 agrégée unique ; permet de supprimer `MAX_ACTIVE_SET_SIZE` (ADR 0038).
- **Révisé par :** [ADR 0070](./0070-authentification-proposeur-registre-bls.md) (registre BLS
  indexé, auth proposeur) et [ADR 0075](./0075-genese-enrolement-validateurs.md) (clé BLS **obligatoire**
  au bonding, PoP liée à l'identité).

---

> ⚠️ **Révision (septembre 2026, ADRs 0070/0075).** Deux points de cet ADR ont évolué depuis
> la rédaction initiale :
> - **La clé BLS n'est plus optionnelle.** Le « mode dégradé Ed25519-only » évoqué en §4 et §6
>   est **supprimé** : une clé BLS valide **et** une Proof-of-Possession sont exigées au bonding
>   (ADR 0075 §3.1 — invariant de liveness ; un validateur sans clé BLS ne peut pas co-signer,
>   donc ne doit pas pouvoir bloquer un quorum). `bls_pub_key` et `bls_pop` ne sont plus `Option`
>   côté validation d'entrée.
> - **La PoP est liée à l'identité.** Elle ne signe plus la seule clé BLS mais
>   `bls_pub_key ‖ validator_address ‖ chain_id` (DST `VINX_BLS_POP_V2`, voir `pop_message`), ce
>   qui empêche de rejouer une PoP sous une autre adresse ou une autre chaîne (ADR 0075 §3.2).
> - Le DST des co-signatures est `VINX_BLS_COSIG_V1`.

## 1. Contexte

Chaque bloc VinX contient actuellement une `Vec<BlockSignature>` : une signature Ed25519
de 64 octets par validateur actif. Avec N = 21 validateurs :
- Taille signatures/bloc : 21 × 64 = **1 344 octets**
- Vérification : 21 vérifications Ed25519 indépendantes

Au-delà de ~100 validateurs, ces co-signatures individuelles :
1. Alourdissent chaque bloc (N × 64 octets)
2. Augmentent le temps de vérification O(N)
3. Stressent la couche gossip (propagation de chaque co-sig individuelle)

La suppression du plafond dur `MAX_ACTIVE_SET_SIZE` (ADR 0038 révisé) rend ce problème
concret dès que la gouvernance dépasse ~100 validateurs.

## 2. Décision

Remplacer les N signatures Ed25519 individuelles dans le champ `signatures` du `Block`
par **une unique signature BLS12-381 agrégée**, identifiant aussi la liste de signataires.

La courbe BLS12-381 (IETF draft-irtf-cfrg-bls-signature) offre :
- **Agrégation** : N signatures BLS → 1 signature de 48 octets
- **Vérification agrégée** : 1 pairing check (≈ coût fixe) au lieu de N
- **Taille** : 48 octets quelle que soit N

### Périmètre de l'ADR

| Usage | Schéma retenu |
|---|---|
| Co-signatures de blocs (validateurs) | **BLS12-381** (cet ADR) |
| Signatures de transactions (comptes) | Ed25519 (inchangé) |
| Clés de gouvernance | Ed25519 (inchangé) |

La clé Ed25519 existante des validateurs est conservée comme **identifiant de vote et clé de gouvernance**. Une clé BLS distincte est utilisée exclusivement pour les co-signatures de blocs. Les validateurs publient leur clé BLS lors de l'admission dans le pool (champ additionnel dans la transaction `AddValidator`).

## 3. Schéma de signature BLS (IETF BLS12-381)

```
Scheme : BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_POP_
Key size : 48 bytes (G1 public key)
Signature size : 96 bytes (G2 element)
Aggregated sig : 96 bytes (single G2 element for all N signers)
```

**Proof of Possession (PoP)** : chaque validateur prouve la possession de sa clé BLS
lors de l'admission (`Register`). Cela évite les attaques rogue-key lors de l'agrégation.

## 4. Modifications structurelles

### `Block` / `BlockHeader`
```
Avant : signatures: Vec<BlockSignature>   // une par co-signataire
Après : bls_cosig:  Option<BlsAggregate>  // une signature agrégée
         cosigners: BitVec                 // 1 bit par position dans le set actif
```

`BitVec` de N bits encode quels validateurs (par leur rang trié dans le set actif) ont
signé. Taille : ⌈N/8⌉ octets. Pour N = 21 : 3 octets. Pour N = 1000 : 125 octets.

### `ValidatorPoolEntry`
```rust
pub bls_pub_key: Option<[u8; 48]>,  // G1 point (compressed), None avant ADR 0046
pub bls_pop:     Option<[u8; 96]>,  // Proof-of-Possession
```

### Transaction `AddValidator`
Nouveau champ optionnel `bls_pub_key: Option<[u8; 48]>`. Sans clé BLS, le validateur
participe avec Ed25519 uniquement (mode dégradé, admis en transition).

## 5. Bibliothèque : `blst`

Crate retenu : [`blst`](https://crates.io/crates/blst) (Microsoft Research / Ethereum
Foundation). Raisons :
- Implémentation de référence BLS12-381 (utilisée par Ethereum, Filecoin)
- Constant-time, résistant aux side-channels
- API Rust safe avec wrapper zero-copy
- Pas de dépendance à OpenSSL

```toml
# vinx-crypto/Cargo.toml
[dependencies]
blst = "0.3"
```

## 6. Chemin de migration (rétrocompat bloc-par-bloc)

1. **Phase 1** (cet ADR) : `signatures` reste mais `bls_cosig` est ajouté optionnel.
   Les blocs peuvent porter l'un ou l'autre. Vieux nœuds ignorent `bls_cosig`.
2. **Phase 2** (gouvernance) : un paramètre `bls_required_at_height` active la vérification
   BLS-only au-delà d'une hauteur donnée.
3. **Phase 3** : `signatures: Vec<BlockSignature>` retiré.

## 7. Implémentation

### `crates/vinx-crypto`
- `bls.rs` : wrapper autour de `blst` — `BlsKeyPair`, `BlsPubKey`, `BlsSignature`,
  `bls_sign(sk, msg)`, `bls_verify(pk, sig, msg)`,
  `bls_aggregate(sigs)`, `bls_verify_aggregate(pks, agg_sig, msg)`, `bls_pop_prove`, `bls_pop_verify`.

### `crates/vinx-core`
- `block.rs` : ajout `bls_cosig: Option<BlsAggregate>` et `cosigners_bitvec: Vec<u8>`.
- `validator_pool.rs` : ajout `bls_pub_key`, `bls_pop`.

### `crates/vinx-state`
- `world_state.rs` : `verify_block_bls_cosig()` — vérification à l'application P2P.

### `crates/vinx-node`
- `producer.rs` : signature BLS du bloc produit.
- `p2p.rs` : agrégation des co-signatures BLS reçues par gossip.

## 8. Conséquences

**Positif**
- Blocs ultra-compacts quel que soit N (96 octets de co-sig, indépendant du nombre de validateurs).
- Vérification O(1) au lieu de O(N).
- Suppression justifiée de `MAX_ACTIVE_SET_SIZE` — plus de contrainte gossip.

**Négatif / Risques**
- Courbe BLS12-381 moins auditable que Ed25519 (implémentation plus complexe).
- Les validateurs doivent stocker et gérer deux types de clés.
- La vérification d'agrégat nécessite la connaissance du set actif (pour reconstruire les PKs).
