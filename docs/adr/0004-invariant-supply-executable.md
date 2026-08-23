# ADR 0004 — Invariant de supply exécutable

- **Statut :** Remplacé par ADR 0040 (Émission progressive sans La Fonderie) — ✅
- **Date :** Juillet 2026
- **Portée :** Tokenomics — invariant de conservation de la supply, garde dure.
- **Décideur :** VinX Labs.

---

## 1. Contexte

L'invariant de supply (`circulating + foundry = MAX_SUPPLY`) n'était pas vérifié
**déterministiquement** à chaque bloc — un bug dans `settle_block` pouvait créer ou
détruire des tokens sans être détecté.

## 2. Décision (historique — remplacée)

Appliquer `supply_invariant_holds()` comme **garde dure** sur tous les chemins de bloc
(production, P2P, sync). Si l'invariant est violé, la node panique (`unreachable!`) plutôt
que de propager un état invalide.

Formule originale : `circulating + foundry = MAX_SUPPLY`.

**Remplacée par ADR 0040 :** La Fonderie (`foundry`) disparaît. Le nouvel invariant est :

```
circulating + epoch_pot + destroyed = emitted ≤ MAX_SUPPLY
```

- `emitted` : tokens réellement émis via la courbe exponentielle.
- `epoch_pot` : tokens émis en attente de distribution aux validateurs.
- `destroyed` : tokens détruits par le reaping de comptes poussière.
- La garde dure est **maintenue** ; seule la formule change.

## 3. Voir ADR 0040

Tout le détail de l'implémentation actuelle est dans
[ADR 0040](./0040-emission-progressive-sans-fonderie.md).
