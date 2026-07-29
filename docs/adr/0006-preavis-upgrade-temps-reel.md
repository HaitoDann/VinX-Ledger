# ADR 0006 — Préavis d'upgrade en temps réel

- **Statut :** Accepté (implémenté — consensus-breaking)
- **Catégorie :** Cohérence · **Priorité :** 🟠 moyenne
- **Date :** Juillet 2026
- **Lié :** ADR 0005 (temps réseau), whitepaper §7-§8.

## Contexte

Le whitepaper et le GUIDE annoncent des préavis d'upgrade en **jours réels** (7/30/90).
Le code, lui, les applique encore en **hauteur de bloc** (`UPGRADE_NOTICE_*_BLOCKS`,
`activation_height`). C'est le **dernier écart doc↔code** du projet, et il est incohérent
avec le reste du modèle : depuis le fair launch, l'émission et la déliaison utilisent le
**timestamp**, précisément parce que la hauteur n'est pas une horloge (cadence adaptative).

## Décision proposée

Passer l'activation d'upgrade au **temps réel** :

- `ScheduledUpgrade` porte une `activation_ts` (unix seconds) au lieu de (ou en plus de)
  `activation_height`.
- Contrôle du préavis : `activation_ts ≥ annonce_ts + notice_secs` (7/30/90 j).
- Activation quand `timestamp_du_bloc ≥ activation_ts` (idéalement sur le MTP de l'ADR 0005).
- Constantes `UPGRADE_NOTICE_*_SECS` remplacent `*_BLOCKS`.

## Conséquences

- **+** Aligne les préavis sur émission/déliaison ; ferme l'écart doc↔code ; garantit un
  vrai délai calendaire quelle que soit la cadence.
- **−** **Surface consensus-critique** : change le payload d'`AnnounceUpgrade`
  (hauteur→timestamp), `decode_upgrade_payload`, le **vecteur doré** de `signing_bytes`,
  le wallet CLI, la console admin et le SDK. À coordonner avec un bump de version.

## Alternatives écartées

- **Garder la hauteur + corriger les docs** pour dire « en blocs » : rejeté — réintroduit
  l'incohérence « la hauteur comme horloge » que le fair launch a justement supprimée.

## Notes d'implémentation

Consensus-breaking → nouvelle genèse (acceptable en pré-mainnet) ou activation planifiée
via le mécanisme d'upgrade lui-même. Mettre à jour le vecteur doré et tous les signeurs
(Rust, JS admin, SDK, desktop-core).
