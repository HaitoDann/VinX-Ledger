# Protocole 7 : validateurs nommés, caution, présence, délai admin

**Statut : projet, à valider avant tout code.** Une fois activées, ces règles
sont définitives (une règle de protocole ne se retire pas). Les valeurs en gras
sont paramétrables par la gouvernance (`SetParam`), sauf mention contraire.
Les choix encore ouverts sont listés au §9.

Contexte : [`VINX-CONSENSUS.md`](./VINX-CONSENSUS.md) et la
[charte des validateurs](./CHARTE-DES-VALIDATEURS.md).

## 1. Registre des validateurs

Chaque validateur actif ou passé a une fiche dans l'état (nouvelle table) :

```
ValidatorRecord {
    key:       PublicKey,     // clé de consensus
    name:      String,        // 3 à 24 caractères [A-Za-z0-9 -], unique, jamais réattribué
    operator:  String,        // 1 à 64 octets
    country:   [u8; 2],       // code ISO 3166-1 (« FR »)
    website:   String,        // 0 à 96 octets
    contact:   String,        // 0 à 96 octets
    status:    Active | Jailed { since } | Leaving { until } | Left | Banned,
    bond:      Amount,        // caution bloquée (§3)
    joined_at: u64,           // hauteur d'entrée
}
```

- `AddValidator` devient `AddValidator { key, name, operator, country, website,
  contact }` : la gouvernance fixe l'identité. Le nom ne doit jamais avoir été
  utilisé.
- Nouvelle transaction **signée par le validateur**, depuis l'adresse de sa clé :
  `UpdateValidatorInfo { website, contact }`. Le nom, l'opérateur et le pays ne
  changent que par la gouvernance (`SetValidatorIdentity`).
- À l'activation, les validateurs existants reçoivent une fiche à partir d'un
  fichier fourni avec la mise à jour (Cinnamon, Amber, Obsidian, Pearl, et
  l'opérateur déclaré).
- L'API (`/validators`) et l'explorateur affichent ces fiches. L'option
  `--validator-names` devient inutile.

## 2. Diversité

- Un opérateur est identifié par son champ `operator`, comparé sans tenir
  compte de la casse ni des espaces.
- Si le paramètre **`diversity`** vaut 1, un `AddValidator` est refusé quand,
  après l'ajout, un opérateur aurait plus de `⌊(n − 1)/3⌋` validateurs actifs
  (`n` : taille de l'ensemble après l'ajout).
- Valeur initiale : **0** (désactivé), pour le lancement où le projet opère
  seul ses validateurs.
- Les retraits et mises à l'écart ne sont jamais refusés pour cette raison. Ils
  sont signalés dans l'onglet Gouvernance du cockpit.

## 3. Caution

- Paramètre **`validator_bond`** : **10 000 VINX**.
- Nouvelle transaction `DepositBond { amount }`, envoyée depuis l'adresse de la
  clé du validateur. Elle bloque `amount` dans sa fiche.
- `AddValidator` est refusé si la caution déposée est inférieure à
  `validator_bond`.
- Si `validator_bond` augmente, les validateurs en place ne sont pas exclus :
  le nouveau montant s'applique aux entrées et aux retours de mise à l'écart.
- **Invariant de supply** : la somme des cautions s'ajoute à l'invariant
  (SPEC §4.3). Une caution ne crée ni ne détruit d'unités.

## 4. Sanction de la double signature

- Nouvelle transaction `ReportEquivocation { evidence }`, envoyable par
  n'importe quel compte, qui paie les frais. Les nœuds la créent d'eux-mêmes
  quand ils relèvent une preuve. La preuve est vérifiée avec la seule clé du
  validateur (SPEC §7.5) ; elle doit concerner une hauteur où il était actif.
- Effet, dans le bloc qui l'inclut :
  1. **100 %** de la caution est confisquée ;
  2. elle est répartie **à parts égales entre les autres validateurs
     actifs**, crédités à l'adresse de leur clé. Le reliquat de la division
     revient au pot d'émission ;
  3. le validateur passe à `Banned` : il est retiré de l'ensemble à `H + 2` et
     sa clé ne peut plus jamais être ajoutée ;
  4. une même faute n'est sanctionnée qu'une fois (identifiant de la preuve
     conservé).
- La sanction reste possible pendant le délai de restitution d'un validateur
  sorti (§6).

## 5. Présence et récompenses

**Mesure de la présence.** Chaque bloc `H + 1` inclut la **liste des
validateurs dont le precommit a été reçu pour `H`** : un champ `last_commit`,
avec les signatures, vérifiées comme un certificat. C'est un changement du
format des blocs, actif à partir de la hauteur d'activation. Le proposeur doit
y mettre tous les precommits reçus, et au moins un quorum.

**Récompenses** (remplace le partage de SPEC §4.4, étape 2). À chaque fin
d'époque (17 280 blocs), pour chaque validateur actif pendant l'époque :

- `présence = blocs signés / blocs de l'époque où il était actif` ;
- **éligible** si `présence ≥ min_availability`, soit **97 %** (paramètre en
  dix-millièmes : **9 700**) ;
- le pot est partagé entre les éligibles seulement :
  - **`vote_share`** du pot (**50 %**) au prorata des blocs signés ;
  - le reste au prorata des blocs proposés ;
- sans validateur éligible, le pot est conservé pour l'époque suivante ;
- les parts non versées aux non-éligibles reviennent aux éligibles.

## 6. Absence, mise à l'écart, départ

- **Mise à l'écart automatique** : un validateur absent de `last_commit` sur
  les **`absence_blocks`** derniers blocs (**1 000**) passe à `Jailed`. Il est
  retiré de l'ensemble à `H + 2`, et sa caution est conservée. La mise à
  l'écart est refusée si elle laisserait l'ensemble vide.
- **Retour** : transaction `Unjail` depuis l'adresse du validateur, au plus tôt
  **`jail_min_blocks`** (**1 000**) après la mise à l'écart, et avec une caution
  au moins égale à `validator_bond`. Retour dans l'ensemble à `H + 2`.
- Le **quorum n'est jamais réduit** : il se recalcule sur l'ensemble actif,
  sans le validateur mis à l'écart.
- **Départ volontaire** : transaction `LeaveValidators` depuis l'adresse du
  validateur, qui passe à `Leaving`. Il est retiré à `H + 2`. Sa caution est
  rendue par `WithdrawBond` après **`bond_unlock_delay`** (**14 jours**, en
  secondes : **1 209 600**). Un `RemoveValidator` de la gouvernance suit le
  même chemin.

## 7. Délai des actions admin

- Paramètre **`admin_delay`** : **48 h** (en secondes : **172 800**).
- Une action admin incluse dans un bloc est **mise en attente**, avec une
  échéance. Elle est exécutée par le premier bloc dont l'horodatage atteint
  l'échéance, dans l'ordre d'inclusion.
- Nouvelle action `CancelPending { id }`, immédiate, avec 2 signatures sur 3 :
  annule une action en attente.
- L'API (`/governance`) et l'explorateur listent les actions en attente et leur
  échéance.
- **Actions soumises au délai** (proposition, à valider au §9) : `AddValidator`,
  `RemoveValidator`, `SetParam`, `ReplaceSigner`, `SetValidatorIdentity`,
  `TreasuryTransfer`, `EscrowReturn`.
- **Actions immédiates** : `EscrowRelease` (mécanique, au profit de la
  trésorerie), `ScheduleUpgrade` (qui a déjà son propre préavis d'au moins
  24 h), `CancelPending`.

## 8. Nouveaux paramètres

| Code | Paramètre | Valeur initiale | Unité |
|---|---|---|---|
| `0x0E` | `validator_bond` | 10 000 VINX | unités |
| `0x0F` | `min_availability` | 9 700 | dix-millièmes |
| `0x10` | `vote_share` | 5 000 | dix-millièmes du pot |
| `0x11` | `absence_blocks` | 1 000 | blocs |
| `0x12` | `jail_min_blocks` | 1 000 | blocs |
| `0x13` | `bond_unlock_delay` | 1 209 600 | secondes |
| `0x14` | `admin_delay` | 172 800 | secondes |
| `0x15` | `diversity` | 0 | 0 ou 1 |

## 9. Choix encore ouverts

| Question | Proposition |
|---|---|
| Partage du pot entre signatures et propositions | 50 / 50 (`vote_share`) |
| `TreasuryTransfer` soumis au délai de 48 h ? | Oui, par transparence. Cela retarde aussi les paiements courants de la trésorerie, comme le robinet des testeurs |
| Caution des 4 validateurs existants à l'activation | Délai de grâce de 30 jours pour déposer, sans récompense tant qu'elle n'est pas déposée |
| Validateur mis à l'écart : récompense de l'époque ? | Aucune pour l'époque de la mise à l'écart |
| Signaler les preuves | Automatique par tout nœud ; récompense au signaleur ? Proposition : non (les frais restent à sa charge), pour éviter les incitations perverses |

## 10. Mise en œuvre prévue

1. Validation de ce document (règles et valeurs).
2. Code, tests unitaires et tests multi-nœuds : mise à l'écart, sanction,
   retour, délai admin, avec 4 puis 7 validateurs.
3. Fuzzing des nouvelles transactions (cible `transaction`).
4. Essai complet sur devnet, avec la mise à jour à chaud 6 → 7.
5. Release, puis mise à jour du testnet en suivant la procédure du protocole 6.
