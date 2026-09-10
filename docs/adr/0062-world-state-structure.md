# ADR 0062 — WorldState — structure de l'état du monde

- **Statut :** Vérifié — ✅ implémenté avant la formalisation des ADRs
- **Date :** 2025 (pré-ADR) · Formalisé août 2026
- **Portée :** Protocole fondamental — structure canonique de l'état de la chaîne VinX.
- **Décideur :** VinX Labs.
- **Crate :** `crates/vinx-state` — `src/world_state.rs`

- **Révisé par :** [ADR 0072](./0072-state-root-engage-consensus.md) — le `state_root` engage
  désormais l'état de consensus, pas seulement les comptes (finding VINX-04)

> ⚠️ **Correction (septembre 2026, finding VINX-04).** `compute_state_root` ne renvoyait que
> la racine Merkle des **comptes** : `validator_set`, `validator_pool`, `admin_address`,
> `epoch_beacon` et une vingtaine d'autres champs de consensus n'étaient engagés par rien.
> Deux nœuds pouvaient diverger sur l'intégralité du set de validateurs et sur la clé admin
> tout en publiant un `state_root` identique — divergence silencieuse et indétectable.
> Tout nouveau champ de consensus ajouté à `WorldState` **doit** être ajouté au
> `ConsensusCommitment` d'ADR 0072, sous peine de rouvrir exactement ce trou.

---

## 1. Contexte

Le `WorldState` est **l'état complet** de la L1 à une hauteur donnée. Chaque bloc transforme
le `WorldState` de manière déterministe. L'empreinte du `WorldState` (son `state_root`)
est committée dans l'en-tête de chaque bloc → n'importe quel nœud peut vérifier qu'il a
le même état qu'un autre.

## 2. Décision — structure du WorldState

```rust
WorldState {
    // Comptes VINX
    accounts:      BTreeMap<Address, Account>,
    emitted_atoms: u128,         // Total émis (courbe exponentielle)
    epoch_pot:     u128,         // Atoms en attente de distribution aux validateurs
    destroyed_atoms: u128,       // Atoms détruits (reaping, brûlage futur)

    // Validateurs
    validators:    BTreeMap<Address, ValidatorEntry>,
    admin_policy:  AdminPolicy,  // Politique de gouvernance (K-of-M signataires)
    pending_governance: Vec<GovernancePendingItem>,

    // Jailing / fiabilité (ADR 0027)
    reliability:   BTreeMap<Address, ReliabilityRecord>,

    // Modules (ADR 0010)
    modules:       BTreeMap<Hash32, ModuleEntry>,

    // Misc
    genesis_hash:  Hash32,       // Empreinte de la configuration de genèse
    admin_address: Option<Address>, // Adresse admin legacy (si comité non configuré)
}
```

**Toutes les maps sont `BTreeMap`** (ordre canonique déterministe) → jamais `HashMap`
(ordre non-déterministe → divergence du `state_root` entre nœuds).

## 3. Compte (`Account`)

```rust
Account {
    balance:   u128,                // Atoms disponibles (transférables)
    staked:    u128,                // Atoms en bond
    nonce:     u64,                 // Dernier nonce accepté (anti-replay)
    unbonding: Vec<UnbondEntry>,    // [(amount, release_height)]
    bls_key:   Option<BlsPublicKey>, // Clé BLS12-381 (ADR 0046)
}
```

## 4. Calcul du state_root

> ⚠️ **Layout révisé (ADRs 0072 & 0069).** L'esquisse ci-dessous est conservée pour l'intuition,
> mais **le layout faisant foi est celui d'ADR 0072** : `state_root = hash256(DST ‖ accounts_root ‖
> consensus_root)`, où `consensus_root` engage **tous** les champs de consensus (dont
> `emission_started` et `foundry`, ajoutés depuis). Le hachage est **BLAKE3** (`hash256`, ADR 0069),
> plus SHA-256.

```
accounts_root = merkle_root( hash256(account_0) ‖ hash256(account_1) ‖ … ‖ hash256(account_n) )

consensus_root = hash256(
    validators ‖ validator_pool ‖ admin_policy ‖ reliability ‖ epoch_beacon ‖
    emitted_atoms ‖ epoch_pot ‖ destroyed_atoms ‖ emission_started ‖ foundry ‖ …
)   // liste exhaustive et faisant foi dans ADR 0072 (ConsensusCommitment)

state_root = hash256(DST ‖ accounts_root ‖ consensus_root)
```

Le `state_root` est inclus dans l'en-tête de bloc et co-signé par les validateurs →
garantit que tous les nœuds ont appliqué les mêmes transitions **et** partagent le même état
de consensus (VINX-04).

## 5. Transition (settle_block)

`WorldState::settle_block(block, chain_params)` est la **fonction pure et déterministe**
qui applique un bloc et retourne le nouveau `WorldState`. Règles :
1. Valider chaque tx (signature, nonce, chain_id, solde).
2. Appliquer les effets (transferts, staking, gouvernance, ancrage).
3. Calculer l'émission de l'époque (si fin d'époque).
4. Mettre à jour la fiabilité des validateurs (jailing).
5. Vérifier l'invariant de supply (`supply_invariant_holds()`).
6. Calculer le nouveau `state_root`.

## 6. Critères de validation

- [x] `settle_block` est idempotent (même bloc → même état).
- [x] `state_root` identique sur tous les nœuds après le même bloc (banc n=3).
- [x] `supply_invariant_holds()` passe sur tous les chemins (production, P2P, sync).
- [x] Toutes les maps sont `BTreeMap` (audité dans ADR 0020).
- [x] `cargo test --workspace` vert (tests de propriété dans `vinx-state/tests/property_tests.rs`).

## 7. Conséquences

- **Positif :** `BTreeMap` garantit l'ordre canonique → `state_root` déterministe.
- **Positif :** `settle_block` pure → testable sans nœud, parallélisable pour la vérif.
- **Compromis :** `BTreeMap` est plus lent que `HashMap` pour les grandes collections
  (O(log n) vs O(1)) → à surveiller à très grand nombre de comptes (Phase 5).
