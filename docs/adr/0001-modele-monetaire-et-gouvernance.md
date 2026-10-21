# ADR 0001 — Modèle monétaire, consensus et gouvernance

- **Statut :** proposé
- **Date :** 2026-10-21

## Contexte

VinX Ledger repart d'un dépôt neuf. L'ancien projet (`VinX-Ledger-Legacy`) visait
un PoS permissionless avec émission décroissante ; il a été abandonné au profit
d'un registre de paiement à validateurs autorisés, dont le mainteneur garde la
gouvernance, mais dans des limites que le protocole lui impose.

Principe directeur : **le mainteneur décide, mais ne peut pas tricher en silence.**
Toute la supply existe à la genèse, rien ne peut être créé ensuite, et chaque
action de gouvernance est inscrite on-chain.

## Décisions

### 1. Consensus

- BFT de type Tendermint (proposition → prevote → precommit, verrou).
- **Proof-of-Authority** : l'ensemble des validateurs est une liste autorisée,
  gérée par la multisig admin (§6). Pas de staking, de récompenses, de slashing
  ni de jailing.
- Finalité au quorum de plus de 2/3 des validateurs, en un bloc.

### 2. Supply et unités

- **1 000 000 000 VINX**, entièrement créés à la genèse.
- **Aucune opération de mint n'existe** dans le protocole.
- **9 décimales** : 1 VINX = 10⁹ unités. La supply totale (10¹⁸ unités) tient
  dans un entier non signé 64 bits ; les montants sont des `u64`.

### 3. Répartition à la genèse

| Compte | Montant | Rôle |
|---|---|---|
| Escrow | 900 000 000 VINX (90 %) | Bloqué, libéré selon §4 |
| Trésorerie | 100 000 000 VINX (10 %) | Disponible, contrôlée par la multisig |

Aucune allocation fondateur séparée.

### 4. Émission : libérations plafonnées depuis l'escrow

- Le calendrier, fixé à la genèse, découpe l'escrow en **120 tranches mensuelles**
  de **7 500 000 VINX** (10 ans).
- Chaque tranche est un **plafond** : pour une tranche échue, la multisig libère
  vers la trésorerie un montant `r` avec `0 ≤ r ≤ plafond`.
- Le reste (`plafond − r`) **retourne en escrow** sous forme d'une nouvelle tranche
  ajoutée **en fin de calendrier**.
- La trésorerie peut également renvoyer des VINX en escrow ; ils forment eux aussi
  une nouvelle tranche en fin de calendrier.
- **Le calendrier ne peut que s'allonger** : aucune action ne peut avancer une
  tranche ni relever un plafond. Le mainteneur peut ralentir l'émission, jamais
  l'accélérer.

### 5. Frais

- Frais **fixes et faibles** par transaction, dont le rôle est l'anti-spam.
- **100 % des frais vont à la trésorerie. Aucun burn.**

### 6. Gouvernance

- Une **multisig admin 2-sur-3** peut :
  - ajouter ou retirer des validateurs ;
  - dépenser depuis la trésorerie ;
  - libérer une tranche d'escrow (dans la limite de §4) ou renvoyer des VINX en escrow ;
  - modifier les paramètres non monétaires (montant des frais, réserve minimale,
    limites de bloc, etc.).
- Elle **ne peut pas** créer de VINX, relever un plafond, avancer une tranche
  ni déplacer des fonds d'un compte qui n'est pas la trésorerie.
- Toutes les actions admin sont des transactions on-chain, signées et auditables.

### 7. Réserve minimale

Chaque compte doit conserver un petit solde minimum (valeur fixée par la
spécification), pour limiter la création de comptes vides.

### 8. Invariant

À chaque bloc :

```
somme(soldes de tous les comptes) + somme(tranches en escrow) = 1 000 000 000 VINX
```

Tout bloc qui viole cet invariant est invalide.

### 9. Hors protocole

La politique de distribution (faucet, programmes communautaires, publication de
la répartition) relève d'un document de politique monétaire séparé. Elle peut
évoluer sans modifier le protocole.

## Conséquences

- Le protocole est nettement plus simple que l'ancien VinX : plus de staking, de
  VRF, de récompenses par époque ni d'émission continue.
- La confiance repose sur des règles vérifiables (supply fixe, plafond d'émission,
  journal on-chain) plutôt que sur la décentralisation.
- La sécurité dépend fortement de la gestion des clés de la multisig et des
  validateurs.
- Les valeurs chiffrées (frais, réserve minimale, temps de bloc) ne sont fixées
  que dans la spécification et la configuration de genèse, jamais dupliquées
  dans la documentation.
