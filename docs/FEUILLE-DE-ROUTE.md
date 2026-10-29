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
   titre, séquestre) immobilise une réserve, rendue à sa suppression.

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
| | Mandats de prélèvement | Un tiers prélève jusqu'à X par période, révocable | Prévu |
| | Frais payés par un tiers | Le commerçant paie les frais du client | Prévu |
| | Plafonds de dépense | Clé « téléphone » limitée, clé « coffre » sans limite | Fait (0.12, lot 4b) |
| | Récupération sociale | Des proches remplacent une clé perdue, avec délai d'opposition | Prévu |
| | Arbres pour tout l'état | Noms et séquestres rangés comme les comptes (voir §6) | Prévu |
| **Protocole 5** | Jetons de tiers | Monnaies, points, crédits : accord préalable, gel déclaré, émetteur visible, création payante | Prévu |
| | Titres (non fongibles) | Collections d'objets uniques : billets, certificats, cartes de membre | Prévu |
| | Réserve par objet | Chaque ligne de jeton ou titre immobilise un dépôt | Prévu |
| **Protocole 6** | Canaux de paiement | Un dépôt, des reçus signés hors chaîne, un règlement : micropaiements | Prévu |
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
| Noms et séquestres | Recalculés en entier à chaque bloc aujourd'hui | **À ranger dans des arbres avant le protocole 5** |
| Jetons et titres | Une ligne par détenteur, un enregistrement par titre | Réserve par objet, accord préalable, nettoyage des noms expirés |

## 7. Ordre des versions

| Étape | Type | Contenu |
|---|---|---|
| 0.8.x | Logiciel et protocole 2 | Portefeuille, noms, séquestres, nœud public (fait) |
| **0.9** | Logiciel | Demandes de paiement (lien `vinx:` et QR), export CSV, horodatage de documents |
| **0.10** | Protocole 3, nouvelle genèse | Émission des validateurs, frais aux validateurs, trésorerie de genèse supprimée (ADR 0002) |
| **0.11** | Protocole 4, lot 4a | Paiements groupés, horodatage natif |
| **0.12** | Protocole 4, lot 4b | Comptes multisig, plafonds journaliers et clé coffre |
| Protocole 4, lots 4c et 4d | Règles | Mandats et frais payés par un tiers, arbres pour tout l'état |
| Ensuite | Logiciel | Portefeuille Android (`.apk`), scan de QR |
| Protocole 5 | Règles | Jetons et titres, réserve par objet |
| Protocole 6 | Règles | Canaux de paiement |
| En parallèle | Hors chaîne | Pilote VinX Ping (premier subnet), conception des subnets |

Une règle de protocole ne se retire pas : chaque version de protocole n'est
lancée que lorsqu'un besoin réel la justifie (testeurs, pilote, premier émetteur).

## 8. Le format des demandes de paiement

```
vinx:DESTINATAIRE?amount=12.5&memo=FAC-42&label=Pizza+de+vendredi
```

- `DESTINATAIRE` : une adresse `vinx1…` ou un nom `.vinx`.
- `amount` : en VINX, point décimal, 9 décimales au plus (facultatif).
- `memo` : la référence, recopiée dans le mémo du paiement (facultatif).
- `label` : le motif affiché au payeur, jamais envoyé sur la chaîne (facultatif).

Le portefeuille préremplit l'envoi ; le payeur vérifie toujours l'aperçu avant
de confirmer.
