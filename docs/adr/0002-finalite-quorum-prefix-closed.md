# ADR 0002 — Finalité au quorum prefix-closed

- **Statut :** Vérifié — ✅ tranche 1 implémentée et validée (banc n=3)
- **Date :** Juillet 2026 · Implémenté août 2026
- **Portée :** Sûreté BFT — finalité explicite prefix-closed, refus de bâtir dans le vide.
- **Décideur :** VinX Labs.

---

## 1. Contexte

Le producteur scelle un bloc avec sa **seule** signature. Le quorum (⌈2n/3⌉ co-signatures)
n'est vérifié qu'*a posteriori*, via des messages P2P séparés. Résultat : le `finalized_height`
n'existait pas — n'importe quel bloc scellé était traité comme « final » par les nœuds.

Problèmes :
- Deux proposeurs honnêtes peuvent sceller deux blocs différents à la même hauteur → fork.
- Sans plancher explicite inréorganisable, le fork-choice (ADR 0031) n'a pas d'ancre.
- Un leader peut bâtir une longue chaîne sans attendre les co-signatures → divergence potentielle.

## 2. Options envisagées

| Option | Description | Compromis |
|---|---|---|
| A — Finalité synchrone (2-phase commit) | Le proposeur attend les ⌈2n/3⌉ co-signatures avant de publier | Latence haute ; complique le timeout leader |
| B — Finalité asynchrone prefix-closed ✅ | Le proposeur publie immédiatement ; la finalité avance sur le **préfixe contigu** de blocs ayant reçu le quorum de co-signatures | Latence optimale (1 aller-retour réseau) ; fault-tolerant |
| C — Pas de finalité explicite | Le fork-choice "longest-chain" seul | Réorganisations profondes possibles ; pas de BFT |

## 3. Décision

**Option B — Finalité asynchrone prefix-closed.**

- Le `finalized_height` avance sur le **préfixe contigu** de blocs ayant reçu ≥ ⌈2n/3⌉
  co-signatures valides. Mis à jour à la production et à chaque réception de co-signature.
- Exposé sur `/health`.
- À n=1 la finalité est **immédiate** (le proposeur seul forme le quorum).
- **Refus de bâtir dans le vide** : le leader **et** le backup refusent de sceller au-delà de
  `MAX_UNFINALIZED_DEPTH = 64` blocs non-finalisés au-dessus de `finalized_height`.
  - À n=1 : jamais déclenché.
  - À n≥2 : mord seulement si la finalité est réellement bloquée (partition réseau).
  - Borne les forks concurrents et la fenêtre du fork-choice (ADR 0031).

## 4. Critères de validation

- [x] À n=1 : `finalized_height` suit `height` immédiatement après chaque bloc.
- [x] À n=3 : `finalized_height` avance après réception de 2 co-signatures valides.
- [x] À n=3 : si 1 nœud tombe, la finalité **gèle** mais ne bifurque pas.
- [x] `MAX_UNFINALIZED_DEPTH = 64` bloqué et testé.
- [x] `/health` expose `finalized_height`.
- [x] Banc n=3 : convergence indépendante de l'ordre d'arrivée vérifiée.

## 5. Règle BFT importante (découverte au banc n=3)

Le **quorum de finalité reste sur le set complet bondé** (`⌈2n/3⌉`), **jamais** réduit par
le jailing (ADR 0027). La table `reliability` est dérivée/subjective sous partition — la réduire
laisserait une minorité (ex. split 1│2) jailer la majorité dans sa vue, tomber à quorum 1, et
finaliser une branche rivale (double-finalité).

Seule la gouvernance (`RemoveValidator`, committée et finalisée) réduit `n`.

## 6. Reste à implémenter

- **Tranche 2** : view-change formel (remplacement du leader quand MAX_UNFINALIZED_DEPTH est atteint).
- Persistance du schedule quorum (aujourd'hui reconstruit à l'application → après reload, repli
  sur le quorum courant pour le petit suffixe non-finalisé).
- Latence de finalité mesurée = 1 aller-retour réseau (objectif à confirmer au banc réseau réel).
