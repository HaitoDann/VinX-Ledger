# Feuille de route de VinX Ledger

Ce document fixe le positionnement de VinX Ledger, les briques prévues, les
usages visés, ce que le projet refuse et l'ordre des versions. Il fait
référence pour toute nouvelle proposition.

## 1. Positionnement

> **VinX Ledger est une blockchain à fonctions natives pour payer, garantir et
> posséder : simple, sûre et rapide, sur laquelle particuliers, commerçants,
> associations et projets construisent leurs usages sans jamais écrire de
> contrat intelligent.**

- **Une chaîne de paiement à fonctions natives**, dans la famille du XRP Ledger
  et de Stellar (avant ses contrats). Ce n'est ni une chaîne à contrats
  intelligents (Ethereum, Solana), ni une « appchain » au service d'une seule
  application.
- **Chaque fonction est écrite une fois dans le protocole**, relue, testée et
  partagée par tous. Une nouvelle fonction passe par une mise à jour du
  protocole, décidée par la gouvernance (SPEC §9).
- **Les applications vivent hors de la chaîne.** VinX garantit l'argent, la
  propriété, les signatures et la date ; la logique et les données de
  l'application restent chez son opérateur.
- **Les subnets, les micropaiements et le paiement du quotidien sont des
  usages du même rail**, pas des identités concurrentes.

## 2. Principes non négociables

1. **Supply fixe** : 1 milliard de VINX, aucune création monétaire, jamais
   (invariant §4.3).
2. **État minimal** : la chaîne ne garde que des soldes, des propriétaires, des
   engagements et des empreintes. Jamais de fichiers ni de données
   d'application. Les blocs sont élagués après 24 h.
3. **Fonctions génériques seulement** : aucune règle propre à un usage
   particulier (un subnet, un commerçant, une application) n'entre dans le
   protocole.
4. **Les VINX entrent en circulation par l'escrow (gouvernance) et par
   l'émission des validateurs (automatique, ADR 0002)**, jamais par création
   au-delà du milliard.
5. **Pas de contrats intelligents.** Si un jour il faut plus de souplesse, ce
   sera un langage de conditions limité (signatures, dates, secrets), jamais
   des programmes.
6. **Occuper l'état a un coût** : tout objet durable (compte, ligne de jeton,
   titre, séquestre) doit immobiliser une réserve, rendue à sa suppression.
   Depuis le protocole 6, les jetons acceptés, les mandats et les règles de
   compte immobilisent chacun 0,1 VINX ; les noms se paient chaque année.

## 3. Les briques

| Version | Brique | Ce qu'elle fait | État |
|---|---|---|---|
| Protocole 1 | Paiement avec mémo | Payer, avec référence ; mémo obligatoire possible | Actif |
| Protocole 1 | Rotation de clé | Changer de clé sans changer d'adresse | Actif |
| Protocole 1 | Gouvernance | Multisig 2 sur 3, paramètres, validateurs, mises à jour programmées | Actif |
| Protocole 2 | Noms `.vinx` | Un nom à la place d'une adresse, un an renouvelable | Activé au bloc 70 000 du testnet |
| Protocole 2 | Séquestre | Fonds bloqués, versés ou remboursés, arbitre, échéance | Activé au bloc 70 000 du testnet |
| **Protocole 3** | Émission des validateurs | 100 M sur une courbe à demi-vie de 10 ans, répartis par époque selon les blocs proposés ; frais aux validateurs (ADR 0002) | Testnet remis à zéro en 0.10 |
| **Protocole 4** | Paiements groupés | Jusqu'à 100 destinataires en une transaction, frais par tranche de 10 | Fait (0.11, lot 4a) |
| | Horodatage natif | Transaction dédiée, retrouvée par sa seule empreinte | Fait (0.11, lot 4a) |
| | Comptes multisig | « 2 signatures sur 3 » pour dépenser, pour tous | Fait (0.12, lot 4b) |
| | Mandats de prélèvement | Un tiers prélève jusqu'à X par période, révocable | Fait (0.13, lot 4c) |
| | Frais payés par un tiers | Le commerçant paie les frais du client | Fait (0.13, lot 4c) |
| | Plafonds de dépense | Clé « téléphone » limitée, clé « coffre » sans limite | Fait (0.12, lot 4b) |
| | Récupération sociale | Des proches remplacent une clé perdue, avec délai d'opposition | Prévu |
| | Arbres pour tout l'état | Noms, séquestres, règles et mandats rangés comme les comptes (voir §6) | Fait (0.14, lot 4d) |
| **Protocole 5** | Jetons de tiers | Monnaies, points, crédits : symbole lié à un nom `.vinx`, accord préalable, pouvoirs déclarés (émission, gel, reprise forcée) et abandonnables, création payante (100 VINX) | Règles faites (0.15, lot 5a), API et ligne de commande faites (0.16, lot 5b) ; portefeuille fait (0.17, lot 5c) ; séquestres et mandats en jetons faits (0.18, lot 5d) |
| | Titres (non fongibles) | Collections d'objets uniques : billets, certificats, cartes de membre | Prévu |
| **Protocole 6** | Réserve par objet | Chaque jeton accepté, mandat ou règle de compte immobilise 0,1 VINX, rendus à sa suppression | Fait (0.21, lot 1c) |
| **Protocole 7** | Canaux de paiement | Un dépôt, des reçus signés hors chaîne, un règlement : micropaiements | Prévu |
| Plus tard | Paiements conditionnels | Secret et délai : échanges sûrs avec d'autres chaînes | Si besoin |
| Plus tard | Clés post-quantiques | ML-DSA, via la rotation de clé | Avant que ça presse |

## 4. Les usages visés

- **Particuliers** :
  - payer au quotidien, par QR ou par nom `.vinx` ;
  - acheter à un inconnu par séquestre ;
  - compte commun en multisig ;
  - plafonds et récupération sociale contre la perte du téléphone ;
  - horodater un document ;
  - détenir billets et cartes de membre.
- **Commerçants et indépendants** :
  - caisse par QR, frais pris en charge ;
  - abonnements par mandat de prélèvement ;
  - fidélité et bons d'achat en jetons ;
  - cautions de location en séquestre.
- **Entreprises** :
  - salaires et fournisseurs en paiements groupés ;
  - trésorerie en multisig ;
  - paiement à la livraison ;
  - certificats d'authenticité en titres ;
  - horodatage de contrats.
- **Associations et collectivités** :
  - caisse transparente en multisig ;
  - cotisations par prélèvement ;
  - cartes de membre ;
  - monnaie locale ;
  - financement participatif en séquestre.
- **Événements** :
  - billetterie en titres, revente sécurisée, compostage à l'entrée ;
  - jetons de consommation.
- **Projets et subnets** :
  - réseaux de services rémunérés (calcul, stockage, sondes, IA) ;
  - paiement à l'usage par canaux ;
  - cautions des participants ;
  - crédits en jetons.
- **Émetteurs régulés (plus tard)** :
  - un euro numérique émis par un établissement agréé, avec gel déclaré.

## 5. Ce que VinX refuse

- Les contrats intelligents et les applications dont la logique tourne sur la chaîne.
- La finance décentralisée : place de marché intégrée, prêts automatiques,
  stablecoins algorithmiques.
- Le stockage de données sur la chaîne.
- Toute création de VINX, y compris pour récompenser des subnets.
- Copier l'économie de Bittensor (TAO) : on reprend la structure des subnets
  (comité, caisse, cautions, répartitions), pas l'émission.

## 6. Stockage : ce qu'il faut tenir

| Élément | Taille | Maîtrise |
|---|---|---|
| Historique des blocs | Élagué après 24 h ; ~17 Go par jour au maximum théorique (1 000 tx/s en continu) | Un seul nœud archive facultatif |
| Comptes | ~150 à 200 Mo pour 1 million | Réserve minimale de 0,1 VINX |
| Noms, séquestres, règles, mandats | Arbres et écriture ligne par ligne depuis le protocole 4 : 0,2 ms par bloc avec 200 000 noms, contre 500 ms avant | Fait (0.14) |
| Jetons et titres | Une ligne par détenteur, un enregistrement par titre | Réserve par objet, accord préalable, nettoyage des noms expirés |

## 7. Ordre des versions

| Étape | Type | Contenu |
|---|---|---|
| 0.8.x | Logiciel et protocole 2 | Portefeuille, noms, séquestres, nœud public (fait) |
| **0.9** | Logiciel | Demandes de paiement (lien `vinx:` et QR), export CSV, horodatage de documents |
| **0.10** | Protocole 3, nouvelle genèse | Émission des validateurs, frais aux validateurs, trésorerie de genèse supprimée (ADR 0002) |
| **0.11** | Protocole 4, lot 4a | Paiements groupés, horodatage natif |
| **0.12** | Protocole 4, lot 4b | Comptes multisig, plafonds journaliers et clé coffre |
| **0.13** | Protocole 4, lot 4c | Mandats de prélèvement, frais payés par un parrain |
| **0.14** | Protocole 4, lot 4d | Arbres pour tout l'état : une modification ne recalcule et n'écrit que sa ligne |
| **0.15** | Protocole 5, lot 5a | Actifs natifs : création, émission, transferts, gel, reprise forcée, accord préalable |
| **0.16** | Logiciel, lot 5b | API des actifs, commandes `vinx-wallet asset` et `vinx-wallet name`, guide de l'émetteur |
| **0.17** | Logiciel, lot 5c | Onglet Jetons du portefeuille : accepter, envoyer, créer, émettre, geler, reprendre |
| **0.18** | Protocole 5, lot 5d | Séquestres et mandats de prélèvement en jetons |
| **0.18** | Release, nouvelle genèse | Protocole 5 complet (séquestres et mandats en jetons) ; testnet 2 (`vinx-testnet-2`) |
| **0.19** | Logiciel, lot A1 | Portefeuille mobile : cœur en WebAssembly, payer, recevoir, scanner un QR, jetons ; API ouverte aux autres origines |
| Ensuite | Logiciel, lot A2 | Application Android (`.apk`) publiée dans les releases (fait) |
| **0.20** | Logiciel, lot 1a | Explorateur public (`/explorer`) : blocs, transactions, comptes, jetons, noms, recherche |
| — | Exercices, lot 1b | Exercices de panne sur le testnet 2 : 4 sur 4 réussis (`docs/EXERCICES-DE-PANNE.md`) |
| **0.21** | Protocole 6, lot 1c | Réserve par objet ; première mise à jour du protocole sans remise à zéro (`docs/MISE-A-JOUR-PROTOCOLE-6.md`) |
| **0.22** | Logiciel, lot D1 | API documentée (`/docs`, `/openapi.json`) ; détenteurs et historique d'un jeton |
| **0.23** | Logiciel, lot I2 | Console émetteur (onglet « Émettre ») : création guidée, fiche du jeton, détenteurs, distribution en lot |
| **0.24** | Logiciel, lot I1 | Page jeton de l'explorateur : circulation, détenteurs et répartition, pouvoirs expliqués, historique |
| **0.25** | Logiciel, lot I3 | Parcours du détenteur : découvrir et accepter un jeton, cartes de jetons, réserve expliquée, demandes de paiement en jeton (bureau et mobile) |
| **0.26** | Logiciel, lot I4 | Cockpit : onglet Gouvernance (à faire, compte à rebours de la mise à jour, préparation des nœuds, programmation guidée) ; `supported_protocol` dans `/status` |
| **0.27** | Sécurité, lot S1 | Attestations de provenance des releases, clé fixe de l'APK, contrôle des dépendances (cargo-deny), nonces consécutifs en ligne de commande |
| — | Robustesse, lot S2 | Exercices de panne rejoués en CI (quorum perdu puis rétabli, arrêt brutal de tout le réseau), fuzzing des jetons et de l'API à chaque PR, fuzzing long nocturne (cargo-fuzz) |
| **0.28** | Réseau, lot 3 | VinX Consensus (document de référence), charte des validateurs, candidature, spécification du protocole 7 (projet), nœud public Copper (`--node-name`) |
| À venir | Protocole 7 | Registre des validateurs nommés, diversité, caution et sanction, présence et récompenses, mise à l'écart, délai des actions admin (`docs/PROTOCOLE-7.md`) |
| Protocole 5 | Règles | Jetons et titres, réserve par objet |
| Protocole 7 | Règles | Canaux de paiement |
| En parallèle | Hors chaîne | Pilote VinX Ping (premier subnet), conception des subnets |

Une règle de protocole ne se retire pas : chaque version de protocole n'est
lancée que lorsqu'un besoin réel la justifie (testeurs, pilote, premier émetteur).

## 8. Le format des demandes de paiement

```
vinx:DESTINATAIRE?amount=12.5&memo=FAC-42&label=Pizza+de+vendredi
```

- `DESTINATAIRE` : une adresse `vinx1…` ou un nom `.vinx`.
- `amount` : point décimal, 9 décimales au plus en VINX, ou les décimales du
  jeton (facultatif).
- `asset` : le symbole d'un jeton, pour une demande en jeton (facultatif ; sans
  lui, la demande est en VINX). Le payeur doit avoir accepté ce jeton.
- `memo` : la référence, recopiée dans le mémo du paiement (facultatif).
- `label` : le motif affiché au payeur, jamais envoyé sur la chaîne (facultatif).

Le portefeuille préremplit l'envoi ; le payeur vérifie toujours l'aperçu avant
de confirmer.
