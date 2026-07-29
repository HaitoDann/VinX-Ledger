# ADR 0020 — Sérialisation canonique consensus-critique

- **Statut :** Proposé
- **Catégorie :** Robustesse · **Priorité :** 🟠 moyenne
- **Date :** Juillet 2026

## Contexte

Toute suite d'octets qui entre dans un **hash signé** doit avoir un **encodage canonique
unique** — sinon deux implémentations (ou deux versions) peuvent diverger, et une
malléabilité devient possible.

État actuel :
- `Transaction::signing_bytes`, `BlockHeader::hash`, `hash_account` sont **écrits à la
  main** en big-endian à largeur fixe → déterministes et verrouillés par vecteurs dorés.
  **C'est bon.**
- Mais des **payloads** consensus-pertinents passent par `bincode`/`borsh` :
  `SlashEvidence`, `GovernanceAction`, et le format wire P2P. Leur canonicité n'est pas
  formellement garantie (ordre de map, largeurs, variantes d'enum, absence de flottant).

## Décision proposée

- **Auditer et figer** l'encodage de toutes les structures consensus-critiques
  (payloads inclus). Interdire tout sérialiseur non déterministe sur le hot-path.
- Ajouter des **vecteurs dorés** pour les payloads (SlashEvidence, GovernanceAction) comme
  pour `signing_bytes`.
- **Documenter** la disposition canonique comme partie de la spec (prérequis pour une
  éventuelle 2ᵉ implémentation).

## Conséquences

- **+** Empêche la malléabilité et la divergence inter-implémentations ; condition
  nécessaire à un client alternatif ou à un audit sérieux.
- **−** Surtout du travail d'audit + tests ; coût runtime négligeable.

## Alternatives écartées

- **Supposer que « ça marche »** : rejeté — le consensus exige des garanties de
  déterminisme, pas des suppositions.

## Notes d'implémentation

Vérifier que borsh (déjà utilisé sur le wire) est bien déterministe pour nos types ;
préférer borsh à bincode pour les payloads signés ; ajouter des tests de round-trip +
vecteurs dorés. Lister explicitement dans la spec chaque champ et sa largeur.
