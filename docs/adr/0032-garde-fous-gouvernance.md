# ADR 0032 — Garde-fous de gouvernance (bornes de paramètres)

- **Statut :** Proposé — 🚧 **brouillon de discussion** (aucune borne n'est arrêtée ; ce
  document cadre le problème et liste les décisions à trancher ensemble)
- **Catégorie :** Gouvernance · **Priorité :** 🟠 moyenne-haute
- **Date :** Juillet 2026
- **Liens :** encadre les pouvoirs ouverts par le multisig K-of-M (ADR 0011) ; interagit avec
  l'immuabilité de l'émission (ADR 0021), l'arrêt d'urgence (ADR 0017) et le préavis d'upgrade
  (ADR 0006).

> ⚠️ **Ce ADR est délibérément non prescriptif.** Des bornes mal calibrées peuvent être
> **pires** que l'absence de bornes (neutraliser la gouvernance en pleine crise). Il pose les
> questions ; les valeurs et le partage immuable/gouvernable seront décidés en discussion.

## Contexte

Depuis l'ADR 0011, un **comité K-of-M** (ou l'admin mono-clé legacy) peut :

- `UpdateFeeFloor` — changer le plancher de frais ;
- `AddValidator` / `RemoveValidator` — modifier le set de validateurs ;
- `ScheduleUpgrade` — planifier un upgrade protocolaire ;
- `SetAdminPolicy` — redéfinir le comité lui-même.

Quelques gardes existent déjà (refus de retirer le **dernier** validateur ; bond requis à
l'ajout ; préavis d'upgrade, ADR 0006 ; nonce consommé seulement au succès). Mais **rien ne
borne l'amplitude** de ces pouvoirs. Un comité **capté, corrompu ou piraté** pourrait :

- mettre `fee_floor` à un niveau **astronomique** → censure de fait (plus personne ne peut
  payer) / DoS économique ;
- **vider** le set jusqu'à un seul validateur → recentralisation, voire chaîne à la merci
  d'une seule clé ;
- `SetAdminPolicy` vers un comité **inatteignable** (seuil impossible, signataires perdus) →
  gouvernance **verrouillée à jamais** ;
- enchaîner des upgrades malveillants (dans les limites du préavis).

Le risque n'est pas théorique : la surface d'attaque de la gouvernance **augmente** avec la
valeur du réseau. Des **garde-fous** — des bornes dures dans la transition d'état, que **même
la gouvernance ne peut franchir** — réduisent la casse maximale d'un comité hostile.

## La tension centrale (pourquoi il faut en discuter)

C'est un arbitrage à **double tranchant** :

> **Résistance à la capture** (borner fort pour qu'un comité hostile ne puisse pas bricoler la
> chaîne) **⟂ Réactivité en urgence** (laisser la gouvernance répondre vite à une crise réelle
> — monter fortement les frais sous une attaque de spam, éjecter vite un validateur
> compromis, corriger un bug via upgrade).

Sur-contraindre neutralise la gouvernance **quand on en a le plus besoin** ; sous-contraindre
laisse la porte à la capture. Le bon design n'est pas « le plus de bornes possible » — c'est le
**minimum de bornes qui empêche les issues irréversibles**, en laissant les manœuvres
réversibles libres.

Heuristique proposée pour trancher chaque paramètre :
- **Borner dur** ce qui est **irréversible ou fatal** (verrouiller la gouvernance, vider le set).
- **Laisser libre (ou borne souple)** ce qui est **réversible** (les frais peuvent monter puis
  redescendre ; un validateur retiré peut revenir).
- Préférer des **bornes de *vitesse*** (taux de changement max par période) aux bornes de
  *niveau* absolu, quand c'est possible — elles limitent l'abus brutal sans figer les
  réponses légitimes.

## Pistes de bornes (à débattre, **rien n'est arrêté**)

| Pouvoir | Risque si non borné | Piste de garde-fou | Réversible ? |
|---|---|---|---|
| `UpdateFeeFloor` | frais censurants | plafond absolu **et/ou** taux de hausse max par fenêtre | oui → borne souple |
| `RemoveValidator` | vidage du set | **plancher de taille** du set (> 1, à définir) ; taux de retrait max | partiellement |
| `AddValidator` | inflation Sybil du set | bond déjà requis ; éventuel plafond de taille | oui |
| `ScheduleUpgrade` | upgrades malveillants en rafale | préavis (0006) déjà ; fréquence max | oui |
| `SetAdminPolicy` | **gouvernance verrouillée** | seuil ∈ `1..=M`, `M ≤ MAX_ADMIN_SIGNERS` (déjà) ; interdiction d'un comité **prouvablement inatteignable** | **non → borne dure** |

## Le méta-problème : qui garde les gardiens ?

Décision structurante à trancher **ensemble** :

- Si les bornes sont **gravées immuables** (comme l'émission, ADR 0021) → increvables par un
  comité hostile, **mais** un besoin légitime futur de les changer exige un **hard-fork**
  (upgrade protocolaire).
- Si les bornes sont **elles-mêmes gouvernables** → ce ne sont plus vraiment des bornes (un
  comité captif les desserre d'abord).
- Voie médiane possible : **bornes gravées** pour les issues *irréversibles* (verrouillage de
  gouvernance, plancher du set), **bornes gouvernables-dans-une-méta-borne** pour le
  réversible (frais). À arbitrer paramètre par paramètre.

Lien avec l'ADR 0017 (arrêt d'urgence) : un garde-fou *ne doit pas* empêcher un halt coordonné
légitime — les deux ADR doivent être conçus **ensemble** (le halt est la soupape de dernier
recours ; les bornes empêchent l'abus courant).

## Questions ouvertes à trancher ensemble

1. **Bornes de niveau vs de vitesse** : plafond absolu de `fee_floor`, ou taux de hausse max ?
   (Je penche pour la vitesse — moins susceptible de mal vieillir.)
2. **Plancher du set de validateurs** : quelle valeur minimale « sûre » ? Est-elle gravée ou
   gouvernable-dans-une-borne ?
3. **Immuable vs gouvernable** : quels garde-fous grave-t-on définitivement, lesquels
   restent ajustables ? (le cœur de la discussion)
4. **`SetAdminPolicy` sûr** : comment prouver on-chain qu'un comité est *atteignable* (éviter
   le verrouillage) sans pouvoir vérifier la possession des clés ? (peut-être : exiger que les
   nouveaux signataires **co-signent** la transition, prouvant qu'ils sont vivants.)
5. **Articulation avec 0017** : les bornes laissent-elles toujours passer un halt d'urgence ?
6. **Rétroactivité** : les bornes s'appliquent-elles au comité *actuel* ou seulement aux
   futures policies ?

## Conséquences (esquisse)

**Positif** — réduit la casse maximale d'une gouvernance captée/piratée ; rend les pouvoirs
gouvernementaux *prévisibles* (bon pour la confiance).
**Coûts** — risque de **figer une réponse légitime** si mal calibré (d'où la prudence) ;
consensus-critique (les bornes entrent dans la transition d'état) ; probable bump de version
si de nouveaux compteurs (taux de changement) sont ajoutés.

## Alternatives écartées (provisoire)

- **Tout graver, zéro pouvoir gouvernable** : rejeté — une chaîne doit pouvoir s'adapter ; ça
  déplace juste le problème vers des hard-forks constants.
- **Faire confiance au comité (statu quo)** : insuffisant à mesure que la valeur croît — la
  question 3 mérite une réponse explicite, pas un défaut implicite.

## Prochaine étape

**Discussion dédiée** sur les questions ouvertes (surtout #1, #3, #4) **avant** toute
implémentation. Ce document sert de base ; il passera de « brouillon » à « Accepté » une fois
les arbitrages faits.
