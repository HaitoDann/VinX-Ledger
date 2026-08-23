# ADR 0056 — Bond & queue de déliaison (mécanisme de staking)

- **Statut :** Vérifié — ✅ implémenté avant la formalisation des ADRs
- **Date :** 2025 (pré-ADR) · Formalisé août 2026
- **Portée :** Économie & sécurité — mécanisme de mise en gage et de retrait des validateurs.
- **Décideur :** VinX Labs.
- **Crate :** `crates/vinx-state` — `src/world_state.rs` ; `crates/vinx-core` — `src/validator_pool.rs`

---

## 1. Contexte

Le bond (mise en gage) est le mécanisme économique central de sécurité de VinX. Un
validateur qui stake des VINX a quelque chose à perdre en cas de comportement malveillant
(slashing). La sortie doit être **retardée** (unbonding) pour empêcher les attaques
« stake, attaque, retire ».

## 2. Décision

### 2.1 Stake (entrée)

La tx `Stake (0x02)` :
1. Déduit `amount` du solde `from`.
2. Ajoute `amount` à `account.staked`.
3. **Ne modifie pas encore le set de validateurs** — l'admission dans le set actif est une
   opération de gouvernance séparée (ou automatique via ADR 0038 pour Open PoS).

**Contraintes :**
- `MIN_BOND_HARD_FLOOR = 10_000 VINX` (immuable — ne peut pas être réduit par gouvernance).
- `MAX_BOND_HARD_CAP = 100_000_000 VINX` (immuable — anti-capture par un seul acteur).
- `MIN_BOND_DEFAULT = 100_000 VINX` (gouvernable dans `[FLOOR, CAP]`).

### 2.2 Unstake (sortie — unbonding queue)

La tx `Unstake (0x03)` :
1. Déduit `amount` de `account.staked`.
2. Crée une entrée dans la **queue de déliaison** : `UnbondEntry { amount, release_height }`.
3. `release_height = current_height + UNBONDING_HEIGHT` où
   `UNBONDING_HEIGHT = UNBONDING_SECS / BLOCK_TIME_SECS = 259200 / 12 = 21_600 blocs` (3 jours).
4. À `current_height >= release_height`, les fonds sont libérés vers `account.balance`.

**Pourquoi 3 jours ?**
- Suffisant pour détecter et punir une attaque BFT (ADR 0003, ADR 0030) avant que l'attaquant
  ne récupère son bond.
- Court assez pour ne pas décourager les validateurs honnêtes de sortir.

### 2.3 Slashing du bond

Un slash (ADR 0003) réduit `account.staked` directement. Si la tx de slash arrive **pendant**
la période d'unbonding, le fonds en cours de déliaison peut être slashé partiellement.

**Plafond d'unbonds par compte (ADR 0009)** : anti-spam — un compte ne peut avoir qu'un
nombre limité d'entrées de déliaison simultanées.

## 3. États d'un compte validateur

```
balance    → VINX disponibles (transferables, pour les frais)
staked     → VINX en bond actif (slashable)
unbonding  → [(amount, release_height)] — en cours de déliaison
```

**Invariant :** `balance + staked + sum(unbonding) = total_vinx_owned_by_account`

**Exemption du dépôt existentiel :** un compte avec `staked > 0` n'est jamais reapé (ADR 0026),
même si son `balance == 0`.

## 4. Critères de validation

- [x] Stake : `staked` augmente, `balance` diminue de même montant.
- [x] Unstake : `staked` diminue, une entrée d'unbonding est créée avec la bonne `release_height`.
- [x] Les fonds unbondés sont libérés exactement à `release_height` (et pas avant).
- [x] Un slash pendant l'unbonding réduit le montant à libérer.
- [x] Le plafond d'unbonds simultanés est respecté.
- [x] `cargo test --workspace` vert.

## 5. Conséquences

- **Positif :** fenêtre de slashing garantie (3 jours > temps de détection d'équivocation).
- **Positif :** les fonds en unbonding sont toujours slashables → pas de fuite instantanée.
- **Compromis :** 3 jours de liquidité réduite pour les validateurs honnêtes.
- **Compromis :** la complexité de la queue de déliaison augmente la taille du WorldState
  (mais chaque entrée est bornée par le plafond d'unbonds par compte).
