# Protocole 8 : réserve du protocole et gouvernance votée

**Statut : brouillon, à valider.** Ce document traduit en règles précises
l'[ADR 0003](./adr/0003-reserve-et-gouvernance.md). Les points marqués
**[À décider]** attendent une décision (récapitulés au §10).

Le protocole 8 sera publié avec le protocole 7, dans la même grosse mise à
jour, et éprouvé sur une **pré-production** qui part d'une genèse au nouveau
modèle (§8).

## 1. Deux modèles de chaîne

Le modèle est fixé **à la genèse**, et ne change jamais ensuite :

| | Modèle « escrow » (testnets existants) | Modèle « réserve » (pré-production, réseau principal) |
|---|---|---|
| 900 M VINX | escrow en 120 tranches, libérées par l'admin | **réserve du protocole**, sans clé |
| Trésorerie | alimentée par l'escrow, dépensée par l'admin | **n'existe pas** (solde 0, aucun paiement admin) |
| Paramètres | `SetParam` par l'admin | **proposés** par l'admin, **votés** (§5) |
| Robinet | possible (`testnet`, `devnet`) | **interdit** |

Un testnet au modèle escrow peut passer au protocole 8 par mise à jour : il
garde l'escrow (engagement de l'ADR 0001). Les règles de ce document ne
s'appliquent qu'aux chaînes au modèle réserve.

## 2. La réserve

- **Solde initial** : 900 000 000 VINX, compté dans l'invariant de supply.
- **Sorties** : uniquement par une demande acceptée (§3). Aucune clé ne la
  contrôle.
- **Plafond mensuel** : 5 000 000 VINX par mois. Un mois dure 2 629 800 s,
  compté depuis la genèse. Le non-dépensé ne se reporte pas.
- **Plafond par demande** : 100 000 VINX.
- **Entrées** : cautions de demande non rendues (§3), et rien d'autre.
- Ces trois montants sont des **constantes du protocole**, pas des
  paramètres : seule une nouvelle version du protocole peut les changer.
  **[À décider : constantes ou paramètres votés]**. Recommandation :
  constantes, car c'est ce qui rend le calendrier crédible.

## 3. Demande de dépense

### 3.1 Dépôt

Transaction `SubmitSpend { beneficiary, amount, title, url }`, envoyable par
n'importe quel compte :

- `amount` entre 1 unité et 100 000 VINX ;
- `title` : 1 à 96 octets ; `url` : 0 à 200 octets (descriptif public) ;
- une **caution** de `proposal_deposit` (proposé : **1 000 VINX**) est
  bloquée, comptée dans l'invariant ;
- l'identifiant de la demande est l'identifiant de la transaction.

### 3.2 Vote des validateurs (7 jours)

- Transaction `VoteSpend { id, approve }`, depuis l'adresse de la clé d'un
  validateur **actif**. Un validateur peut changer son vote pendant le
  scrutin.
- La demande **passe** dès que les approbations dépassent les **2/3 des
  validateurs actifs**, calculés sur l'ensemble actif au moment du décompte.
  Un validateur dont l'adresse est bénéficiaire ne compte ni dans les votes,
  ni dans le total.
- Sinon elle est **rejetée** à la fin des 7 jours.
- Caution : **rendue** si la demande passe ce premier vote, **versée à la
  réserve** sinon (le « soutien minimal » de l'ADR est donc l'accord des
  validateurs). **[À décider]**

### 3.3 Vote des détenteurs (14 jours)

- Ouvert dès que les validateurs ont approuvé.
- Transaction `VoteHolders { id, choice, amount }`, `choice` = pour ou
  contre : bloque `amount` VINX du compte jusqu'à la fin du scrutin. Une
  adresse peut compléter son vote, dans le même sens seulement.
- **Quorum** : les VINX bloqués pour la demande atteignent au moins
  **10 % de la circulation** (`holder_quorum` = 1 000 dix-millièmes).
- **Circulation** = supply − réserve − émission restante − cautions des
  validateurs − cautions de demande − VINX bloqués dans les séquestres.
  **[À décider : quorum minimal fixe en plus du pourcentage]** (voir §9).
- **Acceptée** si le quorum est atteint et que les voix pour dépassent les
  voix contre, au poids des VINX.
- À la clôture, **tous les VINX bloqués sont rendus automatiquement** à leurs
  comptes, dans le bloc de clôture. Le coût du bloc est proportionnel au
  nombre de votants ; un plafond de **10 000 votants par demande** le borne.
  **[À décider : rendu automatique ou retrait par transaction]**

### 3.4 Versement

- Une demande acceptée entre dans une **file**, par ordre d'acceptation.
- Au début de chaque bloc, les demandes en tête de file sont versées tant que
  le plafond du mois le permet. Sinon, elles attendent le mois suivant.
- Un gros projet se découpe en plusieurs demandes, une par étape.

## 4. Ce que perd et garde l'admin (multisig 2 sur 3)

Sur une chaîne au modèle réserve, à partir du protocole 8 :

- **Refusées** : `EscrowRelease`, `EscrowReturn`, `TreasuryTransfer`,
  `SetParam`.
- **Nouvelle** : `ProposeParam { param, value }` (§5). L'admin est le seul à
  pouvoir proposer un changement de paramètre ; il ne peut plus en imposer
  aucun.
- **Gardées**, avec le délai de 48 h et l'annulation du protocole 7 :
  `ScheduleUpgrade`, `AddNamedValidator`, `RemoveValidator`,
  `SetValidatorIdentity`, `ReplaceSigner`, `CancelPending`.
- **Urgence** : un correctif de sécurité doit pouvoir être programmé plus
  vite que 17 280 blocs. Proposition : `ScheduleUpgrade` avec **les trois
  signatures** autorise un préavis réduit à **720 blocs** (environ 1 h), et
  son délai admin tombe à 1 h. **[À décider]**

## 5. Paramètres votés

`ProposeParam` ouvre un scrutin, comme une demande de dépense, sans caution :

| Type | Paramètres | Vote |
|---|---|---|
| **Économiques** | `min_fee`, `min_reserve`, `object_reserve`, `name_fee`, `asset_fee`, `validator_bond`, `min_availability`, `vote_share`, `proposal_deposit`, `holder_quorum` | validateurs (plus des 2/3, 7 jours) **puis** détenteurs (quorum, majorité, 14 jours) |
| **Techniques** | délais de consensus, `min_block_interval`, `max_block_txs`, `max_block_bytes`, `max_memo_bytes`, `absence_blocks`, `jail_min_blocks`, `bond_unlock_delay`, `diversity` | validateurs seuls (plus des 2/3, 7 jours) |

- Un paramètre accepté s'applique **deux blocs** après la clôture.
- `admin_delay` reste fixé par le protocole (48 h).
- Les durées de scrutin (7 et 14 jours) sont des constantes ; un réseau de
  pré-production peut les raccourcir à la genèse (§8).

## 6. Données de l'état

Nouvelles tables (étiquettes à partir de 15), dans `state_root` à partir du
protocole 8 :

- `Reserve` : solde, début du mois en cours, dépensé dans le mois ;
- `Proposal` (TAG 15) : type (dépense ou paramètre), auteur, contenu,
  caution, phase, échéances, votes des validateurs, totaux pour et contre ;
- `HolderLock` (TAG 16) : `(demande, adresse) → montant, sens`, compté dans
  l'invariant ;
- `SpendQueue` (TAG 17) : demandes acceptées en attente de versement.

Invariant : `soldes + réserve + cautions + VINX bloqués aux votes + … = 10¹⁸
unités`.

## 7. Transparence (API, explorateur, cockpit)

- `/reserve` : solde, plafond du mois, dépensé, prochain mois.
- `/proposals` et `/proposals/{id}` : chaque demande, ses votes, son état,
  son versement.
- Explorateur : onglet **Réserve** (solde, demandes, votes, versements).
- Cockpit : proposer un paramètre ; voir les scrutins en cours.
- Portefeuille : déposer une demande, voter comme détenteur ; un validateur
  vote depuis la ligne de commande (`vinx-wallet validator vote`).

## 8. Pré-production

Genèse au protocole 8, modèle réserve :

```
vinx-node genesis --chain-id vinx-preprod-1 --model reserve \
  --admin … --admin … --admin … \
  --validator CLE-CINNAMON --identity "Cinnamon,VinX Labs,FR" \
  --required-bond 1000 \
  --vote-periods 3600,7200 \
  --out genesis.json
```

- Pas de robinet, trésorerie à 0, **aucune part réservée**.
- `--vote-periods` (validateurs, détenteurs, en secondes) : réservé aux
  chaînes `preprod`, `testnet` ou `devnet`, pour éprouver les scrutins en
  quelques heures au lieu de 3 semaines. **[À décider]**
- Déroulé :
  1. Cinnamon seul produit les blocs et reçoit toute l'émission (environ
     19 000 VINX par jour) ;
  2. il dépose sa caution de 1 000 VINX ;
  3. il finance les adresses d'Amber, Obsidian et Pearl, qui déposent leur
     caution ;
  4. l'admin propose les trois ajouts en même temps ; ils entrent ensemble
     après 48 h ;
  5. on éprouve une première demande de dépense de la réserve, avec ses deux
     votes.

## 9. Risques, sans complaisance

- **Concentration au démarrage** : sans premine, presque toute la
  circulation appartient d'abord aux validateurs, donc à VinX Labs tant que
  les validateurs sont les siens. Le double vote est alors formel : c'est le
  fondateur qui décide, comme aujourd'hui, mais en public. Ça ne disparaît
  qu'avec des validateurs externes et une distribution plus large. D'où la
  question d'un quorum minimal fixe et d'une distribution initiale (ADR 0003,
  « À trancher »).
- **Faible participation** : 10 % de la circulation peut être difficile à
  atteindre plus tard, et facile au début.
- **Vote au poids** : les gros détenteurs pèsent plus ; le vote des
  validateurs sert de contrepoids.
- **Taille du module** : c'est le plus gros morceau depuis les jetons. Il
  devra être audité avant le réseau principal.
- **Juridique** : statut de la réserve et du vote, à faire valider (MiCA)
  avant la genèse du réseau principal.

## 10. Décisions à prendre

| Question | Proposition |
|---|---|
| Plafonds de la réserve (5 M par mois, 100 000 par demande) : constantes ou paramètres | Constantes |
| `proposal_deposit` | 1 000 VINX |
| Caution rendue si | Les validateurs approuvent |
| Quorum des détenteurs | 10 % de la circulation, plus un minimum fixe à décider |
| VINX bloqués au vote | Rendus automatiquement à la clôture, au plus 10 000 votants |
| Urgence | 3 signatures sur 3 : préavis de 720 blocs, délai admin d'1 h |
| Durées de scrutin raccourcies en pré-production | Oui, à la genèse, réservé aux chaînes non principales |
| Validateur bénéficiaire d'une demande | Ne vote pas et ne compte pas dans le total |

## 11. Mise en œuvre prévue

1. Validation de ce document.
2. Genèse au modèle réserve (`--model reserve`), réserve dans l'état et
   l'invariant.
3. Demandes, deux scrutins, file de versement ; paramètres votés ;
   restrictions de l'admin.
4. API, explorateur, cockpit, portefeuille ; fuzzing des nouvelles
   transactions.
5. Tests unitaires et multi-nœuds ; répétition complète sur devnet du déroulé
   du §8.
6. PR unique protocoles 7 et 8, release, pré-production.
