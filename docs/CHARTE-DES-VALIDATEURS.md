# Charte des validateurs de VinX Ledger

Un validateur VinX est un acteur **identifié** qui s'engage à faire tourner un
nœud fiable, à jour et honnête. Cette charte dit ce qu'on attend de lui, ce
qu'il reçoit, comment il entre et comment il sort. Le fonctionnement du réseau
est décrit dans [`VINX-CONSENSUS.md`](./VINX-CONSENSUS.md).

Les règles marquées **[P7]** seront appliquées par la chaîne elle-même à partir
du protocole 7 ([`PROTOCOLE-7.md`](./PROTOCOLE-7.md)) ; d'ici là, elles sont
des engagements.

## 1. Identité

- **Un nom** par validateur, choisi dans le thème du réseau : marchandises des
  routes commerciales (Cinnamon, Amber, Obsidian, Pearl, Saffron, Indigo,
  Jade, Silk, Ivory, Myrrh, Cardamom, Ebony…). Un nom est **unique et jamais
  réattribué**, même après le départ de son validateur.
- **Un opérateur** déclaré : l'entreprise, l'association ou la personne
  responsable du nœud, avec son pays, son site et un contact.
- Ces informations sont **publiques** : explorateur, API, documentation.
  **[P7]** Elles sont inscrites dans la chaîne.

Le nom **Copper** est réservé au nœud public du projet, qui n'est pas un
validateur.

## 2. Diversité

Un même opérateur ne peut pas faire tourner assez de validateurs pour bloquer la
chaîne à lui seul : au plus `⌊(n − 1)/3⌋` sièges sur `n` (1 sur 4, 2 sur 7,
4 sur 13). **[P7]** La chaîne refuse un ajout qui violerait cette règle.

Pendant le lancement, quand le projet fait tourner seul tous les validateurs,
cette règle est désactivée ; la gouvernance l'active dès que des opérateurs
externes ont rejoint le réseau.

## 3. Obligations

| Domaine | Engagement |
|---|---|
| Disponibilité | **97 % au moins** des blocs signés. En dessous, pas de récompense sur la période **[P7]** ; aucun bloc signé sur les 1 000 derniers : mise à l'écart automatique **[P7]** |
| Mises à jour | Nouvelle version installée **avant** la hauteur d'activation d'une mise à jour du protocole (préavis d'au moins 24 h) ; correctifs de sécurité sous 72 h |
| Sécurité | Clé du validateur chiffrée, jamais copiée sur un autre nœud actif (un double démarrage provoque une double signature) ; serveur à jour, pare-feu, accès SSH par clé seulement |
| Honnêteté | Jamais de double signature : elle fait perdre **toute la caution** et exclut définitivement **[P7]** |
| Supervision | Alertes actives (`deploy/monitoring`) ; incident signalé à la gouvernance sous 24 h |
| Contact | Une adresse joignable, mise à jour en cas de changement |

## 4. Ce que reçoit un validateur

- Une part de l'**émission** (100 M VINX sur la durée, demi-vie 10 ans) et des
  **frais de transaction**, chaque jour (17 280 blocs). **[P7]** Au prorata
  des blocs signés et proposés, pour les validateurs disponibles au moins
  97 % du temps.
- Sa part des **cautions confisquées** aux validateurs qui trichent **[P7]**.
- La visibilité de son nom et de son opérateur sur le réseau.

## 5. Caution [P7]

- **10 000 VINX**, montant paramétrable par la gouvernance, déposés avant
  l'entrée et bloqués tant que le validateur est actif.
- **Perdue en totalité** en cas de double signature prouvée, et répartie à
  parts égales entre les autres validateurs actifs.
- **Rendue** après le départ, une fois écoulé un délai de sécurité (14 jours,
  paramétrable) pendant lequel une faute passée reste sanctionnable.
- **Conservée** en cas de simple absence : la mise à l'écart n'est pas une
  sanction.

## 6. Entrée

1. **Candidature** : remplir le [modèle de candidature](./CANDIDATURE-VALIDATEUR.md)
   et l'envoyer à la gouvernance.
2. **Installation** : nœud installé selon
   [`DEPLOYER-UN-VALIDATEUR.md`](./DEPLOYER-UN-VALIDATEUR.md), binaire vérifié
   ([`VERIFIER-UNE-RELEASE.md`](./VERIFIER-UNE-RELEASE.md)), synchronisé en
   observateur.
3. **Contrôle** : la gouvernance vérifie le nœud depuis le cockpit (onglet
   Actions, « Contrôler le nœud ») : version, synchronisation, réseau.
4. **Caution** déposée depuis l'adresse du validateur **[P7]**.
5. **Ajout** : proposition `AddValidator` signée par 2 des 3 clés admin ;
   effectif après le délai des actions admin **[P7]**, puis 2 blocs.

## 7. Sortie

- **Volontaire** : le validateur annonce son départ (préavis souhaité : 7 jours)
  **[P7]** ou demande son retrait à la gouvernance. Il reste en ligne jusqu'à
  sa sortie effective, puis récupère sa caution après le délai de sécurité.
- **Par la gouvernance** : manquements répétés à la charte, opérateur
  injoignable, clé compromise. Retrait par `RemoveValidator` ; la caution est
  rendue, sauf tricherie prouvée.
- **Automatique** **[P7]** : double signature prouvée (caution perdue,
  exclusion définitive).

Le nom d'un validateur sorti n'est jamais réattribué.

## 8. Évolution

La charte évolue par décision de la gouvernance, publiée dans ce dépôt. Les
validateurs sont prévenus au moins 30 jours avant tout durcissement de leurs
obligations.
