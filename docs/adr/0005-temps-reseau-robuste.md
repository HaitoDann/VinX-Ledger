# ADR 0005 — Temps réseau robuste

- **Statut :** ✅ Implémenté (à éprouver au banc n≥2)
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

État de l'implémentation :

1. **Bornes à la réception** — faites sur les deux chemins : gossip P2P (`p2p/mod.rs`)
   et sync HTTP de démarrage (`sync.rs`) : monotonie stricte vs le tip + plafond
   `horloge locale + MAX_CLOCK_DRIFT_SECS` (120 s).
2. **MTP comme horloge protocole** — fait : `Chain::median_time_past_with(next_ts)`
   calcule la médiane des `MEDIAN_TIME_BLOCKS` derniers timestamps *incluant le bloc
   appliqué*. Les trois chemins (production, application P2P, replay sync) passent ce
   MTP à `set_block_context` et `settle_block` : émission, déliaison de bond et
   activation d'upgrade comparent à la médiane — un producteur seul ne déplace le temps
   protocole que d'un cran de médiane, jamais d'un saut. Déterministe : chaque nœud le
   calcule du même préfixe de chaîne + header.
3. **Plafond d'incrément par bloc** — volontairement **non retenu** : la cadence est
   à la demande (une chaîne au repos ne produit pas), donc un long écart entre deux
   blocs est *légitime* et l'émission intégrée sur les timestamps doit le rattraper.
   Le plafond `horloge locale + dérive` à la réception borne déjà l'avance possible
   par rapport au temps réel, ce qui est la garantie recherchée.

Reste : éprouver au banc n≥2 (l'exploit ne se manifeste qu'à plusieurs validateurs).
