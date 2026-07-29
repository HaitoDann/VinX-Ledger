# ADR 0005 — Temps réseau robuste

- **Statut :** Proposé
- **Catégorie :** Robustesse · **Priorité :** 🟠 moyenne
- **Date :** Juillet 2026
- **Lié :** émission (ADR fair launch), déliaison de bond, ADR 0002 (finalité).

## Contexte

Depuis le fair launch, deux garanties monétaires **font confiance au `timestamp` du
bloc** : l'émission (décroît selon le temps réel écoulé) et la déliaison de bond (mûrit
après 3 jours réels). Or ce timestamp est posé par **un seul producteur**.

Bornes actuelles : à la **production**, on force la monotonie et on plafonne à l'horloge
locale (`producer.rs`). Mais à la **réception** P2P/sync, aucune borne stricte n'est
appliquée. À n≥2, un producteur malhonnête pourrait gonfler son timestamp pour
sur-émettre à son profit ou raccourcir une déliaison.

## Décision proposée

1. **Bornes strictes à la réception** (P2P `NewBlock`, `SyncResponse`, `validate_block`) :
   rejeter tout bloc dont `timestamp < timestamp(prev)` (monotonie) ou
   `timestamp > horloge_locale + TOLÉRANCE` (ex. 120 s).
2. **Median Time Past (MTP)** façon Bitcoin : les comparaisons temps-sensibles
   (émission, déliaison, préavis d'upgrade) utilisent la **médiane des `k` derniers
   timestamps** (ex. k=11) plutôt que le timestamp brut du dernier bloc — un producteur
   seul ne peut plus faire sauter le temps.
3. **Plafonner l'incrément par bloc** pour éviter les sauts avant.

## Conséquences

- **+** Robuste contre la manipulation du temps par un producteur isolé — indispensable
  puisque l'émission, c'est de la monnaie.
- **−** Le MTP retarde légèrement le « temps courant » (négligeable face à des fenêtres
  de l'ordre du jour). Petite complexité (fenêtre glissante dans `Chain`).

## Alternatives écartées

- **Oracle de temps externe** : dépendance externe, contraire à la souveraineté du
  protocole.
- **Ne rien faire** : inacceptable dès n≥2 (l'émission est falsifiable).

## Notes d'implémentation

Helper `median_time_past()` sur `Chain` ; bornes dans `consensus::validate_block` + les
handlers P2P ; brancher les comparaisons d'émission/déliaison sur le MTP. Testable avec
des blocs synthétiques ; l'exploit ne se manifeste qu'à n≥2 (banc 3-validateurs).
