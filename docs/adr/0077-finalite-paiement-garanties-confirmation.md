# ADR 0077 — Finalité de paiement : garantie et règle de confirmation

- **Statut :** Accepté 📐 — la finalité est implémentée ; la **règle exposée aux marchands** reste à écrire dans le SDK
- **Date :** Septembre 2026
- **Portée :** Produit — ce qu'un bénéficiaire doit attendre avant de considérer un paiement acquis
- **Décideur :** VinX Labs
- **Complète :** ADR 0002 (finalité prefix-closed), ADR 0031 (fork-choice), ADR 0064 (rail de paiement), ADR 0070 (authentification)
- **Crates :** `vinx-node` (`src/chain.rs`, `src/rpc/`), `sdk/`

---

## 1. Contexte

VinX est un **rail de paiement** (ADR 0064). La question centrale d'un rail de paiement
n'est pas « quel est le TPS » mais :

> **Quand un bénéficiaire peut-il livrer le bien ?**

Le protocole a la réponse — `finalized_height` avance sur le préfixe contigu de blocs
atteignant le quorum (ADR 0002) et la réorganisation sous la finalité est interdite
(ADR 0031) — mais elle n'est **exposée nulle part** comme une règle utilisable. L'API RPC
expose `finalized: bool` par bloc, sans énoncer la garantie ni ses conditions.

Sans règle explicite, chaque intégrateur invente la sienne, et les mauvaises réponses
(« j'attends 6 blocs comme Bitcoin », ou pire « la transaction est dans le mempool, c'est
bon ») produisent soit une latence inutile, soit des pertes.

## 2. Décision

### 2.1 La garantie

VinX offre une **finalité déterministe**, non probabiliste. Un bloc à hauteur `H` est final
lorsque `H <= finalized_height`, c'est-à-dire lorsque lui **et tous ses prédécesseurs** ont
réuni les co-signatures d'au moins `quorum = ⌈2n/3⌉` validateurs **enregistrés au registre
BLS** (ADR 0070).

À partir de là, et sous l'hypothèse BFT standard (moins d'un tiers du set byzantin), le bloc
est irréversible : le fork-choice ne peut pas contredire la finalité, et
`Chain::reorg_replace` refuse toute réorganisation sous `finalized_height`.

Il n'y a **pas** de « profondeur de confirmation » à choisir : la finalité n'est pas une
probabilité qui s'améliore avec le temps. Elle est atteinte ou non.

### 2.2 La règle exposée au bénéficiaire

```
Un paiement est acquis ⟺ la transaction est incluse dans un bloc à hauteur H
                          ET H <= finalized_height
```

Aucune autre condition, et surtout aucun décompte de blocs. À la cadence de 12 s (ADR 0043)
et avec un set en bonne santé, la finalité suit l'inclusion d'un ou deux blocs.

### 2.3 Les trois états à exposer

Le SDK et l'API doivent distinguer exactement trois états, sans zone grise :

| État | Signification | Action marchand |
|---|---|---|
| `pending` | Dans le mempool, pas dans un bloc | **Ne rien livrer** |
| `included` | Dans un bloc à hauteur `H > finalized_height` | **Ne rien livrer** — réorganisation encore possible |
| `final` | Dans un bloc à hauteur `H <= finalized_height` | Paiement acquis |

`included` est le piège : la transaction est visible dans un explorateur et paraît réussie.
C'est précisément l'état que le fork-choice peut défaire (ADR 0031). Le SDK doit rendre
impossible de confondre `included` et `final` — deux champs distincts, jamais un seul
booléen « confirmé ».

### 2.4 Ce que la finalité ne couvre pas

À énoncer sans détour dans la documentation d'intégration :

- **Elle suppose l'hypothèse BFT.** Au-delà d'un tiers de validateurs byzantins, la sûreté
  ne tient plus — aucun protocole BFT ne le prétend.
- **Elle est relative à la vue du nœud interrogé.** Un nœud amorcé par un snapshot
  malveillant a une `finalized_height` qui ne veut rien dire (ADR 0074). Un marchand doit
  interroger un nœud qu'il opère, ou plusieurs nœuds indépendants.
- **Elle ne dit rien du montant.** Le rail ne fournit ni séquestre, ni remboursement, ni
  réversibilité : un paiement final est définitif, y compris en cas d'erreur de destinataire.

## 3. Conséquences

- L'API RPC doit exposer, par transaction, la hauteur d'inclusion **et** l'état des trois
  ci-dessus, plutôt que de laisser l'appelant comparer lui-même.
- L'explorateur embarqué (ADR 0060) doit afficher `included` et `final` distinctement — un
  paiement affiché « confirmé » alors qu'il est réorganisable est un défaut produit.
- La règle est **plus simple** que celle d'une chaîne à finalité probabiliste. C'est un
  argument produit : à énoncer dans le whitepaper, pas seulement dans la doc technique.

## 4. Alternatives écartées

| Option | Pourquoi écartée |
|---|---|
| Recommander une profondeur en blocs (« attendre 6 ») | Faux modèle : sur une chaîne à finalité déterministe, la profondeur n'ajoute aucune garantie que la finalité ne donne déjà |
| Exposer un booléen unique `confirmed` | Écrase la distinction `included` / `final` — exactement la confusion qui coûte de l'argent |
| Laisser l'intégrateur décider | La question est protocolaire, pas applicative : y répondre est le travail du rail |

## 5. Critères de validation

- [x] `finalized_height` n'avance que sur un préfixe contigu à quorum de validateurs
      enregistrés (ADR 0002, ADR 0070).
- [x] Aucune réorganisation sous `finalized_height` — `debug_assert` dans `reorg_replace`,
      candidats purgés sous la finalité.
- [ ] L'API expose les trois états par transaction — **à coder**.
- [ ] Le SDK n'offre aucun moyen de confondre `included` et `final` — **à coder**.
- [ ] L'explorateur distingue les deux — **à coder**.
- [ ] Documentation d'intégration marchand écrite, incluant les limites §2.4 — **à écrire**.
