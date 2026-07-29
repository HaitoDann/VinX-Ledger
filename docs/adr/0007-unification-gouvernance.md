# ADR 0007 — Unification des chemins de gouvernance

- **Statut :** Proposé
- **Catégorie :** Cohérence · **Priorité :** 🟠 moyenne
- **Date :** Juillet 2026

## Contexte

Deux chemins distincts font la même chose :

- Les **types de transaction dédiés** `AddValidator` (0x05) et `RemoveValidator` (0x06).
- Les **`GovernanceAction::AddValidator` / `RemoveValidator`** exécutées via
  `AdminAction` (0x08).

Leurs sémantiques divergent légèrement : le type dédié **refuse** un doublon avec une
erreur, alors que la version `AdminAction` **l'ignore silencieusement**. C'est de la
duplication (double maintenance, double surface de test) et une source d'incohérence —
exactement le genre de « saleté » que le projet veut éviter dans le cœur.

## Décision proposée

**Un seul chemin de gouvernance : `AdminAction` (0x08).**

- Retirer (ou réduire à de simples alias dépréciés) les types de tx `0x05`/`0x06`.
- Centraliser la validation (bond requis, dernier validateur, doublon) dans un endroit
  unique, avec une sémantique unique.
- Le wallet CLI, la console admin et le desktop utilisent `admin-action`.

## Conséquences

- **+** Une seule sémantique, un seul point de validation, moins de discriminants.
- **−** **Consensus-breaking** (renumérotation/retrait de discriminants) → nouvelle genèse
  ou upgrade planifié. Acceptable en pré-mainnet.

## Alternatives écartées

- **Garder les deux** : rejeté — la duplication est précisément la dette qu'on refuse
  dans la monnaie pure.
- **Garder les types dédiés, retirer `AdminAction`** : rejeté — `AdminAction` est plus
  général (il porte déjà `UpdateFeeFloor`, `RotateAdmin`, `ScheduleUpgrade`).

## Notes d'implémentation

Aligner d'abord les sémantiques (choisir : erreur vs ignore sur doublon), puis retirer
les handlers `apply_add_validator` / `apply_remove_validator` dédiés au profit du dispatch
`AdminAction`. Mettre à jour vecteurs dorés et clients.
