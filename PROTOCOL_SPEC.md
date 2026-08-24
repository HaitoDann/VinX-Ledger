# VinX Ledger — Spécification de protocole

> **Document de référence canonique.**
> En cas de contradiction entre ce fichier et tout autre document du repo, **ce fichier fait foi**.
> Mis à jour : août 2026.

**Légende d'état :**
- ✅ **Implémenté** — dans le code, tests verts
- 🔴 **Accepté / Cible** — décision prise, à implémenter
- 🟠 **Proposé** — en discussion, pas encore acté

---

## 1. Identité du réseau

| Paramètre | Valeur |
|---|---|
| Nom | VinX Ledger |
| Ticker | VINX |
| Chain ID | `42` |
| Adresses | Bech32, préfixe `vinx1` |
| Unité minimale | atom = 10⁻¹⁸ VINX (18 décimales) |
| RPC par défaut | `http://127.0.0.1:8545` |

---

## 2. Cryptographie ✅

| Rôle | Algorithme |
|---|---|
| Signatures de transactions | Ed25519 |
| Co-signatures de blocs | BLS12-381 (`blst`) |
| Hachage | SHA-256 |
| Arbre d'état | Merkle SHA-256 |
| Encodage des adresses | Bech32 |
| VRF (consensus cible) 🔴 | ECVRF RFC 9381 |

---

## 3. Tokenomics ✅

### 3.1 Supply

| Paramètre | Valeur |
|---|---|
| `MAX_SUPPLY` | **100 000 000 000 VinX** (100 milliards) |
| Prémine | **Aucun** — aucun token à la genèse |
| Burn | **Aucun** — le slash redistribue, jamais ne détruit |
| Immuabilité | Supply max et courbe d'émission gravées (ADR 0021/0040) |

### 3.2 Émission ✅

Décroissance exponentielle continue, intégrée sur les timestamps réels (jamais sur la hauteur) :

```
R(t) = R₀ · e^(−λt)
  λ = ln(2) / T_half
  T_half ≈ 20 ans
  R₀ ≈ 3,47 milliards VinX/an
```

- `mint_emission()` est appelé à **chaque bloc** ; elle crédite le producteur et incrémente `emitted_atoms`.
- `remaining_supply = MAX_SUPPLY − emitted_atoms` est dérivé à la demande (non stocké).

### 3.3 Invariant garanti à chaque bloc ✅

```
circulating_supply + epoch_dist_emission_pot + destroyed_atoms == emitted_atoms ≤ MAX_SUPPLY
```

### 3.4 Distribution de l'émission

| État | Règle |
|---|---|
| Actuellement ✅ | 100 % au producteur du bloc, immédiatement |
| Cible (ADR 0028) 🔴 | Par époque (1 h) : `PROPOSER_SHARE_BPS = 20 %` aux proposeurs ; 80 % aux co-signataires proportionnellement à leurs co-signatures effectives |

Les **frais** restent toujours au producteur immédiatement, hors époque.

---

## 4. Production de blocs ✅

| Paramètre | Valeur |
|---|---|
| Cadence | **12 s fixe** (ADR 0043 + ADR 0045) |
| Max tx/bloc | **3 000** (`max_block_txs`) |
| Max mempool | **100 000** (`max_mempool_size`) |
| `MAX_UNFINALIZED_DEPTH` | **64** blocs |
| Blocs vides | Produits normalement (émission continue, congestion via base-fee) |

> Il n'y a **pas** de block-on-demand, pas de heartbeat périodique. La cadence est strictement fixe.

---

## 5. Frais ✅

```
frais = base_fee × poids(type) × multiplicateur_congestion
```

| Paramètre | Valeur |
|---|---|
| `base_fee` initial | `100 000 000 000 000` atoms (= 0,0001 VINX) |
| Multiplicateur congestion | ×1 à **×3** (remplissage mempool) |
| Destinataire | **100 % au producteur du bloc**, immédiatement |
| Calcul | Forfait — **indépendant du montant** transféré |

**Poids par type de transaction :**

| Type | Poids |
|---|---|
| `Transfer` | `1` |
| `Stake` / `Unstake` | `0` (exempt — bond a déjà un coût d'immobilisation) |
| `AdminAction` | `0` (exempt) |
| `AnnounceUpgrade` | `0` (exempt) |
| `SlashValidator` | `0` (exempt — incité, pas pénalisé) |
| `AnchorState` | `1` |

---

## 6. Types de transactions ✅

| Code | Nom | Description |
|---|---|---|
| `0x01` | `Transfer` | Envoi de VINX entre comptes |
| `0x02` | `Stake` | Verrouillage en bond (entrée ou complément) |
| `0x03` | `Unstake` | Déverrouillage (file de déliaison 3 j) |
| `0x04` | `AnnounceUpgrade` | Annonce d'une mise à jour protocole (admin, ADR 0006) |
| `0x07` | `SlashValidator` | Slashing pour équivocation (preuve cryptographique requise) |
| `0x08` | `AdminAction` | Action de gouvernance — mono-admin ou comité K-of-M (ADR 0011) |
| `0x09` | `AnchorState` | Ancrage de module/Appchain (`ModuleOp` : Register/Anchor/Deregister) |
| `0x0B` | `ForceExit` 🔴 | Sortie forcée d'une Appchain sur le L1 (ADR 0048) |
| `0x0C` | `CrossMsgExpire` 🔴 | Expiration d'un message cross-Appchain (remboursement, ADR 0049) |

> Les codes `0x05` et `0x06` sont retirés (ADR 0007) — ajout/retrait de validateur passe par `AdminAction`.

---

## 7. WorldState — structure de données ✅

### Comptes (`accounts: BTreeMap<Address, Account>`)

| Champ | Type | Description |
|---|---|---|
| `balance` | `Amount` | Solde libre (atoms) |
| `bond` | `Amount` | Montant verrouillé en staking |
| `nonce` | `u64` | Compteur anti-rejeu |

### État global (`WorldState`)

| Champ | Type | Description |
|---|---|---|
| `accounts` | `BTreeMap<Address, Account>` | Tous les comptes |
| `validator_set` | `ValidatorSet` | Pool de validateurs, quorum `⌈2n/3⌉` |
| `circulating_supply` | `Amount` | Tokens détenus par les comptes |
| `emitted_atoms` | `u128` | Cumul des atoms mintés depuis la genèse |
| `emission_epoch_ts` | `u64` | Timestamp de référence de la courbe d'émission |
| `epoch_dist_emission_pot` | `Amount` | Atoms en attente de distribution (slash 90 % + ADR 0028) |
| `destroyed_atoms` | `u128` | Atoms perdus définitivement (dust de reaping) |
| `pending_unbonds` | `Vec<PendingUnbond>` | Déliaisons en cours — slashables jusqu'à `unlock_ts` |
| `fee_floor` | `Amount` | Plancher de frais gouvernable |
| `base_fee` | `Amount` | Frais courants (congestion) |
| `finalized_height` | `u64` | Hauteur finalisée (prefix-closed, ADR 0002) |

---

## 8. Consensus

### 8.1 Consensus implémenté — PoA Threshold ✅

| Règle | Valeur |
|---|---|
| Leader | Round-robin sur le **set actif** (jailés sautés — ADR 0027) |
| Quorum de finalité | `⌈2n/3⌉` sur le **set complet bondé** à cette hauteur |
| Jailing | Jamais ne réduit le quorum — sûreté sous partition |
| Finalité | Prefix-closed — `finalized_height` avance sur le plus long préfixe contigu ≥ quorum |
| Sûreté vérifiée | Banc n=3 : à 2/3 vivant la finalité avance, à 1/3 elle gèle |
| Fork-choice | `canonical_head` pur, réorg par snapshot+rejeu (ADR 0031) |

### 8.2 Consensus cible — PoS Algorand-style 🔴 (ADR 0029)

| Règle | Valeur |
|---|---|
| VRF | ECVRF RFC 9381 |
| Taille du comité | **n ≈ 100** — tirage uniforme parmi les bondés |
| Leader | Validateur avec la **sortie VRF la plus faible** (imprévisible) |
| Signatures | BLS12-381 agrégé — 100 signatures → 1, vérification O(1) |
| Finalité | BFT ≥ 67 % du comité |
| Anti-DoS | Leader inconnu jusqu'au dernier moment (contrairement au round-robin) |

---

## 9. Pool de validateurs ✅ / 🔴 (ADR 0038)

### 9.1 Admission

| Règle | Valeur |
|---|---|
| Mode | **Permissionless** — bond suffit, aucune approbation admin |
| Bond requis par défaut | **100 000 VinX** (gouvernable dans les bornes immuables) |
| `MIN_BOND_HARD_FLOOR` | **10 000 VinX** — immuable |
| `MAX_BOND_HARD_CAP` | **100 000 000 VinX** — immuable |

### 9.2 Warmup 🔴

- 3 époques complètes de co-signatures réelles avant d'entrer dans le ranking.
- Exception bootstrap : si le pool est en dessous de N, les validateurs en warmup participent immédiatement.

### 9.3 Set actif et rotation 🔴

| Paramètre | Valeur |
|---|---|
| N (set actif par défaut) | **21** validateurs |
| N gouvernable | ±2 par décision, cooldown 7 j |
| Score | Taux de co-signature sur une fenêtre glissante de **7 jours** |
| Rotation | À chaque clôture d'époque — top-N par score |
| Tiebreaker | `SHA-256(epoch_number_le ‖ validator_address)` |

### 9.4 Déliaison ✅

| Paramètre | Valeur |
|---|---|
| `UNBONDING_SECS` | **3 jours** (temps réel) |
| Bond slashable | Pendant toute la période de déliaison |

---

## 10. Époques et distribution (ADR 0028) 🔴

| Paramètre | Valeur |
|---|---|
| `EPOCH_DURATION_SECS` | **3 600 s (1 h)** par défaut |
| `PROPOSER_SHARE_BPS` | **20 %** — revient aux proposeurs proportionnellement à leurs blocs |
| Reste (80 %) | Co-signataires proportionnellement à leurs co-signatures valides dans l'époque |
| Frais | Toujours au producteur immédiatement — jamais mutualisés |
| Aucune pondération par bond | Seul le travail effectif compte (pas de plutocratie) |

---

## 11. Jailing et slashing ✅

### Jailing (ADR 0027)

- Un validateur est **jailé** s'il ne produit pas son bloc dans le délai.
- État dérivé de faits on-chain (proposeur effectif ≠ leader prévu).
- Le validateur jailé **sort du set actif** (rotation saute) mais le quorum de finalité reste calculé sur le **set complet bondé**.
- Sortie du jail : tx `Unjail` (à implémenter 🔴).

### Slashing (ADR 0027)

| Type | Sanction |
|---|---|
| Équivocation (double-vote prouvé) | **100 % du bond** |
| Répartition du slash | 10 % bounty au rapporteur ; **90 % dans `epoch_dist_emission_pot`** |
| Downtime | Suspension du set actif — **aucun slash économique** |

> Le slash ne détruit pas les tokens. Les 90 % restent dans `epoch_dist_emission_pot` jusqu'à distribution par époque (ADR 0028).

---

## 12. Gouvernance ✅ (ADR 0011)

| Mode | Règle |
|---|---|
| Legacy (sans policy) | Clé admin unique → exécution immédiate |
| Comité K-of-M | `threshold` approbations de signataires distincts (chaque approbation = une tx) |

**Actions de gouvernance disponibles :**

| Action | Effet |
|---|---|
| `RegisterValidator` | Ajoute un validateur au pool |
| `DeregisterValidator` | Retire un validateur |
| `UpdateFeeFloor { atoms }` | Modifie le plancher de frais |
| `SetAdminPolicy { policy }` | Passe en comité K-of-M ou retour admin unique |
| `RegisterModule` | Inscrit un module bondé (hors-L1) dans le registre |
| `DeregisterModule` | Retire un module |
| `SuspendValidator` / `UnsuspendValidator` | Gestion manuelle (admin) |
| `SetActiveValidatorBond` | Modifie le bond requis (dans les bornes immuables) |
| `SetMaxActiveValidators` | Modifie N (±2, cooldown 7 j) |

---

## 13. Appchains ZK (architecture cible) 🔴

### Principe

Le L1 ne fait que **vérifier et settlémenter** — jamais exécuter. Les Appchains s'exécutent hors-L1 et soumettent des preuves :

```
Appchain (hors-L1)
  └─ génère preuve SP1 (RISC-V zkVM, Groth16)
  └─ publie state diffs sur Celestia (DA)
        │
        └─► L1 : AnchorState { sp1_proof, state_diff, da_commitment }
              L1 vérifie Groth16 + engagement Celestia
              L1 settlémente les balances
```

### SP1 Proof Verification (ADR 0050) 🔴

```rust
AnchorState {
    appchain_id:    u32,
    prev_root:      Hash32,
    new_root:       Hash32,
    sp1_proof:      Option<Sp1Proof>,   // None = mode Bonded (niveau 1)
    state_diff:     Vec<(Address, BalanceDelta)>,
    da_commitment:  Option<DaCommitment>,
}
Sp1Proof { verifying_key: Sp1VerifyingKey([u8; 32]), proof_bytes: Vec<u8> }
```

**Deux modes :**
- **Niveau 1 (Bonded)** : pas de preuve ZK — le séquenceur poste son bond ; fraude détectable via DA.
- **Niveau 3 (ZK-verified)** : preuve Groth16 vérifiée on-chain — sécurité cryptographique complète.

### DA Celestia (ADR 0034) 🔴

- Les state diffs + données des Appchains sont publiés sur Celestia.
- Le L1 stocke un `DaCommitment` dans chaque `AnchorState`.
- Prérequis du slashing de fraude (ADR 0023).

### ForceExit / Escape Hatch (ADR 0048) 🔴

| Paramètre | Valeur |
|---|---|
| Type tx | `0x0B` |
| Mécanisme | `MerkleProof` du solde dans l'Appchain |
| Timeout séquenceur | **10 blocs L1** (~120 s) |
| En cas d'ignorance | Bond du séquenceur **slashé** |
| Garantie | Aucun utilisateur ne peut être censuré définitivement |

### Clearinghouse cross-Appchain (ADR 0049) 🔴

Flux asynchrone en 4 étapes :

```
LOCK (src L1) → CrossMsg enregistré L1 → MINT (dst L1) → ACK
```

| Champ | Description |
|---|---|
| `CrossMsg { id, src, dst, sender, recipient, amount, expiry_ts, status }` | Message cross-Appchain |
| `CrossMsgStatus` | `Pending` / `Delivered` / `Expired` / `Refunded` |
| Timeout / remboursement | `CrossMsgExpire (0x0C)` — remboursement automatique à expiry |

---

## 14. Dépôt existentiel ✅ (ADR 0026)

- Tout compte doit maintenir un solde minimum (dépôt existentiel) ou être **reapé**.
- Les atoms du reaping vont dans `destroyed_atoms` (clôture de l'invariant).
- Comptes exemptés : validateurs bondés, admin.

---

## 15. Mise à jour de protocole ✅ (ADR 0006)

- `AnnounceUpgrade { version, activation_ts }` — publie un préavis on-chain.
- L'upgrade s'active au **timestamp réel** indiqué (jamais à une hauteur).
- Forkless : les nœuds à jour appliquent les nouvelles règles sans fork.

---

## 16. API RPC (endpoints disponibles) ✅

| Méthode | Route | Description |
|---|---|---|
| GET | `/health` | Statut, hauteur, mempool, chain_id |
| GET | `/network/stats` | base_fee, emitted_atoms, circulating_supply, admin |
| GET | `/chain/height` | Hauteur courante |
| GET | `/block/<N>` | Bloc à la hauteur N |
| GET | `/tx/<HASH>` | Transaction par hash |
| GET | `/account/<ADDR>` | Solde, bond, nonce |
| GET | `/account/<ADDR>/txs` | Historique paginé |
| GET | `/account/<ADDR>/proof` | Preuve Merkle d'état |
| GET | `/validators` | Set de validateurs + quorum |
| GET | `/mempool/size` | Taille du mempool |
| GET | `/metrics` | Métriques Prometheus |
| POST | `/faucet/request` | Demande de tokens (devnet seulement) |
| POST | `/tx/submit` | Soumettre une transaction signée |
| GET | `/admin` | Console d'administration (auth requise) |

**Métriques Prometheus clés :**
```
vinx_chain_height
vinx_mempool_size
vinx_base_fee
vinx_emitted_atoms
vinx_circulating_supply
vinx_validator_count
vinx_blocks_produced_total
vinx_tx_submitted_total{status="ok"|"err"}
vinx_tx_in_block_total
```

---

## 17. Index des ADRs

### Phase 1 — Socle L1 ✅ Vérifié / Implémenté

| ADR | Titre résumé |
|---|---|
| 0051 | Primitives crypto : Ed25519, Bech32 `vinx1...`, SHA-256, Merkle |
| 0052 | Keystore wallet — Argon2id + AES-256-GCM |
| 0053 | Anti-replay : nonce, chain_id, expiry_height |
| 0054 | Types de transactions fondamentaux (0x01–0x0A) |
| 0055 | Modèle de frais forfaitaire (flat fee) |
| 0056 | Bond & queue de déliaison (staking, 3 jours unbonding) |
| 0057 | Mempool (100 000 tx, nonce ordering, staged/drain) |
| 0058 | Couche P2P de base (libp2p + gossipsub) |
| 0059 | API RPC REST (routes publiques + admin fail-closed + faucet) |
| 0060 | Explorateur de blocs embarqué (UI HTML statique) |
| 0061 | Stockage persistant (STORAGE_VERSION=14, migrations auto) |
| 0062 | WorldState — structure BTreeMap canonique |
| 0063 | Consensus PoA Threshold initial (round-robin + quorum BFT) |
| 0002 | Finalité au quorum (prefix-closed, `⌈2n/3⌉`) |
| 0003 | Slashing automatique de l'équivocation |
| 0005 | Temps réseau (MTP — Median Time Past) |
| 0006 | Mises à jour forkless (AnnounceUpgrade, timestamp) |
| 0007 | Unification gouvernance (AdminAction seul) |
| 0008 | Chain ID sûr (pas de défaut silencieux) |
| 0009 | Frais stake/unstake — plafond déliaisons anti-spam |
| 0010 | Registre de modules bondés (AnchorState 0x09) |
| 0011 | Gouvernance K-of-M (multisig) |
| 0015 | Vérification parallèle des signatures (multi-cœurs) |
| 0020 | Sérialisation canonique (vecteurs dorés BTreeMap) |
| 0021 | Immutabilité de la courbe d'émission |
| 0022 | P2P anti-DoS (guard, rate-limit, borne zstd) |
| 0026 | Dépôt existentiel + reaping |
| 0027 | Jailing / fiabilité des validateurs |
| 0031 | Règle de fork-choice (canonical_head) |
| 0040 | Émission progressive (T_half≈20 ans, sans prémine) |
| 0043 | Cadence fixe 12 s, 3 000 tx/bloc |
| 0045 | Abolition du heartbeat |
| 0046 | BLS12-381 agrégé (co-signatures) |

### Phase 2 — PoS Algorand-style 📐 Acceptés (architecture cible)

| ADR | Titre résumé |
|---|---|
| 0029 | Comité VRF Algorand-style (ECVRF RFC 9381, n≈100) |
| 0038 | Open PoS — pool permissionless, warmup, rotation |
| 0028 | Récompenses par époque (PROPOSER_SHARE_BPS = 20 %) |

### Phase 3 — Appchains ZK 📐 Acceptés (architecture cible)

| ADR | Titre résumé |
|---|---|
| 0050 | Vérification SP1 Groth16 on-chain |
| 0034 | Disponibilité des données — Celestia |
| 0048 | ForceExit / Escape Hatch (0x0B) |
| 0049 | Clearinghouse cross-Appchain (0x0C) |

### Proposés (en discussion) 💡

| ADR | Titre résumé | Phase |
|---|---|---|
| 0023 | Adjudication slashing de module (fraude opérateur) | 3 |
| 0024 | Infrastructure subnets : escrow bondé + racine récompense | 3 |
| 0030 | Accountability des co-signatures conflictuelles | 2 |
| 0032 | Garde-fous de gouvernance | 2 |
| 0033 | Genèse et bootstrap fair launch | 1 |
| 0035 | Bornes de ressources par transaction | 1 |
| 0036 | Churn des validateurs | 2 |
| 0037 | Propagation compacte des blocs (CompactBlock) | — |
| 0039 | Rémunération opérateurs modules (escrow + fee_schedule) | 4 |
| 0041 | Répartition émission par melt (Appchains) | 4 |
| 0042 | Époque de règlement de l'émission | 4 |
| 0044 | Garde-fous d'équité et amorçage | 4 |
| 0047 | Émission élastique à réservoir | 4 |
| 0012 | Gestion clés validateur (remote signer) | 5 |
| 0013 | Cycle de vie de l'état (loyer) | 5 |
| 0014 | Standard light-client (balance proof) | 3 |
| 0016 | Posture post-quantique (ML-DSA) | 5 |
| 0017 | Arrêt d'urgence & reprise | — |
| 0018 | Observabilité & SLO | 5 |
| 0019 | TLS natif rustls | 5 |

### Obsolètes / remplacés

| ADR | Remplacé par |
|---|---|
| 0004 (Invariant de supply) | ADR 0040 (nouvelle formule sans La Fonderie) |
| Heartbeat 10 min | ADR 0045 (cadence fixe) |
| Block-on-demand | ADR 0043/0045 (cadence fixe) |
| La Fonderie | ADR 0040 (émission progressive) |
| 0039 (Infrastructure subnets — ex-doublon) | Renommé ADR 0024 |

---

## 18. Constantes récapitulatives

```
MAX_SUPPLY                = 100_000_000_000 × 10^18 atoms  (100 Md VINX)
BLOCK_TIME_SECS           = 12
MAX_BLOCK_TXS             = 3_000
MAX_MEMPOOL_SIZE          = 100_000
MAX_UNFINALIZED_DEPTH     = 64

T_HALF_EMISSION_SECS      = ~630_720_000  (~20 ans)
R0_ATOMS_PER_SEC          = ~109_954_...  (≈ 3,47 Md VinX/an)

BASE_FEE_ATOMS_DEFAULT    = 100_000_000_000_000  (= 0,0001 VinX)
CONGESTION_MULTIPLIER_MAX = 3

UNBONDING_SECS            = 259_200  (3 jours)

MIN_BOND_HARD_FLOOR       = 10_000 VinX      (immuable)
MAX_BOND_HARD_CAP         = 100_000_000 VinX  (immuable)
MIN_BOND_DEFAULT          = 100_000 VinX      (gouvernable)
N_ACTIVE_DEFAULT          = 21                (gouvernable ±2, cooldown 7 j)

EPOCH_DURATION_SECS       = 3_600  (1 h, ADR 0028 — cible 🔴)
PROPOSER_SHARE_BPS        = 20     (ADR 0028 — cible 🔴)
COMMITTEE_SIZE_TARGET     = 100    (ADR 0029 — cible 🔴)

FORCE_EXIT_TIMEOUT_BLOCKS = 10     (~120 s, ADR 0048 — cible 🔴)

SLASH_BOUNTY_BPS          = 1_000  (10 % au rapporteur)
SLASH_EPOCH_POT_BPS       = 9_000  (90 % dans epoch_dist_emission_pot)

SCORE_WINDOW_SECS         = 604_800  (7 jours glissants, ADR 0038 — cible 🔴)
WARMUP_EPOCHS             = 3        (ADR 0038 — cible 🔴)

CHAIN_ID                  = 42
```
