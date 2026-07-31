# ADR 0034 — Disponibilité des données & vérification d'ancre (modules)

- **Statut :** Proposé
- **Catégorie :** Modules (par-dessus l'ADR 0001) · **Priorité :** 🟠 moyenne
- **Date :** Juillet 2026
- **Liens :** complète la primitive d'ancrage (ADR 0010) ; prérequis de l'adjudication de
  fraude (ADR 0023) ; s'appuie sur le standard light-client (ADR 0014).

## Contexte

L'ADR 0010 permet à un opérateur d'ancrer une **racine** (`anchor_head: Hash32`) de l'état
hors-chaîne de son module, bond à l'appui. Mais la L1 ne **stocke que la racine** — jamais les
données. Il manque **le maillon central de la thèse de l'ADR 0001** :

1. **Où sont les données** derrière la racine ? Une racine sans données disponibles est
   inutile : personne ne peut prouver ce qu'elle engage.
2. **Comment un tiers vérifie une ancre** ? Sans format de preuve d'inclusion standard, un
   utilisateur ne peut pas prouver « mon état X est inclus dans l'ancre H du module M ».

Sans ça, un opérateur peut ancrer une racine dont le contenu est **indisponible** ou
**invérifiable** — et l'ADR 0023 (slashing de fraude) n'a **rien sur quoi s'appuyer** : on ne
peut prouver une fraude que si les données sont disponibles et la vérification standardisée.
C'est le prérequis logique de tout le modèle « surcouches par ancrage bondé ».

## Décision proposée

Standardiser **deux choses minimales**, sans que la L1 n'exécute ni ne stocke la logique du
module (elle reste monnaie pure).

### 1. Engagement de disponibilité (DA commitment)

À chaque `Anchor`, l'opérateur engage — en plus de `anchor_head` — un **pointeur de
disponibilité** vérifiable : le **hash du blob de données** (ou de son en-tête d'érasure-coding)
et sa **localisation logique** (couche DA : gossip du module, DA externe type Celestia, ou
publication on-chain bornée pour les petits modules). La L1 ne stocke que **l'engagement**, pas
les données.

- Le format d'engagement est **canonique** (vecteur doré, ADR 0020) et versionné (plusieurs
  backends DA possibles derrière une même interface).
- **Règle de disponibilité** : une ancre dont les données sont prouvablement indisponibles est
  **contestable** → passerelle vers le slashing (ADR 0023). La *preuve d'indisponibilité* elle-
  même est le point dur (cf. Alternatives / littérature DA).

### 2. Format de preuve d'inclusion standard

Un **schéma de preuve** unique — racine Merkle (déjà la primitive de VinX pour le state_root) +
chemin d'inclusion — permettant à quiconque de prouver, contre `anchor_head`, que
`(clé, valeur)` appartient à l'état ancré. Réutilise la brique Merkle existante (cohérence,
un seul format à auditer) et **s'aligne sur le format light-client de l'ADR 0014** (mêmes
preuves d'inclusion pour l'état L1 et pour l'état de module).

### Ce que la L1 fait / ne fait pas

- **Fait** : stocker `anchor_head` + l'engagement DA (petit, borné) ; exposer les racines
  historiques nécessaires aux preuves ; servir de point de contestation bondé.
- **Ne fait pas** : stocker les données du module, exécuter sa transition, ni garantir la
  disponibilité par elle-même — elle garantit qu'un opérateur qui ment est **slashable** (via
  0023) une fois la fraude/indisponibilité prouvée.

## Conséquences

**Positif**
- Rend les ancres **réellement vérifiables** → débloque l'utilité des modules (l'ADR 0010 seul
  n'ancre qu'un nombre opaque).
- **Prérequis** propre pour l'ADR 0023 (on ne slashe une fraude que si DA + preuve existent).
- Réutilise Merkle (un seul format de preuve pour L1 + modules, aligné 0014).

**Coûts / pièges**
- La **preuve d'indisponibilité** est un problème dur et bien connu (impossible en général sans
  hypothèses ; d'où DAS/érasure-coding). À cadrer honnêtement — la tranche 1 peut se limiter à
  **l'engagement + la preuve d'inclusion** et laisser la *contestation d'indisponibilité* à une
  tranche 2 (avec érasure-coding / échantillonnage).
- Ajoute un champ à `ModuleEntry`/`Anchor` (engagement DA) → changement de format
  (bump de version, migration append comme 0010/0011).
- Le choix du **backend DA** (on-chain borné vs externe) est une décision d'architecture par
  type de module — l'interface doit rester agnostique.

## Alternatives écartées

- **Tout publier on-chain** : rejeté — VinX est monnaie pure, l'état ne doit pas gonfler des
  données de modules (contredit 0026/0013). Seul l'**engagement** est on-chain.
- **Faire confiance à l'opérateur pour la disponibilité** : rejeté — sans preuve, le bond ne
  garantit rien (0023 devient invérifiable).
- **Format de preuve propre à chaque module** : rejeté — fragmente l'auditabilité ; un standard
  Merkle unique aligné sur 0014 est préférable.

## Notes d'implémentation

- `vinx-core` : type `DaCommitment` (hash du blob + backend + version), canonique + vecteur
  doré ; étendre `ModuleOp::Anchor` d'un engagement DA.
- `vinx-state` : `ModuleEntry` porte le dernier engagement ; exposer les racines pour les
  preuves.
- `vinx-node/rpc` : endpoint de preuve d'inclusion (réutilise la machinerie Merkle du
  state_root, format aligné 0014).
- Tranche 1 = engagement + preuve d'inclusion ; tranche 2 = contestation d'indisponibilité
  (érasure-coding / DAS) → alimente 0023.
- Dépendances : ADR 0010 (registre) ; aligné ADR 0014 (light client) ; prérequis d'ADR 0023
  (slashing de fraude).
