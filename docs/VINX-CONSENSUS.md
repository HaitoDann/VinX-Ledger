# VinX Consensus

Comment VinX Ledger décide des blocs, qui les valide, comment ils sont payés et
qui gouverne. Ce document décrit le réseau tel qu'il est, et signale ce
qu'ajoutera le protocole 7 (**[P7]**), dont les règles sont proposées dans
[`PROTOCOLE-7.md`](./PROTOCOLE-7.md).

> **VinX Consensus** : finalité immédiate par un algorithme BFT éprouvé
> (Tendermint) ; validateurs nommés, dont l'identité et l'opérateur sont
> publics ; aucun opérateur ne peut contrôler la chaîne ; récompenses à la
> présence, issues d'une émission décroissante et des frais ; caution perdue
> en cas de tricherie ; gouvernance par une multisignature publique, avec
> délai.

Ce qui est propre à VinX, c'est ce **modèle de réseau**. L'algorithme d'accord
lui-même est Tendermint, choisi parce qu'il est prouvé, audité et utilisé
depuis des années : ce n'est pas un algorithme nouveau, et ce document ne le
présente pas comme tel.

## 1. L'accord sur les blocs

L'algorithme est celui de **Tendermint** (SPEC §7) :

1. un validateur, à tour de rôle, **propose** le bloc suivant ;
2. chaque validateur **vote** une première fois (prevote) s'il le juge valide ;
3. si plus des 2/3 l'ont approuvé, chacun **s'engage** (precommit) ;
4. dès que plus des 2/3 se sont engagés, le bloc est **définitif**.

Conséquences :

- **finalité immédiate** : un paiement inscrit dans un bloc ne peut plus être
  annulé, sans attendre de « confirmations » ;
- **tolérance aux pannes** : avec `n` validateurs, la chaîne continue tant que
  plus des 2/3 répondent. Elle tolère `⌊(n − 1)/3⌋` validateurs en panne ou
  malhonnêtes : 1 sur 4, 2 sur 7, 4 sur 13 ;
- **sûreté avant tout** : si trop de validateurs manquent, la chaîne
  **s'arrête** plutôt que de produire deux versions contradictoires, et repart
  d'elle-même à leur retour (exercices de panne n° 2, rejoués par la CI) ;
- **aucun minage** : pas de calcul inutile, consommation négligeable.

Le quorum (plus des 2/3) n'est **jamais réduit** : c'est lui qui garantit que
deux blocs différents ne peuvent pas être finalisés à la même hauteur. Un
validateur absent est mis à l'écart à la place **[P7]**, et le quorum se
recalcule sur les validateurs restants.

## 2. Les validateurs

VinX est une **preuve d'autorité** (PoA) : les validateurs sont connus, une voix
chacun.

- **Nommés** : chaque validateur porte un nom unique, jamais réattribué
  (Cinnamon, Amber, Obsidian, Pearl…), et déclare l'**opérateur** qui le fait
  tourner (une entreprise, une association, une personne). Aujourd'hui ces noms
  ne sont qu'un réglage d'affichage des nœuds ; **[P7]** les inscrit dans la
  chaîne, avec l'opérateur et le pays, visibles dans l'explorateur.
- **Diversifiés** **[P7]** : un même opérateur ne peut pas détenir assez de
  sièges pour bloquer la chaîne à lui seul.
- **Engagés** **[P7]** : chaque validateur dépose une **caution**
  (10 000 VINX, paramétrable). Une double signature, prouvée par la chaîne
  elle-même, lui fait perdre **toute** sa caution, répartie entre les autres
  validateurs, et l'exclut définitivement.
- **Présents** **[P7]** : un validateur qui ne signe aucun des 1 000 derniers
  blocs est mis à l'écart automatiquement, sans perdre sa caution, et revient
  quand il le demande.

Les obligations des validateurs (disponibilité, mises à jour, sécurité, entrée
et sortie) sont dans la [charte des validateurs](./CHARTE-DES-VALIDATEURS.md).

Le nœud public **Copper** n'est pas un validateur : il suit la chaîne, sert
l'explorateur et l'API, et garde tout l'historique (mode archive).

## 3. L'économie

La supply est fixe : **1 milliard de VINX**, créés à la genèse. Aucune règle,
pas même une mise à jour du protocole, ne peut en créer davantage (SPEC §4.3 et
§9).

| Part | Montant | Rôle |
|---|---|---|
| Trésorerie, par l'escrow | 900 000 000 VINX | Financement du projet et des utilisateurs : 120 tranches mensuelles de 7 500 000 VINX sur 10 ans, libérées par la gouvernance |
| Émission des validateurs | 100 000 000 VINX | Rémunération des validateurs, selon une courbe décroissante |

**Rémunération des validateurs** (SPEC §4.4) :

- chaque jour (17 280 blocs), une part de l'émission est versée, selon une
  courbe décroissante de **demi-vie 10 ans** : la moitié du budget les dix
  premières années, le quart les dix suivantes, etc. ;
- les **frais de transaction** s'y ajoutent ;
- aujourd'hui, le tout est réparti au prorata des blocs **proposés** ;
  **[P7]** : au prorata des blocs **signés** et **proposés**, et seulement
  pour les validateurs disponibles au moins **97 %** du temps. La part des
  autres revient aux validateurs fiables.

À mesure que l'émission décroît, les frais prennent le relais : les validateurs
sont payés par l'usage du réseau.

## 4. La gouvernance

La gouvernance est une **multisignature de 3 clés admin, 2 signatures
requises** (SPEC §5.3). Elle peut :

- ajouter ou retirer un validateur ;
- modifier les paramètres modifiables (frais minimum, réserve, délais…) ;
- libérer les tranches échues de l'escrow et payer depuis la trésorerie ;
- remplacer un signataire admin ;
- programmer une mise à jour du protocole, au moins 24 h à l'avance.

Elle **ne peut pas** : créer des VINX, toucher au solde d'un compte autre que la
trésorerie, modifier une tranche d'escrow, ni annuler un bloc finalisé.

**Transparence.** Chaque action admin est publique et archivée : journal du
cockpit, API `/admin-history`, explorateur. **[P7]** Les actions sensibles ne
prennent effet qu'après un **délai** (48 h, paramétrable) : chacun peut les
voir venir, et la gouvernance peut annuler une action pendant ce délai.

**Ce qu'il faut savoir.** Tant que les clés admin décident des validateurs, le
réseau n'est pas décentralisé au sens de Bitcoin : il est **transparent et
responsable**. C'est un choix assumé, adapté à un réseau de paiement dont les
validateurs sont des acteurs identifiés.

## 5. Les mises à jour

Les règles évoluent par **versions de protocole**, programmées par la
gouvernance au moins 17 280 blocs (≈ 24 h) à l'avance, et activées à chaud,
sans remise à zéro ni perte de données (première fois : le protocole 6,
[`MISE-A-JOUR-PROTOCOLE-6.md`](./MISE-A-JOUR-PROTOCOLE-6.md)). Le cockpit
vérifie avant que chaque nœud sait exécuter la nouvelle version.

## 6. Garanties et limites

| Garanti | Limite |
|---|---|
| Un bloc finalisé ne peut pas être annulé | La chaîne s'arrête si plus d'un tiers des validateurs manquent |
| Supply fixe, invariant vérifié à chaque bloc | Les clés admin peuvent changer les validateurs et les paramètres (avec délai **[P7]**) |
| Double signature détectée, prouvée et diffusée ; **[P7]** sanctionnée automatiquement | Un opérateur peut perdre ses clés : la gouvernance doit alors le remplacer |
| Code testé : pannes rejouées et fuzzing à chaque modification | Aucun audit de sécurité externe à ce jour : indispensable avant un réseau principal |
