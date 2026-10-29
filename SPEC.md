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
  `balance ≥ min_reserve` (§11) **ou** `balance = 0`. Un transfert qui crée un
  compte doit donc apporter au moins `min_reserve`. La trésorerie n'est pas
  soumise à la réserve.
- Un compte vidé n'est **pas supprimé** : il garde son nonce et sa clé, pour
  qu'aucune de ses anciennes transactions ne puisse être rejouée. Il peut de
  nouveau recevoir des fonds, quel que soit le montant. Ainsi, relever
  `min_reserve` ne bloque jamais les fonds d'un compte existant.
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
    registry:   Registry,          // protocole 2 : noms et séquestres (§5.4, §5.5)
    emission:   Emission,          // protocole 3 : émission des validateurs (§4.4)
}
Registry {
    now:      u64,                         // horodatage du bloc, en secondes
    name_fee: u64,                         // §11
    names:    Map<String, NameRecord>,     // triée par nom
    deals:    Map<Hash, Deal>,             // triée par identifiant
}
```

`state_root = H("vinx/state" ‖ racine_SMT(accounts) ‖ H(G))`, avec
`G = borsh(escrow, admin, validators, params, upgrade, protocol_version)`,
suivi de `borsh(registry)` à partir du protocole 2, puis de `borsh(emission)` à
partir du protocole 3.

**Arbre des comptes (SMT).** Arbre binaire compact indexé par les 160 bits de
l'adresse, du bit de poids fort au bit de poids faible :

- sous-arbre vide : 32 octets nuls ;
- sous-arbre contenant un seul compte : `H("vinx/smt/leaf" ‖ adresse ‖ borsh(compte))` ;
- sinon : `H("vinx/smt/node" ‖ gauche ‖ droite)`, gauche = bit suivant à 0.

La racine ne dépend que de l'ensemble des comptes, pas de l'ordre d'insertion.

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
Σ accounts.balance + Σ escrow.tranches.amount + Σ deals.amount
  + emission.pot + (emission.budget − emission.emitted) = 10¹⁸ unités  (1 000 000 000 VINX)
```

Les séquestres (§5.5) ne font que déplacer des unités existantes : la supply ne change pas.

Un bloc qui viole l'invariant est invalide.

### 4.4 Émission des validateurs (protocole 3, ADR 0002)

```
Emission {
    start:        u64,        // genesis_time, en secondes
    budget:       u64,        // 100 000 000 VINX ; 0 avant le protocole 3
    emitted:      u64,        // déjà versé
    pot:          u64,        // frais et reliquats en attente de répartition
    epoch_blocks: u64,        // 17 280 par défaut
    proposed:     Vec<(PublicKey, u64)>,   // blocs proposés depuis le début de l'époque
}
```

- `cible(t) = ⌊budget × (10¹⁸ − f(t − start)) / 10¹⁸⌋`, où
  `f(x) = ⌊e^(−ln2 × (x mod H) / H) × 10¹⁸⌋ >> ⌊x / H⌋` et `H = 315 576 000 s`
  (10 ans de 365,25 jours). `e^(−y)` se calcule par la série de Taylor en
  entiers, à l'échelle 10¹⁸ (`ln2 × 10¹⁸ = 693 147 180 559 945 309`), jusqu'au
  premier terme nul (au plus 40 termes).
- Chaque bloc incrémente le compteur de son proposeur (`header.proposer`, indice
  dans l'ensemble de la hauteur). Un indice hors de l'ensemble invalide le bloc.
- Au bloc `h` tel que `h mod epoch_blocks = 0`, avec `t` l'horodatage du bloc
  en secondes :
  1. `dû = min(cible(t) − emitted, budget − emitted)` ; `emitted += dû` ; `pot += dû` ;
  2. chaque validateur ayant proposé `n` des `N` blocs de l'époque reçoit
     `⌊pot × n / N⌋`, crédité à l'adresse de sa clé ;
  3. `pot` garde le reliquat ; les compteurs repartent de zéro.
  Si aucun bloc n'a été compté, rien n'est versé et `pot` est conservé.

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
- Les frais sont débités du `sender` et **crédités à la trésorerie**, jusqu'au
  protocole 2. À partir du protocole 3, ils s'ajoutent à `emission.pot` et
  vont aux validateurs (§4.4).

### 5.2 Corps des transactions utilisateur

| Code | Transaction | Champs | Effet |
|---|---|---|---|
| `0x01` | `Transfer` | `to: Address`, `amount: u64`, `memo: Option<Vec<u8>>` | Transfert ; `amount > 0` ; `to ≠ sender` ; mémo, s'il est présent, de 1 à `max_memo_bytes` octets ; refusé sans mémo si `to.memo_required` |
| `0x02` | `SetMemoRequired` | `required: bool` | Active ou désactive le mémo obligatoire du compte |
| `0x03` | `RotateKey` | `new_key: PublicKey` | Remplace la clé du compte ; l'adresse ne change pas |

À partir du protocole 2 (avant, ces codes sont refusés) :

| Code | Transaction | Champs | Effet |
|---|---|---|---|
| `0x04` | `RegisterName` | `name: String` | Enregistre ou renouvelle un nom (§5.4) |
| `0x05` | `TransferName` | `name: String`, `to: Address` | Cède un nom non expiré à `to` |
| `0x06` | `OpenDeal` | `seller: Address`, `arbiter: Option<Address>`, `amount: u64`, `deadline: u64`, `memo: Option<Vec<u8>>` | Ouvre un séquestre (§5.5) |
| `0x07` | `ReleaseDeal` | `id: Hash` | Verse le séquestre au vendeur |
| `0x08` | `RefundDeal` | `id: Hash` | Rend le séquestre à l'acheteur |

À partir du protocole 4 :

| Code | Transaction | Champs | Effet |
|---|---|---|---|
| `0x09` | `BatchTransfer` | `payments: Vec<Payment { to, amount, memo }>` | Plusieurs paiements (§5.6) |
| `0x0A` | `Attest` | `digest: Hash` | Horodate une empreinte (§5.7) |

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
- Identifiant : `admin_tx_id = H("vinx/admin/txid" ‖ borsh(signed_admin_tx))`.
- Valide si au moins **2 signataires distincts** sur 3 l'ont signée.
- Pas de frais.

| Action | Champs | Règles |
|---|---|---|
| `EscrowRelease` | `amount: u64` | Première tranche échue ; `amount ≤ tranche.amount` ; `amount` crédité à la trésorerie ; reliquat ajouté en fin d'escrow |
| `EscrowReturn` | `amount: u64` | Débite la trésorerie ; crée une tranche en fin d'escrow |
| `TreasuryTransfer` | `to: Address`, `amount: u64`, `memo: Option<Vec<u8>>` | Mêmes règles qu'un `Transfer`, depuis la trésorerie |
| `AddValidator` | `key: PublicKey` | Effectif à `H + 2` (§7.4) |
| `RemoveValidator` | `key: PublicKey` | Effectif à `H + 2` ; l'ensemble ne peut pas devenir vide |
| `SetParam` | `param: ParamId`, `value: u64` | Uniquement les paramètres de §11 ; `value > 0` sauf pour `min_fee` et `min_reserve` |
| `ReplaceSigner` | `index: u8`, `new_key: PublicKey` | Remplace un signataire admin ; `new_key` ne doit pas déjà être signataire |
| `ScheduleUpgrade` | `version: u32`, `activation_height: u64` | Voir §9 |

Les actions sont encodées avec les codes `0x01` à `0x08`, dans l'ordre du tableau.

Aucune action ne peut créer d'unités, modifier une tranche existante ni débiter un
autre compte que la trésorerie.

### 5.4 Noms `.vinx` (protocole 2)

```
NameRecord { owner: Address, expires_at: u64 }   // horodatage Unix, secondes
```

- Un nom compte 3 à 32 caractères parmi `a-z`, `0-9` et `-`, sans tiret au début
  ni à la fin. On l'écrit `nom.vinx` ; la chaîne ne stocke que `nom`.
- `RegisterName` débite `name_fee` (§11) de l'émetteur, au profit de la
  trésorerie, en plus des frais de la transaction.
  - Nom libre : `owner = sender`, `expires_at = now + 31 557 600` (un an de 365,25 jours).
  - Nom déjà à l'émetteur : renouvellement, `expires_at = max(expires_at, now) + 1 an`.
  - Nom d'un autre : refusé tant que `now < expires_at + 30 jours` (délai de
    grâce réservé à l'ancien titulaire).
- `TransferName` : réservé au titulaire, avant `expires_at` ; l'échéance ne change pas.
- Un nom résout vers `owner` tant que `now < expires_at`. Les paiements restent
  adressés à une adresse : le portefeuille résout le nom avant de signer.

### 5.5 Séquestres (protocole 2)

```
Deal { buyer, seller: Address, arbiter: Option<Address>, amount, deadline: u64, memo: Option<Vec<u8>> }
```

- Identifiant : `H("vinx/deal" ‖ borsh(sender, nonce))` de la transaction `OpenDeal`.
- `OpenDeal` débite `amount` de l'acheteur (`sender`) et le bloque. Règles :
  - mêmes règles qu'un `Transfer` vers `seller` (montant, mémo, réserve d'un nouveau compte) ;
  - l'arbitre, s'il y en a un, n'est ni l'acheteur ni le vendeur ;
  - `now < deadline ≤ now + 1 an`.
- `ReleaseDeal` : par l'acheteur ou l'arbitre ; `amount` est crédité au vendeur.
- `RefundDeal` : par le vendeur ou l'arbitre à tout moment, par l'acheteur si
  `now ≥ deadline` ; `amount` est recrédité à l'acheteur.
- Le séquestre est alors supprimé. Rien ne se dénoue sans transaction.

### 5.6 Paiements groupés (protocole 4)

- De 1 à 100 paiements. Chacun suit les règles d'un `Transfer` (§5.2) : montant
  non nul, destinataire différent de l'émetteur, mémo valide, mémo exigé par
  le destinataire, réserve d'un nouveau compte. L'existence du destinataire est
  jugée avant la transaction.
- Frais minimaux : `min_fee × ⌈n / 10⌉` pour `n` paiements.
- Un seul paiement invalide invalide toute la transaction.

### 5.7 Horodatage (protocole 4)

- `Attest` inscrit une empreinte de 32 octets (par exemple SHA-256 d'un
  fichier) dans le bloc. Il ne change rien d'autre à l'état que les frais.
- Les nœuds indexent les empreintes des blocs conservés
  (`GET /attestations/{empreinte}`). Pour vérifier un horodatage plus ancien que
  l'élagage, il faut un nœud archive.

## 6. Blocs

```
BlockHeader {
    protocol_version: u32,
    chain_id:         String,
    height:           u64,
    timestamp:        u64,        // millisecondes Unix
    prev_hash:        Hash,
    tx_root:          Hash,       // Merkle des admin_tx_id puis des tx_id, dans l'ordre du bloc
    state_root:       Hash,       // état après application du bloc
    validators_hash:  Hash,       // ensemble qui valide le bloc suivant
    proposer:         u16,        // indice dans l'ensemble actif
}
Block { header: BlockHeader, admin_txs: Vec<SignedAdminTransaction>, txs: Vec<SignedTransaction> }
block_hash = H("vinx/block" ‖ borsh(header))
```

- Ordre d'application : transactions d'administration, puis transactions utilisateur.
- Un bloc est invalide dès qu'une de ses transactions l'est : le proposeur ne
  retient que des transactions applicables.
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
- Message de proposition signé : `"vinx/proposal" ‖ borsh(chain_id, height, round, pol_round, block_hash)`,
  où `pol_round` est le round où le bloc re-proposé a obtenu un quorum de prevotes.
  Une proposition n'est acceptée que du proposeur du round, et seule la première
  l'est pour un round donné.
- **Comptage des votes.** Un validateur qui vote pour plusieurs cibles à la même
  étape (équivoque) est compté pour chacune. C'est sans risque pour la sûreté,
  deux quorums se recoupant toujours sur au moins un validateur honnête, et
  nécessaire à la vivacité. Pour borner la mémoire, un nœud garde au plus deux
  cibles par validateur et par étape, plus la cible de la proposition du round.
- Un validateur ne signe **jamais** deux votes différents pour le même
  `(height, round, type)`. Ce garde est persisté sur disque **avant** l'envoi.
- **Relais.** Un nœud relaie chaque proposition et chaque vote valides la première
  fois qu'il les reçoit.
- **Rattrapage.** Un nœud qui reçoit des messages d'une hauteur supérieure récupère
  les blocs manquants avec leur certificat de commit (§6.1) et les applique.
- Délais par round (§11) : `timeout_propose + r × timeout_propose_delta`, et
  `timeout_vote + r × timeout_vote_delta` pour les votes.

### 7.4 Changements de l'ensemble

Un `AddValidator` ou `RemoveValidator` inclus dans le bloc `H` prend effet à la
hauteur `H + 2`. `validators_hash` du bloc `H + 1` reflète déjà le nouvel ensemble.

`validators_hash = H("vinx/validators" ‖ borsh(liste ordonnée des clés))`.

### 7.5 Preuves d'équivoque

- Deux votes signés par le même validateur pour la même hauteur, le même round
  et le même type, avec des cibles différentes, ou deux propositions signées par
  le proposeur d'un round pour des blocs différents, forment une **preuve
  d'équivoque**. Elle se vérifie avec la seule clé publique du validateur.
- Un nœud qui en relève une l'enregistre, la journalise en alerte, la diffuse
  (`NetMessage::Evidence`) et l'expose (`GET /evidence`). Un nœud qui en reçoit
  une la vérifie avant de l'enregistrer.
- Aucune sanction automatique : en PoA, la multisig décide de retirer le
  validateur (`RemoveValidator`).

## 8. Élagage et synchronisation

- Un nœud conserve **l'état complet** et les blocs des `block_retention`
  dernières heures (24 h par défaut) ; les plus anciens sont supprimés. Le bloc
  le plus récent est toujours conservé.
- Mode **archive** : aucun bloc n'est supprimé. Un nœud archive ne se
  synchronise jamais par instantané : il rejoue la chaîne depuis la genèse
  auprès d'un pair qui possède encore les blocs.
- Un nœud qui a manqué peu de blocs les récupère un par un (§8.1, rattrapage).
  Si ses pairs les ont élagués, il se synchronise par l'état (§8.3).

### 8.1 Réseau

- Transport TCP chiffré et authentifié (Noise), multiplexé (Yamux), via libp2p.
  Chaque nœud a une identité réseau Ed25519, distincte de ses clés de consensus.
- **Liste autorisée** : un nœud n'accepte que les pairs qu'il est configuré pour
  joindre ou explicitement autorisés. Les autres connexions sont refusées dès
  la poignée de main.
- **Retransmission** : un validateur rediffuse ses propres propositions et votes
  de la hauteur en cours toutes les secondes, jusqu'à la décision. Tendermint
  suppose que tout message finit par arriver ; un message diffusé avant que les
  pairs soient connectés serait sinon perdu, et la chaîne pourrait s'arrêter.
- Chaque message gossip est enveloppé avec un numéro de séquence, pour qu'une
  retransmission ne soit pas écartée comme doublon.
- **Relais** : propositions, votes et transactions circulent sur le sujet
  gossipsub `vinx/<chain_id>/1`, encodés en borsh (`NetMessage`). Un message est
  identifié par le hachage BLAKE3 de son contenu et relayé une seule fois.
- **Rattrapage** : protocole requête-réponse `/vinx/sync/1`. Une requête donne
  la première hauteur manquante ; la réponse contient au plus 50 blocs finalisés
  avec leurs certificats. Le nœud vérifie chaque certificat contre l'ensemble de
  validateurs de la hauteur, puis exécute le bloc, avant de l'adopter.
- **Quotas** (politique locale, hors consensus) : un pair qui relaie plus de
  1 000 messages par seconde, un message invalide comptant pour 100, est exclu
  60 secondes ; au plus 4 requêtes de rattrapage par seconde et par pair ; une
  réponse de rattrapage vise 8 Mio au plus. Un message n'est relayé qu'après
  avoir été décodé avec succès.

### 8.3 Synchronisation par l'état

- Chaque nœud prépare un **instantané** de son état toutes les 100 hauteurs,
  et plus souvent si l'élagage rendait l'instantané courant impossible à
  prolonger par des blocs conservés.
- Le protocole `/vinx/sync/2` sert : les blocs à partir d'une hauteur (avec la
  plus petite hauteur encore conservée), la description de l'instantané (bloc,
  certificat, état hors comptes, nombre de comptes) et ses comptes par morceaux
  de 10 000, dans l'ordre des adresses.
- Le nœud qui télécharge vérifie, **avant** d'adopter l'état :
  1. que le certificat de commit du bloc est valide pour les validateurs de la genèse ;
  2. que la racine de l'état reconstitué est exactement le `state_root` du bloc ;
  3. que l'ensemble de validateurs de l'état redonne le `validators_hash` du bloc ;
  4. l'invariant de supply.
- Si les validateurs ont changé depuis la genèse, le point 1 échoue : le nœud
  doit se synchroniser bloc par bloc auprès d'un nœud archive, ce qui vérifie
  chaque changement de validateurs.

### 8.2 Mempool (politique locale)

- Au plus 100 transactions en attente par compte, 20 000 au total.
- Mempool plein, une transaction n'entre qu'en payant des frais supérieurs à
  ceux de la transaction évinçable la moins chère, qui est retirée. Seule la
  dernière transaction en attente d'un compte est évinçable.
- Le proposeur remplit le bloc par frais décroissants, dans le respect de
  l'ordre des nonces de chaque compte.
- Frais conseillés (`GET /fees`) : le minimum tant que les transactions en
  attente tiennent dans un bloc ; sinon de quoi dépasser la dernière qui y
  entrerait, et, mempool plein, la moins chère en attente.

## 9. Mises à jour du protocole

- `ScheduleUpgrade` fixe `version` (= version actuelle + 1) et
  `activation_height ≥ hauteur courante + min_upgrade_delay`.
- Une nouvelle `ScheduleUpgrade` remplace la précédente tant que celle-ci n'est
  pas activée.
- À partir de `activation_height`, `protocol_version` doit valoir la nouvelle
  version ; les nœuds appliquent les règles correspondantes.
- Une mise à jour **ne peut pas** modifier la supply, l'escrow ou l'invariant
  (§4, ADR 0001 §7).
- Un nœud refuse les blocs d'une version qu'il ne connaît pas : il s'arrête au
  lieu de diverger. Tous les validateurs doivent donc être mis à jour avant
  `activation_height`.

| Version | Apporte |
|---|---|
| 1 | Paiements, mémo obligatoire, rotation de clé, gouvernance |
| 2 | Noms `.vinx` (§5.4), séquestres (§5.5), paramètre `name_fee` |
| 3 | Émission des validateurs et frais aux validateurs (§4.4, ADR 0002) |
| 4 | Paiements groupés (§5.6), horodatage natif (§5.7) ; suite en cours (feuille de route) |

## 10. Étiquettes de domaine

| Usage | Étiquette |
|---|---|
| Adresse | `vinx/address` |
| Racine d'état | `vinx/state` |
| Transaction signée | `vinx/tx` |
| Identifiant de transaction | `vinx/txid` |
| Transaction admin signée | `vinx/admin` |
| Identifiant de transaction admin | `vinx/admin/txid` |
| En-tête de bloc | `vinx/block` |
| Vote de consensus | `vinx/vote` |
| Proposition de bloc | `vinx/proposal` |
| Ensemble de validateurs | `vinx/validators` |
| Identifiant de séquestre | `vinx/deal` |
| Arbre des comptes (feuille / nœud) | `vinx/smt/leaf`, `vinx/smt/node` |
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

| Code `ParamId` | Paramètre | Valeur initiale | Unité |
|---|---|---|---|
| `0x01` | `min_fee` | 1 000 000 (0,001 VINX) | unités |
| `0x02` | `min_reserve` | 100 000 000 (0,1 VINX) | unités |
| `0x03` | `max_memo_bytes` | 32 | octets |
| `0x04` | `max_block_txs` | 5 000 | transactions |
| `0x05` | `max_block_bytes` | 4 194 304 (4 Mio) | octets |
| `0x06` | `timeout_propose` | 3 000 | ms |
| `0x07` | `timeout_propose_delta` | 1 000 | ms par round |
| `0x08` | `timeout_vote` | 1 000 | ms |
| `0x09` | `timeout_vote_delta` | 500 | ms par round |
| `0x0A` | `min_block_interval` | 5 000 | ms |
| `0x0B` | `name_fee` | 10 000 000 000 (10 VINX) | unités par an ; protocole 2 |

Paramètres non modifiables par `SetParam` :

| Paramètre | Valeur | Remarque |
|---|---|---|
| `max_clock_drift` | 15 000 ms | |
| `min_upgrade_delay` | 17 280 blocs | ≈ 24 h à 5 s par bloc |
| `escrow.period` | 2 629 800 s | |
| Budget d'émission | 100 000 000 VINX | fixé à la genèse (§4.4) |
| Demi-vie de l'émission | 315 576 000 s (10 ans) | |
| `epoch_blocks` | 17 280 blocs | fixé à la genèse |
| `block_retention` | 24 heures | réglage local du nœud, hors consensus ; illimité en mode archive |

## 12. Genèse

Fichier JSON contenant :

- `chain_id`, `genesis_time`, `protocol_version` ;
- `admin` : les 3 clés publiques des signataires ;
- `validators` : la liste initiale des clés de validateurs ;
- `treasury_balance` : 0 (réseau de test : le robinet, voir plus bas) ;
- `escrow` : 120 tranches de 7 500 000 VINX, `period` ;
- `params` : valeurs initiales de §11 ;
- `emission` : `budget` (100 000 000 VINX) et `epoch_blocks` (17 280).
  Absente d'une genèse antérieure au protocole 3 : budget nul.

`treasury_balance + Σ escrow + emission.budget` doit valoir exactement 10¹⁸
unités. Sur un réseau de test, la trésorerie peut recevoir un robinet, pris sur
la dernière tranche d'escrow (ADR 0002 §5).

Un nœud refuse une genèse dont l'invariant (§4.3) n'est pas exactement respecté.

Le bloc 1 a pour `prev_hash` la racine d'état de la genèse (§4.1), et son
horodatage doit être postérieur à `genesis_time`. Deux réseaux de genèses
différentes ne partagent donc aucun bloc.
