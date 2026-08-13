# ADR 0040 — Émission progressive sans La Fonderie

- **Statut :** Accepté — **✅ implémenté** (`STORAGE_VERSION` 10, commit `f044bc4`).
- **Catégorie :** Tokenomics & Économie · **Priorité :** 🔴 haute (change l'invariant de base)
- **Date :** Août 2026
- **Liens :** remplace le modèle « La Fonderie » (whitepaper v4, ADR 0021 révisé) ; modifie
  l'invariant de conservation (ADR 0004 obsolète → remplacé ici) ; impacte ADR 0026
  (reaping → destruction), ADR 0028 (libellé émission), ADR 0033 (bootstrap).

---

## 1. Contexte

Le modèle actuel repose sur **La Fonderie** : 100 milliards de VINX sont créés à la genèse
et placés dans une réserve spéciale (`foundry: Amount`). Chaque bloc « forge » une fraction
de cette réserve et la verse au producteur. L'invariant est `circulation + foundry = 100 Md`.

Trois problèmes :

1. **Apparence de pre-mine.** Techniquement, les 100 Md existent dès le bloc 0. Pour un
   observateur extérieur, La Fonderie ressemble à un pre-mine scellé sous contrôle d'un
   petit groupe — même si personne n'y a accès directement. C'est un vecteur de méfiance.

2. **Le melt réintroduit de la création monétaire opaque.** Quand un validateur est slashé,
   90 % du bond est « fondu dans La Fonderie » : il rentre dans la réserve d'émission et
   sera éventuellement re-forgé vers de futurs validateurs. Ce circuit est non intuitif et
   rend l'émission dépendante du taux de slashing — une variable comportementale, pas une
   constante de protocole.

3. **Terminologie confuse.** « Forge », « melt », « Fonderie » sont des métaphores qui
   obscurcissent le mécanisme pour les nouveaux participants. Un modèle de **minting
   progressif** — les tokens n'existent pas avant d'être émis — est plus lisible et plus
   courant dans l'écosystème.

4. **Emission front-loadée.** Le paramètre actuel (`T_half` = 8 ans) produit un R₀ ≈ 8,66
   milliards/an. Les premières années captent une part disproportionnée de la supply — en
   faveur des premiers validateurs, contre l'esprit du fair launch progressif.

---

## 2. Décision

**Passer à un modèle de minting progressif** : les tokens n'existent pas avant d'être émis.
La courbe reste une **décroissance exponentielle continue** — identique mathématiquement,
mais sans pré-allocation dans une réserve.

### 2.1 Courbe d'émission (inchangée dans sa forme)

```
R(t) = R₀ · e^(−λt)       avec λ = ln(2) / T_half
émission_cumulée(t) = MAX_SUPPLY · (1 − e^(−λt))
```

La courbe est **continue et monotone décroissante** depuis le premier bloc — pas de saut
discret, pas de halving-event. Le mot « halving » disparaît du vocabulaire ; il est remplacé
par **« demi-vie »** (`T_half`) qui décrit la même propriété sans connoter un événement
calendaire.

#### Changement de paramètre : demi-vie allongée

| Paramètre | Avant (v4) | Après (v5) | Effet |
|---|---|---|---|
| `T_half` | 8 ans (252 460 800 s) | **20 ans** (630 720 000 s) | Emission initiale réduite |
| `R₀` | ≈ 8,66 Md/an | ≈ **3,47 Md/an** | Donne « moins au départ » |
| Émission année 1 | ≈ 8,01 Md | ≈ **3,35 Md** | −61 % par rapport à avant |
| 50 % supply émise | ~8 ans | **~20 ans** | Distribution beaucoup plus étalée |
| 90 % supply émise | ~27 ans | **~66 ans** | Longue traîne vers les frais seuls |
| 99 % supply émise | ~53 ans | **~133 ans** | Asymptotique vers la poussière |

> Les valeurs `T_half = 20 ans` et `R₀` sont **indicatives** — elles seront gravées dans
> la configuration de genèse (`GenesisConfig`) au moment du lancement réseau. Une fois
> inscrites dans le bloc 0 et reflétées dans le `genesis_hash`, elles sont **immuables**
> (même garantie qu'ADR 0021).

L'arithmétique reste **entière déterministe** (pas de flottant). La formule calculable est :

```
emitted_atoms(elapsed_secs) = MAX_SUPPLY_ATOMS
    − (MAX_SUPPLY_ATOMS · pow2_frac_neg(elapsed_secs, T_HALF_SECS))
```

où `pow2_frac_neg` est l'approximation entière de `2^(−t/T)` déjà utilisée.

### 2.2 Suppression de La Fonderie comme état tracké

`foundry: Amount` est **retiré** de `WorldState`. La quantité « non encore émise » est une
grandeur **dérivée**, pas un état :

```
non_emis(t) = MAX_SUPPLY_ATOMS − emitted_atoms(t)
```

Elle est calculable à la demande depuis `emitted_atoms` (déjà présent). L'API `/network/stats`
et la métrique `vinx_foundry` sont renommées : `vinx_not_yet_emitted` / `remaining_supply`.

### 2.3 Slashing : redistribution aux validateurs actifs (via le pot d'époque)

Le bond slashé ne quitte pas la circulation. Les fonds changent de main :

| Bénéficiaire | Avant (melt) | Après |
|---|---|---|
| 10 % — rapporteur | Crédité au rapporteur | **Inchangé** |
| 90 % — reste du bond | Fondu dans La Fonderie | **Versé dans `epoch_dist_emission_pot`** |

Les 90 % rejoignent le pot d'époque en cours, exactement comme l'émission. Ils sont
distribués aux proposeurs et co-signataires à la clôture de l'époque (ADR 0028) —
proportionnellement à leur participation effective. Cela :
- **Récompense les validateurs honnêtes** qui maintiennent la sécurité du réseau.
- **Ne détruit aucun jeton** — la supply totale en circulation reste constante (hors relais
  normal émission → circulation).
- **Préserve le principe « aucun burn »** de VinX.
- **Unifie le mécanisme** : slash et émission passent par le même pot, le même algorithme.

#### Comportement du reaping (ADR 0026)

Le dust des comptes reaped (solde < plancher existentiel) est **détruit** — c'est la seule
source de destruction de tokens dans VinX. Les montants sont infimes (≤ 0,001 VINX par
compte). `destroyed_atoms += dust` (pour le suivi comptable).

### 2.4 Nouvel invariant de conservation

L'invariant **ADR 0004 est remplacé** par :

```
circulating_supply + epoch_dist_emission_pot + pending_escrows_total + destroyed_atoms
    = emitted_atoms
emitted_atoms ≤ MAX_SUPPLY_ATOMS
```

où :
- `circulating_supply` = Σ balances comptes (incluant bonds stakés, unbonds en cours)
- `epoch_dist_emission_pot` = émission + slash 90 % en attente de distribution (ADR 0028)
- `pending_escrows_total` = Σ montants bloqués dans les escrows de modules (ADR 0039)
- `destroyed_atoms` = dust reaped uniquement (ADR 0026) — jamais du slash

Cet invariant est **vérifié à chaque bloc** (garde dure, même rigueur qu'avant).

> **Forme simplifiée pour la communication** : « circulation + détruits ≤ émis ≤ 100 Md »
> (le pot et les escrows sont de la circulation temporairement différée — pas une perte).

### 2.5 Comportement à la transition (migration)

Lors de l'application de cet ADR sur une chaîne existante :
- `foundry` existant → calculer `emitted_atoms = MAX_SUPPLY_ATOMS − foundry`
- Initialiser `destroyed_atoms = 0` (historique antérieur non tracké — acceptable)
- Supprimer le champ `foundry` du schéma de stockage + bump `STORAGE_VERSION`

---

## 3. Impact sur les ADR existants

| ADR | Impact |
|---|---|
| **ADR 0021** (immutabilité) | Principe maintenu — la courbe est toujours immuable après genèse. Le `T_half` change (c'est une décision de lancement, pas une modification post-genèse). Statut mis à jour. |
| **ADR 0004** (invariant) | **Remplacé** par cet ADR — le nouvel invariant `circ + destroyed = emitted ≤ MAX` supersède `circ + foundry = MAX`. |
| **ADR 0026** (reaping) | Mise à jour mineure : le dust reaped est détruit, pas fondu. |
| **ADR 0028** (époque) | Mise à jour de libellé : « forgé depuis la Fonderie » → « nouvellement émis ». |
| **ADR 0033** (bootstrap) | Partiellement obsolète (voir §4). |
| **ADR 0001** (vision) | Mise à jour des références à La Fonderie. |

---

## 4. Conséquences

**Positif**
- **Fin de l'apparence de pre-mine** : à la genèse, `emitted_atoms = 0`, `circulating = 0`.
  Il n'y a rien — les tokens n'existent pas encore. Beaucoup plus propre à communiquer.
- **Principe « aucun burn » préservé** : le slash redistribue vers les validateurs honnêtes
  via le pot d'époque — aucun token ne disparaît, ils changent de main. Seul le dust de
  reaping est détruit (infimes montants, par construction ≤ 0,001 VINX par compte reapé).
- **Slash renforce l'incitation à valider** : les 90 % du bond slashé vont aux validateurs
  actifs — récompense collective pour maintenir un set sûr.
- **Émission plus équitable** : avec `T_half = 20 ans`, les premiers validateurs gagnent
  beaucoup moins que dans le modèle 8 ans, réduisant la concentration early.
- **Modèle plus lisible** : minting progressif, un seul concept (`emitted_atoms`), une
  métrique simple (`remaining_supply = MAX − emitted`).
- **Unification du pot d'époque** : émission + slash 90 % passent par le même pot et le
  même algorithme de distribution — cohérence maximale, aucune logique dupliquée.

**Coûts / pièges**
- **Changement consensus-critique** : touche `WorldState`, `settle_block`, l'invariant et le
  slashing → bump `STORAGE_VERSION` obligatoire, migration déterministe.
- **ADR 0021 renégocié** : le `T_half` change avant le lancement. L'esprit (immuable après
  genèse) est respecté ; la lettre (8 ans inscrit) est abandonnée. À documenter clairement.
- **Réduction des gains early** : les premiers validateurs gagnent moins. Pour un réseau
  cherchant à attirer des participants, cela peut ralentir le bootstrapping initial.
  Compensé par l'Open PoA (ADR 0038) qui élargit le set très tôt.
- **`destroyed_atoms` croît indéfiniment** : champ monotone ; borne naturelle = MAX_SUPPLY.

---

## 5. Alternatives écartées

- **Garder La Fonderie avec le melt** : rejeté — les deux problèmes (apparence pre-mine,
  émission dépendante du slashing) persistent.
- **Garder La Fonderie, supprimer le melt** (slash → epoch pot mais foundry reste) :
  partiellement adressé mais l'invariant reste `circ + foundry = MAX` — La Fonderie est
  toujours un pre-mine visible. Rejeté.
- **Slash vers destruction** (90 % détruits) : contredit le principe « aucun burn » de VinX
  et prive les validateurs honnêtes d'une récompense juste. Rejeté au profit de la
  redistribution via epoch pot.
- **Changer la forme de la courbe** (non-exponentielle) : rejeté — l'exponentielle est
  l'unique fonction ayant la propriété `intégrale = MAX` avec un paramètre simple et une
  arithmétique entière raisonnable. Une courbe quadratique ou hypergéométrique serait plus
  difficile à auditer et expliquer.
- **Courbe à démarrage bas puis pic** (log-normale) : rejeté — crée une phase de hausse
  d'émission (entre 0 et le pic) qui est contre-intuitive pour un réseau de paiement
  et difficile à justifier économiquement.
- **Garder `T_half = 8 ans`** : rejeté — le front-loading concentre 50 % de la supply en
  8 ans, amplifiée par la petitesse du set initial → contredit le fair launch progressif.

---

## 6. Notes d'implémentation

- `vinx-core` :
  - `EMISSION_T_HALF_SECS: u64` remplace `HALVING_PERIOD_SECS` (même rôle, terminologie corrigée).
  - `cumulative_emission_atoms(elapsed_secs)` : formule inchangée, constante renommée.
- `vinx-state` / `WorldState` :
  - Supprimer `foundry: Amount`.
  - Ajouter `destroyed_atoms: u128` (suivi comptable des destructions).
  - `emit_work_reward` : plus de débit `foundry`, juste `emitted_atoms +=` et `circulating +=`.
  - `apply_slash` : 10 % → rapporteur (inchangé) ; 90 % → `destroyed_atoms +=` + `circulating −=`.
  - `apply_reaping` (ADR 0026) : dust → `destroyed_atoms +=` + `circulating −=` (plus de `foundry +=`).
  - `check_invariant` : vérifier `circulating + destroyed == emitted ≤ MAX_SUPPLY`.
- `vinx-node/rpc` :
  - `/network/stats` : `foundry` → `remaining_supply` (`MAX_SUPPLY − emitted`), ajouter `destroyed_atoms`.
  - Métriques : `vinx_foundry` → `vinx_remaining_supply` + nouvelle `vinx_destroyed_atoms`.
- Bump `STORAGE_VERSION` : suppression `foundry`, ajout `destroyed_atoms` → migration append.
- Tests : invariant post-slash, invariant post-reaping, `emitted + remaining = MAX`, migration
  depuis un état avec `foundry` existant, `T_half` = 20 ans en constantes de test.
