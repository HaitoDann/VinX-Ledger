# ADR 0021 — Immutabilité de la courbe d'émission

- **Statut :** Proposé
- **Catégorie :** Cohérence · **Priorité :** 🟠 moyenne
- **Date :** Juillet 2026
- **Lié :** fair launch, whitepaper §3 & §9 (règles immuables).

## Contexte

La politique monétaire de VinX repose sur des **constantes de compilation** :
`HALVING_PERIOD_SECS` (8 ans), `ERA0_EMISSION_ATOMS`, le total = `MAX_SUPPLY_ATOMS`. Leur
**statut de gouvernance n'est pas décidé** : peuvent-elles être modifiées par l'admin, par
un upgrade, ou jamais ?

Or une politique monétaire qu'une clé admin pourrait changer **contredit** la promesse de
fair launch : « personne ne décide de la création monétaire ». C'est justement ce que le
projet reproche aux modèles à robinet discrétionnaire.

## Décision proposée

Déclarer la **courbe d'émission immuable** :

- Le total (100 Md), la période de halving (8 ans) et la forme de la courbe **ne sont pas
  gouvernables** — ni par l'admin, ni par une `GovernanceAction`.
- Leur seule voie de changement serait un **upgrade majeur** avec préavis complet — et
  l'esprit de cet ADR est que **cela n'arrive jamais**.
- Inscrire cette règle dans les **« Règles immuables »** du whitepaper (§9), aux côtés du
  cap et de l'absence de burn/pre-mine.

## Conséquences

- **+** Garantie de confiance forte : « nul, pas même le fondateur, ne change le calendrier
  d'émission ». Aligne l'émission avec le cap et l'absence de pre-mine.
- **−** Perte de flexibilité pour ajuster l'émission après coup (assumé : pour de la
  monnaie, **prévisibilité > flexibilité**).

## Alternatives écartées

- **Émission gouvernable** (halving/total ajustables) : rejeté — réintroduit une politique
  monétaire discrétionnaire, exactement ce que le fair launch supprime.

## Notes d'implémentation

Surtout une **décision + documentation** : s'assurer qu'aucune `GovernanceAction` ne touche
ces constantes (c'est déjà le cas), et ajouter la règle explicite au whitepaper §9. Le code
n'expose déjà aucun levier dessus — cet ADR le grave.
