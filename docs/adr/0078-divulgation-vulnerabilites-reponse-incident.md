# ADR 0078 — Divulgation des vulnérabilités & réponse à incident

- **Statut :** Accepté 📐 — politique décidée, `SECURITY.md` et procédures à écrire
- **Date :** Septembre 2026
- **Portée :** Sécurité opérationnelle — recevoir un rapport, et réagir à un incident en cours
- **Décideur :** VinX Labs
- **Complète :** ADR 0017 (arrêt d'urgence), ADR 0079 (release), ADR 0006 (préavis d'upgrade)
- **Crates :** aucun — processus

---

## 1. Contexte

Le dépôt n'a **ni `SECURITY.md`, ni adresse de contact, ni procédure d'incident**. Un
chercheur qui trouve une faille dans un rail de paiement portant de la valeur n'a aujourd'hui
que deux options : ouvrir une issue publique — qui arme l'attaquant avant le correctif — ou
se taire.

L'audit de septembre 2026 a par ailleurs montré que des vulnérabilités critiques
(vol de fonds via le champ `sponsor`, quorum BLS forgeable) sont **atteignables sans clé
privilégiée**. Il faut supposer que d'autres existent, et qu'elles seront trouvées par des
tiers.

Une chaîne ne peut pas non plus « faire un rollback » : une fois un bloc finalisé, la
réponse à incident ne consiste pas à défaire, mais à **arrêter, corriger, redémarrer** — ce
qui exige que les procédures existent **avant** l'incident.

## 2. Décision

### 2.1 Canal de divulgation

Un `SECURITY.md` à la racine énonce :

- une adresse de contact dédiée et une clé publique pour le chiffrement des rapports ;
- l'engagement d'un **accusé de réception sous 48 h** et d'une première évaluation sous 7 j ;
- l'engagement de **ne pas engager de poursuites** contre un chercheur agissant de bonne foi
  dans le périmètre défini ;
- ce qui est **hors périmètre** : ingénierie sociale, DoS volumétrique contre
  l'infrastructure de VinX Labs, tests sur mainnet contre des fonds de tiers.

Les rapports ne passent **jamais** par les issues publiques GitHub.

### 2.2 Classification

| Gravité | Définition | Objectif de correctif |
|---|---|---|
| **Critique** | Vol de fonds, création de monnaie, double finalité, forge de quorum | 24 h |
| **Élevée** | Arrêt du consensus, crash distant de nœud, divergence induite à distance | 72 h |
| **Moyenne** | DoS borné, fuite d'information, contournement d'une garantie non monétaire | 30 j |
| **Faible** | Hardening, défauts sans exploit démontré | Prochain cycle |

La grille est celle utilisée par l'audit de septembre 2026, afin que classification interne
et externe soient comparables.

### 2.3 Embargo

Divulgation coordonnée : publication après déploiement du correctif sur une majorité du set
de validateurs, ou **90 jours** après le rapport, selon ce qui arrive en premier. Le
chercheur est crédité s'il le souhaite.

Un correctif de sécurité **n'est pas** poussé silencieusement sur une branche publique avant
que les validateurs ne soient prêts : un commit trop explicite est une divulgation.
Conséquence directe sur ADR 0079 §5 : les correctifs de sécurité suivent un chemin de
release distinct.

### 2.4 Réponse à incident

Quatre rôles, désignés nommément avant le lancement : **coordinateur** (décide et
communique), **ingénieur** (produit le correctif), **liaison validateurs** (contacte les
opérateurs par un canal hors-bande), **communication** (utilisateurs et intégrateurs).

Déroulé :

1. **Confirmer** — reproduire par un test avant toute action. La discipline de l'audit
   s'applique : un rapport est une hypothèse, y compris un rapport urgent.
2. **Contenir** — selon le cas : arrêt d'urgence coordonné (ADR 0017), filtrage d'un type de
   transaction, ou isolement de nœuds. La décision d'arrêter est du coordinateur.
3. **Corriger** — correctif minimal, avec test de régression, revu par une seconde personne.
4. **Déployer** — coordonné avec les validateurs, hors-bande.
5. **Post-mortem public** — sans blâme, publié même si l'incident n'a causé aucune perte,
   et systématiquement suivi d'un ADR si une décision d'architecture en découle.

### 2.5 Contact hors-bande des validateurs

Une liste de contact des opérateurs de validateurs, **indépendante de la chaîne et du
canal public**, doit exister avant le mainnet. Un incident qui arrête le consensus arrête
aussi tout mécanisme de coordination on-chain.

## 3. Conséquences

- `SECURITY.md`, la clé de contact et la liste hors-bande sont des **prérequis de mainnet**
  (ADR 0080), pas de la documentation optionnelle.
- ADR 0017 (arrêt d'urgence) passe de « futur » à prérequis : la containment de §2.4 en
  dépend. Sans lui, la seule contenance disponible est « demander aux opérateurs d'éteindre ».
- Les délais annoncés engagent : ne les publier que s'ils peuvent être tenus par l'équipe
  réelle.

## 4. Alternatives écartées

| Option | Pourquoi écartée |
|---|---|
| Programme de bug bounty rémunéré dès le lancement | Attire du volume avant que le triage n'existe ; à envisager une fois §2.4 rodé |
| Divulgation complète immédiate | Arme l'attaquant : sur une chaîne, la fenêtre entre publication et déploiement est exploitable |
| Pas de politique, traitement au cas par cas | C'est l'état actuel : un chercheur n'a aucun chemin, donc les failles se découvrent en production |

## 5. Critères de validation

- [ ] `SECURITY.md` publié, avec contact, clé publique, périmètre et délais.
- [ ] Grille de gravité et objectifs de correctif publiés.
- [ ] Rôles d'incident nommément désignés.
- [ ] Liste de contact hors-bande des validateurs constituée et testée par un exercice.
- [ ] Un exercice d'incident simulé réalisé de bout en bout avant le mainnet.
