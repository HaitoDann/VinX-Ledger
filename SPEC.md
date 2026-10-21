# VinX Ledger — Spécification du protocole

> **Version de protocole :** 1 · **Statut :** brouillon
>
> Ce document est la **seule source de vérité** du protocole. Les autres documents y
> renvoient et ne recopient aucune valeur. Les décisions de fond sont dans
> [`docs/adr/0001`](./docs/adr/0001-modele-monetaire-et-gouvernance.md).

## 1. Conventions

- Tous les entiers sont non signés. Les montants sont des `u64` exprimés en
  **unités** : 1 VINX = 10⁹ unités.
- Toute arithmétique sur les montants est **vérifiée** : un dépassement rend la
  transaction ou le bloc invalide. Les sommes intermédiaires se font en `u128`.
- L'encodage binaire est **borsh**. Un décodeur doit rejeter toute entrée qui ne
  se réencode pas à l'identique (octets en trop, variantes inconnues).
- `H(x)` désigne BLAKE3-256. Tout hachage ou toute signature est précédé d'une
  **étiquette de domaine** (tableau en §10) pour qu'un message d'un type ne puisse
  jamais être rejoué comme un autre.
- Dans ce document, `"étiquette" ‖ x` se lit `longueur(étiquette) ‖ étiquette ‖ x`,
  la longueur tenant sur un octet. Sans ce préfixe, `vinx/tx` serait un début
  possible de `vinx/txid` et deux domaines pourraient produire le même message.

## 2. Cryptographie

### 2.1 Algorithmes

Chaque clé publique et chaque signature commence par un **octet d'algorithme** :

| Octet | Algorithme | Clé publique | Signature | Statut |
|---|---|---|---|---|
| `0x01` | Ed25519 | 32 o | 64 o | Actif |
| `0x02` | ML-DSA-44 | 1 312 o | 2 420 o | Réservé (activation par mise à jour) |

- Ed25519 est vérifié en **mode strict** (rejet des points de faible ordre et des
  signatures non canoniques).
- Un algorithme réservé est refusé tant qu'une mise à jour ne l'a pas activé (§9).

### 2.2 Adresses

```
adresse = H("vinx/address" ‖ clé_publique_typée)[0..20]
```

- Encodage texte : **Bech32m**, préfixe `vinx` (ex. `vinx1…`).
- L'adresse ne dépend que de la **première** clé du compte. Une rotation de clé
  (§5.3) conserve l'adresse.
- Comptes système, sans clé :
  - **Trésorerie** : `H("vinx/address" ‖ "treasury")[0..20]`.
  - L'**escrow** n'est pas un compte : c'est une structure de l'état (§4.2).

## 3. Comptes

```
Account {
    balance:       u64,
    nonce:         u64,
    key:           Option<PublicKey>,  // connue après la première dépense
    memo_required: bool,
}
```

- Un compte est créé en recevant un premier transfert.
- **Réserve minimale** : après toute transaction, un compte ordinaire doit avoir
  `balance ≥ min_reserve` (§11). Un transfert qui crée un compte doit donc
  apporter au moins `min_reserve`. La trésorerie n'est pas soumise à la réserve.
- La clé publique est stockée lors de la première transaction signée du compte.

## 4. État global

### 4.1 Contenu

```
State {
    accounts:   SMT<Address, Account>,
    escrow:     Escrow,
    admin:      AdminSet,          // 3 signataires, seuil 2
    validators: ValidatorSet,      // ensemble actif + changements programmés
    params:     Params,            // §11
    upgrade:    Option<PendingUpgrade>,
}
```

`state_root = H("vinx/state" ‖ racine_SMT(accounts) ‖ H(borsh(escrow, admin, validators, params, upgrade)))`.

### 4.2 Escrow

```
Escrow {
    tranches: Vec<Tranche>,   // ordonnées par échéance croissante
    period:   u64,            // durée d'une période, en secondes
}
Tranche { amount: u64, matures_at: u64 }   // horodatage Unix, secondes
```

- À la genèse : 120 tranches de 7 500 000 VINX, la tranche `i` (0 ≤ i < 120)
  échéant à `genesis_time + (i + 1) × period`.
- `period = 2 629 800 s` (un douzième d'année de 365,25 jours) : les 120
  tranches couvrent exactement 10 ans.
- Seule la **première** tranche peut être libérée, et uniquement si
  `matures_at ≤ timestamp du bloc`.
- Toute nouvelle tranche (reliquat ou retour) est ajoutée **en fin de liste**,
  avec `matures_at = dernière_échéance + period`.

### 4.3 Invariant

À la fin de chaque bloc :

```
Σ accounts.balance + Σ escrow.tranches.amount = 10¹⁸ unités  (1 000 000 000 VINX)
```

Un bloc qui viole l'invariant est invalide.

## 5. Transactions

### 5.1 Enveloppe

```
Transaction {
    chain_id: String,
    sender:   Address,
    nonce:    u64,          // doit valoir account.nonce + 1
    fee:      u64,          // ≥ params.min_fee
    body:     TxBody,
}
SignedTransaction {
    tx:         Transaction,
    public_key: Option<PublicKey>,   // obligatoire si la clé n'est pas encore en état
    signature:  Signature,
}
```

- Message signé : `"vinx/tx" ‖ borsh(tx)`.
- Identifiant : `tx_id = H("vinx/txid" ‖ borsh(signed_tx))`.
- Si `public_key` est fourni pour un compte sans clé, son adresse doit être égale
  à `sender`. Si le compte a déjà une clé, `public_key` doit être absent.
- Les frais sont débités du `sender` et **crédités à la trésorerie**.

### 5.2 Corps des transactions utilisateur

| Code | Transaction | Champs | Effet |
|---|---|---|---|
| `0x01` | `Transfer` | `to: Address`, `amount: u64`, `memo: Option<Vec<u8>>` | Transfert ; `amount > 0` ; mémo ≤ `max_memo_bytes` ; refusé sans mémo si `to.memo_required` |
| `0x02` | `SetMemoRequired` | `required: bool` | Active ou désactive le mémo obligatoire du compte |
| `0x03` | `RotateKey` | `new_key: PublicKey` | Remplace la clé du compte ; l'adresse ne change pas |

`RotateKey` est aussi la voie de migration post-quantique : quand `0x02` sera
actif, chaque compte pourra passer à une clé ML-DSA sans changer d'adresse.

### 5.3 Transactions d'administration

```
AdminTransaction {
    chain_id:  String,
    nonce:     u64,                  // compteur propre à l'admin
    action:    AdminAction,
}
SignedAdminTransaction {
    tx:        AdminTransaction,
    approvals: Vec<(signer_index: u8, Signature)>,  // indices distincts, triés
}
```

- Message signé : `"vinx/admin" ‖ borsh(tx)`.
- Valide si au moins **2 signataires distincts** sur 3 l'ont signée.
- Pas de frais.

| Action | Champs | Règles |
|---|---|---|
| `EscrowRelease` | `amount: u64` | Première tranche échue ; `amount ≤ tranche.amount` ; `amount` crédité à la trésorerie ; reliquat ajouté en fin d'escrow |
| `EscrowReturn` | `amount: u64` | Débite la trésorerie ; crée une tranche en fin d'escrow |
| `TreasuryTransfer` | `to: Address`, `amount: u64`, `memo: Option<Vec<u8>>` | Mêmes règles qu'un `Transfer`, depuis la trésorerie |
| `AddValidator` | `key: PublicKey` | Effectif à `H + 2` (§7.4) |
| `RemoveValidator` | `key: PublicKey` | Effectif à `H + 2` ; l'ensemble ne peut pas devenir vide |
| `SetParam` | `param: ParamId`, `value: u64` | Uniquement les paramètres modifiables de §11 |
| `ReplaceSigner` | `index: u8`, `new_key: PublicKey` | Remplace un signataire admin |
| `ScheduleUpgrade` | `version: u32`, `activation_height: u64` | Voir §9 |

Aucune action ne peut créer d'unités, modifier une tranche existante ni débiter un
autre compte que la trésorerie.

## 6. Blocs

```
BlockHeader {
    protocol_version: u32,
    chain_id:         String,
    height:           u64,
    timestamp:        u64,        // millisecondes Unix
    prev_hash:        Hash,
    tx_root:          Hash,       // Merkle des tx_id, dans l'ordre du bloc
    state_root:       Hash,       // état après application du bloc
    validators_hash:  Hash,       // ensemble qui valide le bloc suivant
    proposer:         u16,        // indice dans l'ensemble actif
}
Block { header: BlockHeader, admin_txs: Vec<SignedAdminTransaction>, txs: Vec<SignedTransaction> }
block_hash = H("vinx/block" ‖ borsh(header))
```

- Ordre d'application : transactions d'administration, puis transactions utilisateur.
- `timestamp` strictement supérieur à celui du parent et au plus
  `max_clock_drift` en avance sur l'horloge locale.
- Taille et nombre de transactions bornés par `max_block_bytes` et `max_block_txs`.

### 6.1 Certificat de commit

```
CommitCertificate {
    height:     u64,
    round:      u32,
    block_hash: Hash,
    signatures: Vec<(validator_index: u16, Signature)>,  // indices distincts, triés
}
```

Un bloc est **finalisé** quand il est accompagné d'un certificat portant les
precommits d'un quorum (§7.2) de l'ensemble actif à cette hauteur. Un bloc
finalisé n'est jamais annulé.

## 7. Consensus

### 7.1 Validateurs

- L'ensemble actif est une liste ordonnée de clés publiques. **Une voix par
  validateur.**
- Un validateur signe avec sa clé dédiée, distincte des clés admin.

### 7.2 Quorum

Pour `n` validateurs : `quorum = ⌊2n/3⌋ + 1`. La chaîne tolère `⌊(n−1)/3⌋`
validateurs défaillants ou malveillants.

### 7.3 Déroulement d'une hauteur

Algorithme Tendermint :

1. **Proposition** : le proposeur du round `r` est l'indice `(height + r) mod n`.
   Il diffuse un bloc (ou re-propose le bloc sur lequel il est verrouillé).
2. **Prevote** : chaque validateur vote pour le bloc s'il est valide et compatible
   avec son verrou, sinon `nil`.
3. **Precommit** : sur un quorum de prevotes pour un bloc, le validateur se
   **verrouille** sur ce bloc et émet un precommit.
4. **Commit** : sur un quorum de precommits, le bloc est finalisé.
5. Sans quorum avant expiration des délais, on passe au round `r + 1`.

- Message de vote signé : `"vinx/vote" ‖ borsh(chain_id, type, height, round, block_hash | nil)`.
- Un validateur ne signe **jamais** deux votes différents pour le même
  `(height, round, type)`. Ce garde est persisté sur disque **avant** l'envoi.
- Délais par round (§11) : `timeout_propose + r × timeout_delta`, de même pour
  les votes.

### 7.4 Changements de l'ensemble

Un `AddValidator` ou `RemoveValidator` inclus dans le bloc `H` prend effet à la
hauteur `H + 2`. `validators_hash` du bloc `H + 1` reflète déjà le nouvel ensemble.

## 8. Élagage

- Un nœud conserve **l'état complet** et les blocs des `block_retention` dernières
  secondes ; les blocs plus anciens sont supprimés.
- Mode **archive** optionnel : aucun bloc n'est supprimé.
- Un nouveau nœud se synchronise depuis un instantané d'état dont la racine est
  vérifiée contre un en-tête finalisé, puis rejoue les blocs suivants.

## 9. Mises à jour du protocole

- `ScheduleUpgrade` fixe `version` (= version actuelle + 1) et
  `activation_height ≥ hauteur courante + min_upgrade_delay`.
- Une nouvelle `ScheduleUpgrade` remplace la précédente tant que celle-ci n'est
  pas activée.
- À partir de `activation_height`, `protocol_version` doit valoir la nouvelle
  version ; les nœuds appliquent les règles correspondantes.
- Une mise à jour **ne peut pas** modifier la supply, l'escrow ou l'invariant
  (§4, ADR 0001 §7).

## 10. Étiquettes de domaine

| Usage | Étiquette |
|---|---|
| Adresse | `vinx/address` |
| Racine d'état | `vinx/state` |
| Transaction signée | `vinx/tx` |
| Identifiant de transaction | `vinx/txid` |
| Transaction admin signée | `vinx/admin` |
| En-tête de bloc | `vinx/block` |
| Vote de consensus | `vinx/vote` |
| Merkle (feuille / nœud / liste vide) | `vinx/merkle/leaf`, `vinx/merkle/node`, `vinx/merkle/empty` |

### 10.1 Arbre de Merkle (`tx_root`)

- Feuille : `H("vinx/merkle/leaf" ‖ valeur)` ; nœud : `H("vinx/merkle/node" ‖ gauche ‖ droite)`.
- Un nœud sans voisin remonte tel quel au niveau supérieur ; il n'est jamais
  dupliqué (sinon `[a, b, c]` et `[a, b, c, c]` auraient la même racine).
- Liste vide : `H("vinx/merkle/empty")`.

## 11. Paramètres

Les paramètres « modifiables » peuvent être changés par `SetParam` ; les autres
ne changent que par mise à jour du protocole.

L'intervalle de bloc et les délais démarrent volontairement haut. Ils ne seront
réduits par `SetParam` que si plus de 99 % des hauteurs sont finalisées au
premier round, de façon soutenue, en conditions réelles.

| Paramètre | Valeur proposée | Modifiable |
|---|---|---|
| `min_fee` | 0,001 VINX | oui |
| `min_reserve` | 0,1 VINX | oui |
| `max_memo_bytes` | 32 | oui |
| `max_block_txs` | 5 000 | oui |
| `max_block_bytes` | 4 Mo | oui |
| `timeout_propose` / `timeout_delta` | 3 s / 1 s | oui |
| `timeout_vote` / `timeout_delta` | 1 s / 0,5 s | oui |
| `min_block_interval` | 5 s | oui |
| `max_clock_drift` | 15 s | non |
| `block_retention` | 30 jours | non (local au nœud) |
| `min_upgrade_delay` | 17 280 blocs (≈ 24 h à 5 s) | non |
| `escrow.period` | 2 629 800 s | non |

## 12. Genèse

Fichier JSON contenant :

- `chain_id`, `genesis_time`, `protocol_version` ;
- `admin` : les 3 clés publiques des signataires ;
- `validators` : la liste initiale des clés de validateurs ;
- `treasury_balance` : 100 000 000 VINX ;
- `escrow` : 120 tranches de 7 500 000 VINX, `period` ;
- `params` : valeurs initiales de §11.

Un nœud refuse une genèse dont l'invariant (§4.3) n'est pas exactement respecté.
