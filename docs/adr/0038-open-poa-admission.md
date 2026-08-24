# ADR 0038 — Open PoS : admission permissionless, pool bondé

- **Statut :** Accepté — **non implémenté** à ce jour.
- **Catégorie :** Consensus & finalité / Tokenomics · **Priorité :** 🔴 haute
- **Date :** Août 2026 (révision majeure — remplace la version « bond + veto collectif »)
- **Liens :** concrétise le fair launch (ADR 0021, ADR 0033) ; s'articule avec les récompenses
  par époque (ADR 0028) ; prérequis du comité VRF (ADR 0029) ; dépend du consensus multi-validateur
  éprouvé (ADR 0002/0027/0031) avant d'ouvrir le pool.

> **Note (août 2026) :** cet ADR définit le **pool de validateurs éligibles** (admission,
> bond, warmup, unbonding, slashing). La **sélection pour un bloc donné** (qui signe quoi)
> n'est plus par score mais par **VRF Algorand-style** (ADR 0029). Les deux ADR sont
> complémentaires : 0038 = qui peut être dans le pool ; 0029 = qui est dans le comité du bloc.

---

## 1. Contexte

L'ADR 0038 précédent proposait un mécanisme d'admission « bond + veto collectif ». Ce design
a été abandonné après discussion : le veto collectif ouvre la porte à des cartels (les 14
validateurs actifs s'organisent pour bloquer le #15 concurrent), et n'est pas nécessaire si le
score de performance est le seul critère de sélection.

Le nouveau modèle : **n'importe qui** peut rejoindre le pool de validateurs en postant le bond
requis — sans approbation individuelle, sans veto. Le **set actif** (N validateurs qui
co-signent réellement les blocs) est déterminé par le **score de fiabilité** de chaque
validateur, recalculé et rotaté à chaque clôture d'époque.

## 2. Décision

### 2.1 Pool de validateurs (sans barrière de gouvernance)

Toute adresse postant le bond requis entre **automatiquement** dans le pool — sans validation
individuelle par l'admin ni veto collectif. L'admin conserve un seul pouvoir lié aux
validateurs : ajuster les bornes du bond via gouvernance (ADR 0011), dans les limites immuables
ci-dessous.

**Condition d'entrée :** `bond_atoms ≥ MIN_VALIDATOR_BOND_ATOMS` (gouvernable, par défaut
100 000 VinX, borné dans `[MIN_BOND_HARD_FLOOR, MAX_BOND_HARD_CAP]`).

**Warmup :** un validateur entrant dans le pool n'est **pas immédiatement éligible** au set
actif. Il doit compléter **3 époques complètes** (= 3 × `EPOCH_DURATION_SECS` de temps réel)
de co-signatures réelles avant d'entrer dans le classement. Son score est calculé à partir de
ses co-signatures effectives pendant le warmup ; il n'entre dans le ranking qu'à l'issue de la
3ᵉ époque complète.

> Exception bootstrap : si la taille du pool est inférieure à N, les validateurs en warmup
> sont comptés dans le set actif dès leur entrée (pas de luxe de refuser le quorum).

### 2.2 Score de fiabilité

Le score d'un validateur est son **taux de co-signature sur une fenêtre glissante de 7 jours**
(`VALIDATOR_SCORE_WINDOW_SECS = 604_800`).

```
score(v) = co_signatures_de_v_sur_7j / blocs_finalisés_sur_7j_où_v_était_dans_le_set_actif
```

**Règles fermes :**
- Aucune pondération par le bond : le bond sécurise, il ne multiplie pas le score.
- Aucune pondération par l'ancienneté dans le score lui-même (l'ancienneté sert uniquement
  de tiebreaker).
- Calculé exclusivement depuis les blocs finalisés (déterministe sur tous les nœuds).

**Score de départ :** 50 % (valeur sentinelle pendant le warmup, non utilisée dans le
ranking jusqu'à l'issue du warmup).

**Tiebreaker à score égal :** `SHA-256(epoch_number_le || validator_address)`. Uniforme,
déterministe, calculable par tous les nœuds, varie à chaque époque — aucun avantage
structurel pour aucune adresse.

### 2.3 Set actif et rotation par époque

Le set actif est composé des **N validateurs ayant le score le plus élevé** parmi ceux ayant
complété leur warmup.

**N** est gouvernable, modifiable de ±2 par modification, avec un cooldown de 7 jours entre
deux modifications. Seul un plancher dur est immuable :

```
MIN_ACTIVE_SET_SIZE = 5   (plancher dur, immuable — dessous, BFT n'offre aucune tolérance)
```

Il n'y a **pas de plafond dur** : la gouvernance fixe N librement au-dessus du plancher.
Note pratique : au-delà de ~100 validateurs, les co-signatures Ed25519 individuelles pèsent
sur le gossip ; l'agrégation BLS (ADR 0046) est recommandée pour de grands comités.

Le plancher de 5 s'active dès que le pool atteint 5 validateurs ; en dessous de 5, le set
actif = tout le pool (`set_size = min(N, pool_size)`).

La **rotation** a lieu à chaque clôture d'époque (même tick que ADR 0028) :
1. Recalculer le score de chaque validateur du pool ayant terminé son warmup.
2. Classer par score décroissant, tiebreaker SHA-256.
3. Les N premiers deviennent le nouveau set actif.
4. Les validateurs sortant du set actif retournent dans le pool (continuent d'observer).
5. Le quorum reste calculé sur le **set actif** courant (`⌈2N/3⌉`).

### 2.4 Bond gouvernable — bornes immuables

| Paramètre | Valeur | Gouvernable ? |
|---|---|---|
| `MIN_VALIDATOR_BOND_ATOMS` (défaut) | 100 000 VinX | Oui — ±25 % par modif, cooldown 7 j |
| `MIN_BOND_HARD_FLOOR` | 10 000 VinX | **Non — immuable** |
| `MAX_BOND_HARD_CAP` | 100 000 000 VinX | **Non — immuable** |
| Pas de modification | ±25 % | Immuable |
| Cooldown entre modifs | 7 jours | Immuable |

La gouvernance ne peut jamais descendre le bond en dessous du plancher ni le faire monter
au-dessus du plafond — un attaquant qui contrôle le comité ne peut ni rendre l'entrée triviale
(bond microscopique = Sybil gratuit) ni la rendre inaccessible (bond astronomique = monopole).

### 2.5 Équivocation — slash et blacklist

**Équivocation prouvée** (double-signature à la même hauteur, ADR 0030) :
1. 100 % du bond est slashé (10 % au rapporteur, 90 % au pot d'époque ADR 0028).
2. La **clé de validation** (adresse Ed25519 qui signe les blocs) est ajoutée à
   `banned_validator_keys` — un `HashSet<Address>` persisté dans le WorldState.
3. Toute tentative de rejoindre le pool avec cette clé est refusée.

La blacklist porte sur la **clé de validation**, pas l'adresse de paiement : l'attaquant
doit créer une nouvelle identité cryptographique, transférer des fonds, et attendre 3 époques
de warmup avant de réintégrer le pool. C'est la friction maximale sans KYC.

Le bannissement est irréversible — il ne peut pas être levé par gouvernance.

### 2.6 Downtime — sortie douce sans slash

Un validateur dont le score chute en dessous du top-N **sort du set actif** à la prochaine
rotation d'époque. Il reste dans le pool, continue d'observer et de co-signer (pour améliorer
son score), et peut réintégrer le set actif à une prochaine rotation si son score remonte.

**Aucun slash pour downtime.** La perte de revenus d'émission (zéro co-sigs = zéro part de
l'époque, ADR 0028) est la sanction naturelle.

### 2.7 Sortie volontaire — unbonding

Un validateur qui souhaite quitter soumet une transaction `Unstake` (son bond). Le bond entre
en période de déliaison (`UNBONDING_SECS = 3 jours`). Pendant la déliaison, la clé reste dans
le pool mais avec un marqueur `unbonding` — elle ne participe plus au set actif et son bond
reste slashable pendant la fenêtre de preuve.

## 3. Multi-validateurs par opérateur

Un opérateur peut faire tourner plusieurs validateurs (plusieurs clés, plusieurs bonds). Aucune
règle protocolaire ne l'interdit — c'est un choix de design assumé :

- **Bénéfique** si les nœuds sont géographiquement distribués : plus de co-signatures, plus de
  résilience.
- **Risqué** seulement si un opérateur contrôle ≥ 1/3 du set actif (liveness attack) ou
  ≥ 2/3 (mais la finalité BFT empêche le reorg des blocs finalisés).

La barrière est économique : contrôler 7/21 validateurs coûte 7 × bond + 7 × infrastructure.
Les co-signatures en lock-step sont observables on-chain par n'importe quel analyste.

## 4. Paramètres résumés

| Constante | Valeur | Modifiable ? |
|---|---|---|
| `N` (set actif par défaut) | 21 | Gouvernable, ±2, cooldown 7 j |
| `MIN_ACTIVE_SET_SIZE` | 5 | **Immuable** |
| ~~`MAX_ACTIVE_SET_SIZE`~~ | ~~101~~ | ~~Immuable~~ → **supprimé** (ADR 0046) |
| `ACTIVE_SET_STEP` | 2 | **Immuable** |
| `ACTIVE_SET_COOLDOWN_SECS` | 604 800 (7 j) | **Immuable** |
| `VALIDATOR_SCORE_WINDOW_SECS` | 604 800 (7 j) | Immuable |
| `VALIDATOR_WARMUP_EPOCHS` | 3 | Immuable |
| `MIN_VALIDATOR_BOND_ATOMS` (défaut) | 100 000 VinX | Gouvernable ±25 % cooldown 7 j |
| `MIN_BOND_HARD_FLOOR` | 10 000 VinX | **Immuable** |
| `MAX_BOND_HARD_CAP` | 100 000 000 VinX | **Immuable** |
| `BOND_STEP_BPS` | 2 500 (25 %) | **Immuable** |
| `BOND_COOLDOWN_SECS` | 604 800 (7 j) | **Immuable** |

## 5. Conséquences

**Positif**
- Admission permissionless : quiconque avec le bond peut participer — fair launch cohérent.
- Score de co-signature = seul critère : le travail de sécurité est la seule monnaie.
- Rotation époque : stabilité à court terme (pas de churn bloc par bloc), adaptation à moyen
  terme (downtime détecté en quelques heures).
- Intégration naturelle avec ADR 0028 : les co-signataires du set actif sont exactement ceux
  qui seront récompensés à la clôture d'époque.
- Pas de veto : aucun cartel ne peut bloquer l'admission d'un concurrent.

**Compromis assumés**
- Sybil possible si le bond est accessible : économiquement borné par le coût N × bond.
- Pas de preuve d'identité : la responsabilité légale repose sur la traçabilité on-chain et la
  réputation, pas sur un registre tiers.
- Warmup de 3 époques : un nouvel opérateur attend ~3 h avant d'être éligible au set actif.

## 6. Notes d'implémentation

**Nouveaux types (`vinx-core`) :**
- `ValidatorPoolEntry { bond_atoms, bonded_since_ts, warmup_epochs_remaining, status }`
- `PoolStatus` : `Warmup`, `Active`, `Benched`, `Unbonding`

**Changements `WorldState` (`vinx-state`) :**
- `validator_pool: BTreeMap<Address, ValidatorPoolEntry>` (append après `reliability`)
- `banned_validator_keys: HashSet<Address>` (append)
- `active_set_size: u32` (N courant, gouvernable)
- `last_bond_change_ts: u64` (cooldown bond)
- `last_active_set_size_change_ts: u64` (cooldown N)

**Nouveaux types de gouvernance (`vinx-core`) :**
- `GovernanceAction::UpdateActiveSetSize { new_size: u32 }`
- `GovernanceAction::UpdateMinValidatorBond { atoms: u128 }` (déjà prévu)

**Logique de rotation (appelée à `close_epoch_if_due`, ADR 0028) :**
1. Décrémenter `warmup_epochs_remaining` pour chaque entrée en warmup.
2. Calculer le score de chaque validateur ayant terminé le warmup.
3. Trier par score décroissant, tiebreaker SHA-256(epoch || addr).
4. Mettre à jour `status` : Active (top N), Benched (hors top N).
5. Bump STORAGE_VERSION : append-only sur les nouveaux champs.

**Bump de version :** STORAGE_VERSION 11 → 12 (append `validator_pool` + `banned_validator_keys`
+ `active_set_size` + timestamps de cooldown).

**Dépendances :** ADR 0002, 0027, 0028, 0031, 0040.
