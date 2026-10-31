<p align="center">
  <img src="./assets/vinx-logo.png" alt="VinX" width="540" />
</p>

<h1 align="center">VinX Ledger</h1>

<p align="center"><b>Une blockchain de paiement simple, où les fonctions utiles sont déjà intégrées.</b></p>

> **Statut : réseau de test.** Le VINX n'a aucune valeur. Tout peut encore
> changer, et les soldes du réseau de test peuvent être remis à zéro.

## L'idée

La plupart des blockchains donnent une boîte à outils : pour faire quoi que ce
soit, il faut écrire et publier un programme (un « contrat intelligent »). C'est
puissant, mais c'est aussi là que naissent la plupart des failles, des arnaques
et des frais imprévisibles.

VinX Ledger fait l'inverse. Les usages courants sont **intégrés à la chaîne,
comme des fonctions prêtes à l'emploi**, aux règles connues de tous, testées
une fois pour toutes. On ne programme rien : on s'en sert.

## Ce qu'on peut faire

- **Payer et être payé** en VINX, en quelques secondes, avec des frais minimes
  et fixes.
- **Envoyer en une fois** jusqu'à 100 paiements, par exemple des salaires ou
  des remboursements.
- **Se faire payer facilement** : une demande de paiement par lien ou QR code,
  avec sa référence de facture.
- **Un nom à la place d'une adresse** : `boulangerie.vinx` au lieu d'une suite
  de 40 caractères.
- **Payer par séquestre** : l'argent est bloqué jusqu'à la livraison, avec un
  arbitre si besoin.
- **Les abonnements** : on autorise un commerçant à prélever jusqu'à un montant
  par mois, et on révoque quand on veut.
- **Les comptes partagés** : « 2 signatures sur 3 » pour une association ou une
  entreprise, ou un plafond journalier pour la clé du téléphone.
- **Les frais pris en charge** : un commerçant peut payer les frais de ses
  clients.
- **Horodater un document** : prouver qu'un fichier existait à une date, sans
  jamais le publier.
- **Créer son propre jeton** : points de fidélité, bons d'achat, monnaie d'une
  association, et à terme une monnaie stable. L'émetteur déclare à l'avance ce
  qu'il a le droit de faire, et personne ne reçoit un jeton qu'il n'a pas
  accepté.

Le tout depuis un **portefeuille** qui s'ouvre dans le navigateur, sans
compte à créer : tes clés restent sur ton ordinateur.

## Les principes

- **Une quantité fixe** : 1 milliard de VINX, pas un de plus.
- **Des validateurs identifiés** : le réseau est tenu par des opérateurs
  connus, rémunérés par une émission prévue d'avance (100 millions sur
  plusieurs décennies) et par les frais.
- **Une gouvernance à plusieurs** : toute décision sur le réseau demande la
  signature de 2 administrateurs sur 3, et toute évolution des règles est
  annoncée à l'avance.
- **La simplicité avant tout** : chaque nouvelle fonction doit répondre à un
  besoin réel. Une règle ajoutée ne se retire plus.

## Par où commencer

| Je veux… | Guide |
|---|---|
| Utiliser le portefeuille | [Portefeuille](./docs/PORTEFEUILLE.md) |
| Payer depuis un téléphone | [Portefeuille mobile](./docs/MOBILE.md) |
| Voir ce qui se passe sur le réseau | [L'explorateur](https://vinx-ledger.vinxlabs.fr/explorer) : blocs, transactions, comptes, jetons, noms |
| Créer un jeton | [Jetons](./docs/JETONS.md) |
| Comprendre où va le projet | [Feuille de route](./docs/FEUILLE-DE-ROUTE.md) |
| Faire tourner un validateur | [Déployer un validateur](./docs/DEPLOYER-UN-VALIDATEUR.md), [Exploitation](./docs/EXPLOITATION.md) |
| Ouvrir un nœud public | [Nœud public](./docs/NOEUD-PUBLIC.md) |
| Lancer le testnet 2 | [Testnet 2](./docs/TESTNET-2.md) |
| Développer, compiler, tester | [Développeurs](./docs/DEVELOPPEURS.md), [Spécification](./SPEC.md), [Décisions d'architecture](./docs/adr/) |
| Intégrer VinX dans une application | [API du nœud](https://vinx-ledger.vinxlabs.fr/docs) (aussi sur `/docs` de chaque nœud) |

Les versions prêtes à l'emploi (Windows, macOS, Linux) sont dans les
[Releases](../../releases).

Licence : [Apache-2.0](./LICENSE).
