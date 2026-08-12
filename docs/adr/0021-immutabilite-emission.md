# ADR 0021 — Immutabilité de la courbe d'émission

- **Statut :** Accepté — révisé août 2026 (paramètre `T_half` mis à jour par ADR 0040 avant lancement)
- **Catégorie :** Cohérence · **Priorité :** 🟠 moyenne
- **Date :** Juillet 2026 — révisé Août 2026
- **Lié :** fair launch, whitepaper §3 & §9 (règles immuables) ; ADR 0040 (émission progressive).

## Contexte

La politique monétaire de VinX repose sur une **courbe d'émission exponentielle continue** :
`R(t) = R₀ · e^(−λt)` avec `λ = ln(2) / T_HALF_SECS` et `R₀ = MAX_SUPPLY × λ`.
Son **statut de gouvernance** doit être établi : peut-elle être modifiée par l'admin,
par un upgrade, ou jamais ?

Une politique monétaire qu'une clé admin pourrait changer **contredit** la promesse de fair
launch : « personne ne décide de la création monétaire ». C'est exactement ce que le projet
reproche aux modèles à robinet discrétionnaire.

## Décision

Déclarer la **courbe d'émission immuable après la genèse** :

- Le total (`MAX_SUPPLY = 100 Md`), la **demi-vie** (`T_HALF_SECS`) et la **forme
  exponentielle** de la courbe **ne sont pas gouvernables** — ni par l'admin, ni par une
  `GovernanceAction`.
- Leur seule voie de changement serait un **upgrade majeur** avec préavis complet — et
  l'esprit de cet ADR est que **cela n'arrive jamais après la genèse**.
- Ces paramètres sont inscrits dans la **configuration de genèse** (`GenesisConfig`),
  reflétés dans le `genesis_hash` canonique, et vérifiables par tout nœud.
- Cette règle est inscrite dans les **« Règles immuables »** du whitepaper (§9), aux côtés
  du cap et de l'absence de pre-mine.

### Note sur la révision d'août 2026 (ADR 0040)

ADR 0040 a modifié le paramètre `T_HALF_SECS` avant le lancement réseau :
- Ancienne valeur indicative : 8 ans (252 460 800 s)
- Nouvelle valeur indicative : **20 ans** (630 720 000 s)

Cette modification est effectuée **avant la genèse** — elle ne constitue pas une violation
de l'immuabilité (qui s'applique après le bloc 0). L'esprit de cet ADR est pleinement respecté :
une fois le réseau lancé, aucun mécanisme ne permettra de modifier la courbe.

## Conséquences

- **+** Garantie de confiance forte : « nul, pas même le fondateur, ne change le calendrier
  d'émission après la genèse ». Aligne l'émission avec le cap et l'absence de pre-mine.
- **+** Paramètre gravé dans le `genesis_hash` → tout nœud peut vérifier qu'il est sur la
  bonne chaîne.
- **−** Perte de flexibilité pour ajuster l'émission après coup (assumé : pour de la monnaie,
  **prévisibilité > flexibilité**).

## Alternatives écartées

- **Émission gouvernable** (`T_half`/total ajustables) : rejeté — réintroduit une politique
  monétaire discrétionnaire, exactement ce que le fair launch supprime.

## Notes d'implémentation

- `HALVING_PERIOD_SECS` renommé en `EMISSION_T_HALF_SECS` (ADR 0040) — même rôle,
  terminologie clarifiée (pas d'event discret, juste la demi-vie de la courbe continue).
- S'assurer qu'aucune `GovernanceAction` ne touche ces constantes (déjà le cas).
- Ajouter la règle explicite au whitepaper §9 et au `GenesisConfig` canonique.
