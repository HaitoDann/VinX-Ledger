# ADR 0072 — `state_root` engage l'état de consensus

- **Statut :** Implémenté 🔧 — audit de sécurité de septembre 2026 (finding VINX-04)
- **Date :** Septembre 2026
- **Portée :** Consensus — engagement cryptographique de l'état · **Changement cassant le consensus**
- **Décideur :** VinX Labs
- **Révise :** ADR 0062 (structure du WorldState)
- **Crates :** `vinx-state` (`src/world_state.rs`)

---

## 1. Contexte

`compute_state_root` retournait la racine Merkle des **comptes** seule, et une feuille de
compte est `sha256(address ‖ balance ‖ nonce ‖ staked)`. La racine n'engageait donc rien
d'autre.

Restaient hors engagement — c'est-à-dire tout ce qui décide **qui produit les blocs et qui
gouverne** :

`validator_set`, `validator_pool` (bonds, clés BLS, PoP, clés VRF, statuts),
`banned_validator_keys`, `admin_address`, `admin_policy`, `pending_governance`,
`pending_upgrade`, `current_version`, `pending_unbonds`, `exit_queue`, `reliability`,
`epoch_beacon`, `modules`, `base_fee`, `fee_floor`, `emitted_atoms`, `circulating_supply`,
`destroyed_atoms`, `epoch_dist_emission_pot`, `active_set_size`,
`min_validator_bond_atoms`, `chain_id`.

**Démonstration :** deux `WorldState` aux comptes identiques mais dont le `validator_set`,
l'`admin_address` et l'`epoch_beacon` diffèrent produisaient des racines **égales**.

Or le `state_root` est le **seul** contrôle d'intégrité d'état sur tous les chemins de
validation de bloc (`p2p/mod.rs`, `sync.rs`, `reorg.rs`). Une divergence n'importe où dans
cette liste était donc **silencieuse et indétectable** : deux nœuds pouvaient diverger sur
l'ensemble du set de validateurs et sur la clé admin tout en acceptant mutuellement leurs
blocs. Le défaut ôtait aussi toute valeur au contrôle du snapshot-sync (ADR 0074) et
empêchait toute vérification par client léger (ADR 0014).

## 2. Décision

```
state_root = sha256( "VINX:state_root:v2" ‖ accounts_root ‖ consensus_root )
consensus_root = sha256( "VINX:consensus_root:v1" ‖ bincode(ConsensusCommitment) )
```

`ConsensusCommitment` est une structure d'emprunts couvrant explicitement, champ par champ,
la liste ci-dessus, plus `block_height` et `last_block_ts` (ADR 0076 §2.3).

### 2.1 Déterminisme

Le déterminisme est **structurel**, pas espéré :

- toutes les collections engagées sont des `BTreeMap` ou des `Vec` porteurs d'ordre ;
- `banned_validator_keys`, seul `HashSet`, est **trié** avant hachage — sans quoi l'ordre
  d'itération d'une table de hachage fuirait dans la racine et deux nœuds honnêtes
  calculeraient des racines différentes pour le même état ;
- les champs transitoires (`serde(skip)` : `current_block_ts`, `block_fees`, les caches
  Merkle) sont exclus.

### 2.2 Cas particulier — `reliability`

ADR 0027 qualifiait explicitement `reliability` de table « dérivée, hors `state_root` ».
Cet ADR **renverse** cette décision et l'engage, après vérification que c'est sûr :

- tous les intrants de `on_block_applied` sont déterministes — état (`reliability`,
  `validator_set`, `last_block_ts`) et champs d'en-tête (hauteur, proposeur, `block_ts`) ;
- la table est persistée (migration meta v11), donc un redémarrage ne la recalcule pas
  différemment.

L'argument de sûreté d'ADR 0027 — le quorum de finalité n'est jamais réduit par le jailing —
porte sur le **dénominateur du quorum**, pas sur l'engagement : il reste valable. Engager la
table transforme une divergence de jailing silencieuse en un `state_root` divergent, donc
détectable au bloc suivant.

### 2.3 Séparation de domaine

Deux tags distincts (`v2` pour la racine composée, `v1` pour le sous-arbre consensus)
empêchent qu'un condensat calculé dans un contexte soit réinterprété dans l'autre, et
permettent une révision ultérieure sans ambiguïté.

## 3. Conséquences

- **Hard fork.** Toutes les racines d'état changent. Assumé et fait **maintenant**, la
  chaîne étant en `0.1.0-alpha.1` sans réseau public à migrer. Voir ADR 0079 §4 :
  au-delà du lancement, un tel changement exige une hauteur d'activation.
- Un état de comptes vide n'implique plus une racine nulle : la racine engage toujours le
  consensus. Le test `test_state_root_empty_is_zero` est remplacé en conséquence.
- Toute divergence sur le set de validateurs, la clé admin ou le registre BLS devient
  **détectable au bloc suivant**, au lieu de rester invisible.
- Ajouter un champ de consensus au `WorldState` impose désormais de l'ajouter au
  `ConsensusCommitment` : un champ oublié rouvre exactement ce trou. À vérifier en revue.

## 4. Alternatives écartées

| Option | Pourquoi écartée |
|---|---|
| Arbre Merkle complet sur l'état de consensus | Coût et complexité injustifiés : aucune preuve d'inclusion partielle n'est requise sur ces champs ; un condensat canonique suffit |
| Engager le consensus dans un champ d'en-tête séparé | Doublerait les contrôles sur chaque chemin de validation, avec un risque d'en oublier un — précisément la classe de bug corrigée ici |
| Différer après le lancement | Un hard fork sur un réseau portant de la valeur coûte infiniment plus cher |

## 5. Critères de validation

- [x] Muter individuellement dix champs de consensus (`validator_set`, `admin_address`,
      `epoch_beacon`, `chain_id`, `active_set_size`, `min_validator_bond_atoms`,
      `emitted_atoms`, `destroyed_atoms`, `banned_validator_keys`, `validator_pool`) déplace
      la racine à chaque fois — `test_state_root_commits_to_consensus_state`.
- [x] L'ordre d'insertion dans `banned_validator_keys` n'influence pas la racine —
      `test_consensus_root_is_insertion_order_independent`.
- [x] Racine déterministe pour un état identique, non nulle sur comptes vides —
      `test_state_root_empty_accounts_still_commits_consensus`.
- [x] `cargo test --workspace` vert.
- [ ] Vecteur doré figeant l'encodage `consensus_root`, sur le modèle de
      `test_signing_bytes_golden_vector` — **à écrire avant le mainnet** (ADR 0080).
