# ADR 0050 — Vérification de preuves SP1 sur le L1

- **Statut :** Accepté — Architecture cible (non implémenté)
- **Catégorie :** L1 / Appchains · **Priorité :** 🔴 haute
- **Date :** Août 2026
- **Liens :** fonde les Appchains (ADR 0048, ADR 0049) ; remplace l'ancrage Merkle pur (ADR 0010) pour les Appchains avec preuves de validité ; s'appuie sur le modèle de confiance L1 (ADR 0001 révisé).

---

## 1. Contexte

L'ADR 0010 définit l'ancrage bondé : un opérateur ancre un hash d'état (`anchor_head`) et la L1 le stocke sans l'exécuter. Le modèle de confiance est **niveau 1 (bond + réputation)** : la L1 ne sait pas si l'état ancré est correct.

L'architecture cible de VinX L1 est un **settlement layer ZK-natif** : la L1 vérifie que la transition d'état ancré est **cryptographiquement correcte**, sans l'exécuter. C'est le passage du niveau 1 (bond + réputation) au **niveau 3 (preuves de validité ZK)** de l'ADR 0001.

Le prover retenu est **SP1** (Succinct Labs) — une zkVM RISC-V qui compile du Rust standard et génère des preuves Groth16 ou STARK vérifiables on-chain. Ce choix est motivé par :
- **Rust natif** — le circuit est du code Rust pur, pas un DSL de circuit spécialisé
- **SP1 Groth16** — preuve vérifiable en O(1), coût fixe indépendant de la complexité du programme
- **Maturité** — SP1 est production-grade depuis 2025 (utilisé par Succinct, EigenLayer, etc.)
- **Généralité** — une même infrastructure de vérification couvre toutes les Appchains

---

## 2. Décision

### 2.1 AnchorState avec preuve SP1

La transaction `AnchorState` (0x09) existante est étendue pour porter, en option, une **preuve SP1** :

```
AnchorState {
    module_id: Hash32,
    commitment: Hash32,          // state root (racine Merkle de l'état)
    state_diff: Vec<(Address, BalanceDelta)>,  // diff net des soldes (compact)
    sp1_proof: Option<Sp1Proof>, // preuve de validité ZK (None = niveau 1, Some = niveau 3)
    da_commitment: Option<DaCommitment>, // engagement Celestia (ADR 0034 révisé)
}
```

**Trois modes d'opération pour une Appchain :**

| Mode | `sp1_proof` | Niveau de confiance | Usage |
|------|-------------|---------------------|-------|
| **Bondé** | `None` | Niveau 1 (bond + réputation) | Modules légers, expérimentation |
| **ZK-vérifié** | `Some(...)` | Niveau 3 (validité cryptographique) | Appchains production |
| **ZK + DA** | `Some(...)` + `da_commitment` | Niveau 3 + DA vérifiable | Appchains avec ForceExit complet |

La transition du mode bondé au mode ZK-vérifié est une décision d'opérateur — le L1 accepte les deux.

### 2.2 Vérification on-chain de la preuve SP1

Le L1 implémente un **vérificateur SP1 Groth16** dans `vinx-state` :

```rust
fn verify_sp1_proof(
    proof: &Sp1Proof,
    vk: &Sp1VerifyingKey,   // clé de vérification publique du programme Appchain
    public_values: &[u8],   // (prev_state_root, next_state_root, block_hash)
) -> Result<(), ZkVerificationError>
```

**Coût de vérification :** une opération de pairing BLS12-381 (≈ 250 µs CPU) — constant, indépendant de la taille de l'Appchain.

**Clé de vérification (`vk`) :** chaque Appchain enregistre sa `Sp1VerifyingKey` au moment de la `ModuleOp::Register`. La VK est un hash du programme SP1 — si l'opérateur change son code, il doit re-enregistrer.

### 2.3 State diffs dans l'AnchorState

Le champ `state_diff` contient le **diff net des soldes** (balance changes) du lot de transactions traité. C'est l'information minimale nécessaire pour :
1. Permettre les **ForceExit** (ADR 0048) : l'utilisateur prouve son solde contre le state diff ancré.
2. Permettre le **Clearinghouse** (ADR 0049) : les messages cross-chain sont parsés depuis le state diff.
3. Permettre la **reconstruction d'état** en cas de panne du séquenceur.

Le state diff est inclus **en clair** dans la transaction L1 (pas dans la preuve) — il est compact (quelques Ko par lot) et permet à quiconque de reconstruire l'état sans exécuter le programme Appchain.

**Lien avec DA (ADR 0034 révisé) :** le state diff + l'historique complet des transactions sont publiés sur Celestia. Le `da_commitment` dans l'AnchorState engage ces données de façon vérifiable.

### 2.4 Ce que le L1 vérifie / ne vérifie pas

| Le L1 **vérifie** | Le L1 **ne vérifie pas** |
|-------------------|--------------------------|
| La preuve SP1 est cryptographiquement valide | La logique métier de l'Appchain |
| Le `prev_state_root` correspond au dernier `anchor_head` de l'Appchain | L'ordonnancement interne des transactions |
| Le `next_state_root` est cohérent avec le `state_diff` déclaré | La disponibilité des données historiques |
| L'opérateur est bien l'émetteur de la tx | Le comportement du séquenceur |

**Règle d'or (ADR 0001 invariant) :** « Aucune logique applicative ne s'exécute jamais dans le nœud VinX. » La vérification de preuve est une opération **cryptographique pure**, pas une exécution — elle respecte cet invariant.

### 2.5 Programme SP1 de référence (template Appchain)

Un template de programme SP1 est fourni dans `sdk/vinx-appchain-sp1/` :

```rust
// Logique minimale que tout programme SP1 d'Appchain doit implémenter
fn prove_state_transition(
    prev_state: AppchainState,
    transactions: Vec<AppchainTx>,
) -> (Hash32, Vec<BalanceDelta>) {
    let mut state = prev_state;
    let mut diffs = vec![];
    for tx in transactions {
        state.apply(tx, &mut diffs);
    }
    (state.merkle_root(), diffs)
}
```

Le template inclut : un compte model Account compatible avec le L1, la génération de Merkle proofs pour ForceExit, et le format de CrossMsg pour le Clearinghouse.

---

## 3. Conséquences

**Positif**
- La L1 devient un **settlement layer de vérité cryptographique** : plus de confiance dans l'opérateur pour la correction de l'état.
- Compatible avec n'importe quel type d'Appchain (DEX, lending, identité, etc.) sans changer le L1.
- Le modèle à deux niveaux (bondé / ZK) permet un déploiement progressif.
- Groth16 = taille de preuve constante (~200 octets), vérification O(1).

**Coûts / compromis**
- Le proving SP1 a une latence : de quelques secondes (simple) à quelques minutes (complexe). Les Appchains doivent en tenir compte dans leur cadence de lot.
- La VK doit être re-enregistrée à chaque mise à jour du programme Appchain.
- Ajoute une dépendance externe (`sp1-sdk`, `bn254` / `bls12-381` pour la vérification Groth16).
- La correction du vérificateur Groth16 est critique : audit de sécurité requis avant mainnet.

---

## 4. Roadmap

| Phase | Description |
|-------|-------------|
| **V1** | Vérification Groth16 SP1 on-chain ; `AnchorState` étendu avec `sp1_proof + state_diff` ; les deux modes (bondé / ZK) coexistent |
| **V2** | DA Celestia intégrée (`da_commitment` vérifié via light client Celestia) ; ForceExit complet |
| **V3** | Vérification STARK native (si SP1 passe à STARK on-chain) ; preuves récursives |

---

## 5. Notes d'implémentation

**Nouvelles dépendances `Cargo.toml` :**
```toml
[dependencies]
sp1-sdk = "2.0"           # proving + verification
bn254 = "0.1"             # pairing pour Groth16
```

**`vinx-core` :**
- `Sp1Proof { groth16_proof: Vec<u8>, public_values: Vec<u8> }` (canonique, vecteur doré ADR 0020)
- `Sp1VerifyingKey([u8; 32])` — hash du programme SP1
- `CoreError::ZkProofInvalid` — nouveau variant d'erreur
- `AnchorStatePayload` étendu (migration format, bump STORAGE_VERSION)

**`vinx-state` :**
- `verify_sp1_proof()` — appelé dans `apply_anchor_state()` si `sp1_proof.is_some()`
- `AppchainEntry` dans `ModuleEntry` : ajouter `verifying_key: Option<Sp1VerifyingKey>`
- `apply_state_diff()` — met à jour le séquestre selon les `BalanceDelta`

**`vinx-crypto` :**
- Réutilise le format Merkle existant pour les state proofs ForceExit

**Dépendances :** ADR 0010 (registre de modules — étendu) ; ADR 0048 (ForceExit — dépend des state diffs) ; ADR 0049 (Clearinghouse — dépend des payloads SP1 parsés) ; ADR 0034 révisé (DA Celestia).
