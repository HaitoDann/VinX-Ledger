# ADR 0010 — Primitive d'ancrage & registre de modules

- **Statut :** Accepté — ✅ tranche 1 implémentée (registre + ancrage bondé) ; adjudication de
  fraude différée (→ ADR 0023)
- **Catégorie :** Modules (par-dessus l'ADR 0001) · **Priorité :** 🟠 moyenne
- **Date :** Juillet 2026
- **Liens :** première tranche concrète de l'ADR 0001 (L1 monnaie pure + surcouches par
  ancrage bondé) ; anti-bloat cf. ADR 0026 ; sérialisation canonique cf. ADR 0020.

## Contexte

L'ADR 0001 pose le principe : VinX reste une **L1 monnaie pure** et les fonctionnalités
avancées vivent dans des **modules hors-chaîne** qui s'**ancrent** sur la L1 via un **bond**
et des **commitments** (racines de Merkle). Jusqu'ici, rien de cet ancrage n'existait dans le
code — l'ADR 0001 était du design non implémenté.

## Décision — tranche 1 : registre + primitive d'ancrage

Introduire le **strict minimum** pour qu'un opérateur puisse enregistrer un module bondé et y
publier des ancres — **sans jamais exécuter la logique du module sur la L1**.

### Un seul type de tx : `AnchorState` (0x09)

Appendé au bout de `TransactionType` (discriminants existants inchangés ; octet de signature
`0x09`). Son `payload` porte un `bincode(ModuleOp)` :

```rust
enum ModuleOp {
    Register   { module_id: Hash32, bond_atoms: u128 }, // verrouille un bond
    Anchor     { module_id: Hash32, anchor_head: Hash32 }, // avance la racine engagée
    Deregister { module_id: Hash32 },                    // rend le bond
}
```

Regrouper les trois opérations sous un seul type de tx (plutôt que trois discriminants) garde
l'espace des types propre et réutilise le payload générique — conforme à « réutilise bond +
Merkle + payload générique ».

### Registre d'état

`WorldState.modules: BTreeMap<Hash32, ModuleEntry>` avec
`ModuleEntry { operator, bond, anchor_head, anchored_count }`. Champ **appendé en dernier**
(après les champs 0011), donc un blob pré-0010 (v8) est un préfixe strict d'un blob v9 : la
migration `STORAGE_VERSION 8 → 9` **append** l'encodage par défaut (map vide). Le snapshot
JSON le porte nativement (`serde(default)`).

### Sémantique (validation avant mutation)

- **Register** : `bond ≥ MIN_MODULE_BOND_ATOMS` (1 000 VinX), `module_id` libre, registre non
  plein (`MAX_MODULES`). Le bond + le frais sont débités du solde de l'opérateur ; l'opérateur
  doit **rester un compte vivant** (`solde ≥ ED`, ADR 0026) — il devra exister pour ancrer.
- **Anchor** : **opérateur uniquement** ; met à jour `anchor_head`, incrémente
  `anchored_count`. La L1 **ne vérifie pas** le contenu de l'ancre (elle ne connaît pas la
  logique du module) — juste le droit de l'opérateur et le paiement du frais.
- **Deregister** : **opérateur uniquement** ; **rend le bond** au solde et retire l'entrée.

Chaque opération paie le **frais de base** (même forfait anti-spam qu'un transfert), crédité
au producteur au règlement du bloc. Chaque arm valide **avant** de muter → une op rejetée ne
consomme pas de nonce (le producteur saute une tx échouée sans rollback, cf. ADR 0026).

### Invariants

- **Neutralité de circulation** : un bond est **verrouillé, pas détruit** (l'opérateur le
  possède toujours) ; `circulating_supply` est inchangé, l'invariant de masse (ADR 0004)
  tient. Vérifié en test.
- **Anti-bloat** : `MIN_MODULE_BOND_ATOMS` rend l'enregistrement massif économiquement absurde
  (comme le dépôt existentiel pour les comptes) ; `MAX_MODULES` borne la taille du registre.
  Les opérateurs gardent un compte ≥ ED (jamais poussière, jamais reapé).

## Conséquences

**Positif**
- Première brique exécutable de l'ADR 0001 : des surcouches peuvent s'ancrer, la L1 reste
  pure (aucune exécution de logique de module).
- Additif et borné ; réutilise le modèle de bond et le forfait de frais existants.

**Coûts / limites**
- Changement de format d'état → bump de version + migration (fait, in-place append).
- **Pas d'adjudication de fraude** en tranche 1 : le bond est verrouillé mais aucun mécanisme
  ne le **slash** en cas de fraude d'opérateur — c'est tout l'objet de l'**ADR 0023** (preuves
  de fraude → réputation → zk). Ici, `Deregister` rend toujours le bond intégralement.
- **Pas de délai de déliaison** sur le bond de module (contrairement au bond de validateur) :
  un opérateur peut enregistrer/désenregistrer librement. Suffisant tant qu'il n'y a pas de
  slashing à sécuriser (tranche 2).
- **Pas de commande CLI dédiée** encore : `AnchorState` se construit via le SDK/`new_anchor_state`
  ; un sous-commande wallet est un ajout non-consensus (suite).

## Tranche 2 (différée)

- **Adjudication du slashing de module (ADR 0023)** : prouver une fraude d'opérateur et
  slasher son bond (bond → réputation → preuves de fraude → zk).
- **Délai de déliaison** du bond (fenêtre pendant laquelle il reste slashable), façon bond de
  validateur (ADR 0009).
- **Métadonnées de module** (type, endpoint, DA layer) et découverte.
- **Commande wallet** `anchor` / `register-module` / `deregister-module`.

## Alternatives écartées

- **Trois types de tx dédiés** (RegisterModule/Anchor/Deregister) : rejeté — gonfle l'espace
  des discriminants ; l'enum `ModuleOp` dans un seul `AnchorState` est plus propre et
  extensible.
- **Exécuter la logique de module sur la L1** : rejeté par principe (ADR 0001) — la L1 reste
  monnaie pure ; elle n'ancre que des commitments bondés.
- **Enregistrement gratuit** : rejeté — vecteur de bloat ; le bond minimum est l'anti-spam.

## Notes d'implémentation

- `crates/vinx-core/src/module.rs` : `ModuleOp` ; `amount.rs` :
  `MIN_MODULE_BOND_ATOMS`/`MAX_MODULES` ; `transaction.rs` : `TransactionType::AnchorState`
  (0x09), `new_anchor_state`.
- `crates/vinx-state/src/world_state.rs` : `ModuleEntry`, champ `modules`,
  `apply_anchor_state`, `v9_meta_suffix`. Tests : verrou du bond + neutralité de circulation,
  rejet bond faible/doublon, ancrage opérateur-only, deregister rend le bond, solde
  insuffisant.
- `crates/vinx-node/src/storage.rs` : `STORAGE_VERSION 9` + migration append v8→v9
  (factorisée dans `append_meta_suffix`).
