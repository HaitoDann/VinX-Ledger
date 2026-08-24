# ADR 0053 — Anti-replay : nonce, chain_id, expiry_height

- **Statut :** Vérifié — ✅ implémenté avant la formalisation des ADRs
- **Date :** 2025 (pré-ADR) · Formalisé août 2026
- **Portée :** Sécurité — protection contre la double-dépense et le rejeu de transactions.
- **Décideur :** VinX Labs.
- **Crate :** `crates/vinx-core` — `src/transaction.rs`

---

## 1. Contexte

Sans protection anti-replay, une transaction signée valide peut être ré-émise indéfiniment
(par un attaquant qui intercepte une tx) ou sur un autre réseau (ex. rejouer une tx de
devnet sur mainnet). VinX doit immuniser les transactions contre ces attaques.

## 2. Décision — 3 couches d'anti-replay

### 2.1 Nonce (séquence par compte)

Chaque transaction inclut un `nonce: u64` incrémental par adresse expéditeur. Le WorldState
maintient `account.nonce` — le dernier nonce accepté. Toute tx dont le nonce n'est pas
`account.nonce + 1` est rejetée.

- **Règle du mempool :** les tx à nonce futur (nonce > nonce_actuel + 1) sont acceptées dans
  le mempool mais pas appliquées — elles attendent les tx intermédiaires.
- **Garantie :** une tx ne peut être appliquée qu'une seule fois, dans l'ordre exact.

### 2.2 Chain ID (isolation de réseau)

Chaque transaction inclut `chain_id: u32`. La règle de validation rejette toute tx dont le
`chain_id` ne correspond pas au `chain_id` du nœud.

- Valeur par défaut sûre : voir ADR 0008 (pas de défaut silencieux).
- `CHAIN_ID_MAINNET = 42`, `CHAIN_ID_DEVNET = 1` (cf. PROTOCOL_SPEC.md §2).

### 2.3 Expiry height (expiration automatique)

Chaque transaction peut inclure un `expiry_height: Option<u64>`. Si défini, la tx est
invalide si `current_height > expiry_height`.

- Protège contre le stockage indéfini de tx valides dans un mempool hors-ligne.
- Le mempool appelle `prune_expired(current_height)` à chaque bloc.

### 2.4 Sponsor (protection des comptes tiers)

Une tx peut être co-signée par un `sponsor: Option<PublicKey>` qui paie les frais à la place
de l'expéditeur. La signature du sponsor est vérifiée séparément → pas de rejeu possible
(les deux nonces sont consommés).

## 3. Intégration dans les signing_bytes

Les `signing_bytes` (message signé) incluent **tous les champs anti-replay** dans cet ordre :
```
[tx_type(1o)] [from(20o)] [to(20o)] [amount(16o)] [fee(16o)] [nonce(8o)]
[chain_id(4o)] [expiry_height(9o)] [payload_len(8o)] [payload(var)]
```
Ainsi la signature couvre `chain_id`, `nonce` et `expiry_height` → impossible de modifier
ces champs après signature.

## 4. Critères de validation

- [x] Tx avec nonce dupliqué rejetée (`NonceAlreadyUsed`).
- [x] Tx avec mauvais `chain_id` rejetée.
- [x] Tx avec `expiry_height` dépassé purgée du mempool et rejetée en bloc.
- [x] Tx avec nonce futur acceptée dans le mempool mais pas appliquée immédiatement.
- [x] `cargo test --workspace` vert (vecteurs de test sur `signing_bytes`).
