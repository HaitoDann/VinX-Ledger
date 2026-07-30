# ADR 0015 — Exécution parallèle & vérification parallèle

- **Statut :** Accepté (tranche 1 implémentée — vérification parallèle ; exécution d'état **différée**)
- **Catégorie :** Données, état & scaling · **Priorité :** 🟢 future (pour la tranche 2)
- **Date :** Juillet 2026

## Contexte

« Paralléliser pour scaler » recouvre **deux** choses très différentes, avec des profils
de risque/bénéfice opposés. Les confondre est le piège classique.

1. **Vérification parallèle des signatures** (validation). À la réception d'un bloc, un
   nœud doit vérifier une signature Ed25519 par transaction. C'est le **coût CPU dominant**
   de la validation d'un bloc, c'est **purement fonction des octets de la tx** (aucun accès
   à l'état), et c'est une simple **conjonction booléenne** (toutes doivent passer).
2. **Exécution parallèle de l'état** (façon Solana *Sealevel* / Aptos *Block-STM*). Exécuter
   les **transitions d'état** (mutations de solde) sur plusieurs cœurs à la fois. Cela exige
   une détection de conflits d'accès (quelles tx touchent des comptes disjoints), un
   ordonnanceur, et des garanties de déterminisme strictes.

Le code appliquait jusqu'ici **tout séquentiellement** : `for tx in bloc { apply_transaction(tx) }`,
où `apply_transaction` fait, en ligne et une tx à la fois : garde replay/TTL → **vérif de
signature** → mutation d'état. La vérif de signature — la partie chère — était donc sur le
chemin critique séquentiel de la validation de bloc.

## Décision

### Tranche 1 — Vérification parallèle des signatures ✅ (implémentée)

Sur **tous les chemins de validation de bloc reçu** (P2P `NewBlock`, `SyncResponse`, sync au
démarrage), vérifier **toutes** les signatures de transactions **en parallèle** (rayon
`par_iter`) **en amont**, puis appliquer l'état **séquentiellement** via
`apply_transaction_trusted` (qui saute la re-vérification, garde replay/TTL).

- Ajout de `WorldState::verify_tx_signature_pure(tx)` — vérification **pure, sans `&self`**,
  donc partageable entre threads sans risque.
- Driver `verify_block_tx_signatures_parallel(txs) -> bool` côté `vinx-node`, à côté du
  `verify_block_signatures_parallel` **déjà existant** pour les co-signatures de validateurs
  (même pattern).

**Pourquoi c'est sûr et déterministe.** La vérification ne lit ni ne modifie l'état ; c'est
un `all()` de tests indépendants → le résultat (accepter/rejeter le bloc) est **identique
quel que soit l'ordre** d'exécution des vérifications. Aucun risque de divergence
inter-nœuds. L'**ordre d'exécution de l'état reste strictement séquentiel** — on parallélise
la *validation*, pas l'*exécution*.

**Bénéfice.** Le coût CPU dominant de la validation passe de 1 cœur à *N* cœurs. Gain direct
sur la vitesse de **sync initiale** et sur la capacité d'un suiveur à valider des blocs
pleins — les vrais points de douleur avant même d'avoir un très haut TPS.

### Tranche 2 — Exécution parallèle de l'état ⏸️ (différée, ce ADR l'encadre)

**Non implémentée, et volontairement.** On la documente pour qu'elle soit décidée sur pièces
le jour où elle devient pertinente, pas adoptée par enthousiasme.

Design cible (le jour venu) : modèle **Block-STM optimiste** (préféré à Sealevel, qui impose
de déclarer ses accès à l'avance) —
- exécuter les tx de manière optimiste en parallèle, en journalisant les lectures/écritures ;
- détecter les conflits (deux tx qui touchent le même compte) ;
- **rejouer** déterministiquement les tx en conflit dans l'ordre canonique du bloc ;
- le `state_root` final doit être **identique** à celui de l'exécution séquentielle.

## Conséquences

**Tranche 1 (faite)**
- **+** Scalabilité immédiate de la validation, zéro risque consensus, additif (rien d'autre
  ne change), réutilise rayon déjà présent.
- **−** Aucun inconvénient de correction. (À très bas volume, le coût de dispatch rayon est
  négligeable ; `par_iter` bascule de toute façon en séquentiel pour de petits lots.)

**Tranche 2 (différée)**
- **+** Débit d'exécution multi-cœurs — mais **seulement** utile à des **dizaines de milliers
  de TPS soutenus**, un régime que VinX n'atteint pas encore et ne peut pas mesurer sans banc.
- **−** **Consensus-critique et à haut risque.** Un ordonnanceur non déterministe ou une
  détection de conflit incomplète produit des `state_root` divergents entre nœuds → fork.
  C'est exactement la catégorie de complexité que le projet garde hors du cœur tant qu'elle
  n'est pas *nécessaire* et *mesurable*.

## Alternatives écartées

- **Tout paralléliser maintenant (exécution comprise)** : rejeté. Optimisation prématurée sur
  le terme le plus risqué du système, sans métrique pour la valider (pas de banc 3-validateurs
  à haut TPS) et sans besoin établi. On prend le gain sûr (tranche 1), on cadre le reste.
- **Ne rien paralléliser** : rejeté — la vérif de signature est un gain *gratuit* et sûr, il
  serait absurde de le laisser sur la table.
- **Sealevel (accès déclarés à l'avance)** : écarté pour la tranche 2 au profit de Block-STM
  optimiste, qui ne demande pas aux clients de déclarer leurs accès (incompatible avec des tx
  de paiement simples).

## Critères de déclenchement de la tranche 2

Instruire l'exécution parallèle de l'état **uniquement** quand **tous** les points suivants
sont vrais :

1. Un **banc 3-validateurs** existe et sait générer une charge soutenue réaliste.
2. Le **profilage** montre que l'application séquentielle de l'état (hors signatures, déjà
   parallélisées) est le goulot dominant.
3. Le TPS visé dépasse durablement ce qu'un cœur peut appliquer.

Tant que ces trois conditions ne sont pas réunies, la tranche 1 suffit et la tranche 2 reste
un risque net.

## Notes d'implémentation (tranche 1)

- `crates/vinx-state/src/world_state.rs` : `verify_tx_signature_pure` (pub, sans `&self`) ;
  `verify_tx_signatures(&self)` y délègue.
- `crates/vinx-node/src/p2p/mod.rs` : `verify_block_tx_signatures_parallel` ; branché sur
  `NewBlock` et `SyncResponse`, suivi d'`apply_transaction_trusted`.
- `crates/vinx-node/src/sync.rs` : vérif parallèle par bloc avant l'application trusted.
