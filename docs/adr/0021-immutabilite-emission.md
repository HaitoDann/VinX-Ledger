# ADR 0021 — Immutabilité de la courbe d'émission

- **Statut :** ✅ Implémenté (halving) — **amendé par l'ADR 0040** : l'objet immuable devient la
  loi élastique `E = r·F`, plus le halving discret. Porté en code dans la tranche 0040.
- **Catégorie :** Cohérence · **Priorité :** 🟠 moyenne
- **Date :** Juillet 2026 · **Amendé :** Août 2026 (ADR 0040)
- **Lié :** fair launch, whitepaper §3 & §9 (règles immuables) ; **émission élastique (ADR 0040)**.

> **Amendement (ADR 0040).** Ce que cet ADR grave comme immuable **n'est plus** la courbe à
> halving discret (8 ans, total fixe), mais la **loi d'émission élastique à réservoir**
> `E = r · F` avec `r = 7 %/an` (ADR 0040). Le principe est inchangé — *la politique monétaire
> n'est pas gouvernable* — seul son *contenu* change : on grave une **loi** (débit ∝ Fonderie),
> pas une **courbe** figée. Le reste de cet ADR (immutabilité, cap, pas de pre-mine) tient tel
> quel ; lire « courbe d'émission » ci-dessous comme « loi d'émission `E = r·F` ».

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
