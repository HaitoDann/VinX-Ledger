# ADR 0031 — Règle de fork-choice

- **Statut :** Proposé
- **Catégorie :** Consensus & finalité · **Priorité :** 🔴 haute (complétude de sûreté du
  consensus)
- **Date :** Juillet 2026
- **Liens :** complète la finalité (ADR 0002) ; interagit avec le slot-skip/jailing (ADR 0027)
  et l'accountability (ADR 0030).

## Contexte

VinX a une **finalité** (`finalized_height`, prefix-closed, jamais réorganisée en dessous —
ADR 0002) mais **aucune règle explicite** pour choisir entre plusieurs blocs valides
concurrents **au-dessus** de la finalité. Aujourd'hui un nœud accepte ce qui **étend son tip**
(vérif de `prev_hash`). Cela laisse un angle mort :

- Le round-robin est *advisory* : sur slot-skip, un **backup** produit. Rien n'empêche le
  **leader prévu ET le backup** de produire chacun un bloc valide à la même hauteur → **deux
  forks valides** avant finalité.
- Deux blocs concurrents peuvent chacun rassembler des co-signatures partielles (< quorum).
  Sans règle **déterministe** de sélection, deux nœuds honnêtes peuvent suivre des branches
  différentes transitoirement, et l'ordre d'arrivée réseau décide — c'est fragile et
  non-spécifié.

Un fork-choice **déterministe** est une brique de sûreté manquante : il faut que **tous les
nœuds honnêtes convergent sur la même branche canonique** à partir des mêmes faits, avant même
la finalité.

## Décision proposée

Définir une **fonction de fork-choice totale et déterministe** `canonical(candidats) → tête`,
évaluée sur des faits on-chain observables identiquement (donc pas d'ordre d'arrivée réseau,
pas d'horloge locale).

### Règle (par ordre lexicographique de priorité)

À hauteur contestée, entre blocs valides `B` étendant le préfixe finalisé :

1. **Finalité d'abord (sûreté absolue).** Ne jamais choisir une branche qui contredit un bloc
   **finalisé**. `finalized_height` est un plancher dur : réorg interdite en dessous.
2. **Plus haute finalité justifiée.** Préférer la branche dont le **`finalized_height` est le
   plus élevé** (elle porte le plus de sécurité acquise).
3. **Poids de co-signatures.** À finalité égale, préférer le bloc contesté portant le **plus de
   co-signatures valides** (le plus proche du quorum → le plus « soutenu » par le set).
4. **Priorité au leader prévu.** À poids égal, préférer le bloc du **leader round-robin prévu**
   (`leader_at(H)` sur le set actif) plutôt que celui d'un backup — un backup ne devient
   canonique que si le leader prévu est absent (cohérent avec le slot-skip de l'ADR 0027).
5. **Départage déterministe.** En dernier recours, le **plus petit hash d'en-tête** (ordre
   total, identique partout).

### Bornes de réorganisation

- **Profondeur de réorg bornée** par la finalité : aucune réorg ne peut descendre sous
  `finalized_height`. Une réorg au-dessus est autorisée **uniquement** si la règle ci-dessus
  élit une meilleure branche **et** qu'aucun bloc finalisé n'est contredit.
- **Refus de bâtir dans le vide (ADR 0002 « reste »)** : un producteur ne bâtit pas au-delà
  d'une profondeur non finalisée trop grande — borne la longueur des forks concurrents et la
  fenêtre où le fork-choice opère.

### Interaction avec le slot-skip (ADR 0027)

Le slot-skip et le fork-choice doivent être **cohérents** : la règle #4 encode que le bloc du
leader prévu l'emporte, donc un backup qui produit *pendant que le leader produit aussi* perd
le fork-choice — le backup n'est utile que quand le leader est **réellement absent**. Cela
évite qu'un backup trop pressé ne fracture le réseau, tout en préservant la liveness.

## Modèle

`canonical` est une fonction **pure** des blocs et de leurs co-signatures observés (aucune
entrée subjective) → deux nœuds avec la même vue calculent la même tête ; avec des vues
partielles, ils **convergent** dès que le gossip complète la vue (les faits sont monotones :
plus de co-signatures, finalité plus haute). C'est la propriété clé.

## Conséquences

**Positif**
- **Convergence déterministe** des nœuds honnêtes avant finalité → plus de dépendance à
  l'ordre d'arrivée réseau.
- Résout proprement la **collision leader/backup** du slot-skip.
- Complète la sûreté : 0030 *punit* la double-finalité, 0031 *l'évite* en amont en canonisant
  une seule branche.

**Coûts / pièges**
- Consensus-critique : la fonction doit être **spécifiée formellement** et testée au **banc
  multi-nœuds** (partitions, réordonnancements, leader+backup simultanés).
- La règle #3 (poids de co-signatures) doit se garder du **grief** : un adversaire qui retient
  puis relâche des co-signatures pourrait provoquer des micro-réorgs. La borne de réorg (sous
  finalité) et la priorité au leader prévu (#4) limitent l'impact ; à valider.
- Doit rester **cohérente** avec la sélection VRF future (ADR 0029 phase 2) — le « leader
  prévu » y devient « le leader élu par VRF ».

## Alternatives écartées

- **Longest-chain (Nakamoto)** : rejeté — inadapté à une finalité BFT ; la longueur n'est pas
  le bon signal quand on a des co-signatures et une finalité explicite.
- **Premier-vu gagne (implicite actuel)** : rejeté — non déterministe (dépend de l'ordre
  réseau) → divergence transitoire.
- **Heaviest par bond des signataires** : écarté — pondérer par le stake contredit
  l'égalitarisme VinX (ADR 0028) ; le **nombre** de co-signatures est le signal, pas leur poids.

## Notes d'implémentation

- `vinx-node/chain` + `consensus` : implémenter `canonical()` comme fonction pure ordonnée ;
  l'appeler à chaque réception de bloc/co-signature concurrent(e) ; borne de réorg = plancher
  `finalized_height`.
- Tests (banc n≥2/n≥3) : leader+backup simultanés → une seule tête ; convergence après vue
  partielle ; refus de réorg sous finalité ; départage par hash stable ; cohérence avec
  slot-skip.
- Dépendance : `finalized_height` (ADR 0002) ; s'aligne avec 0027 (leader actif) et 0030
  (punition si la sûreté est quand même violée).
