# ADR 0057 — Mempool — file d'attente de transactions

- **Statut :** Vérifié — ✅ implémenté avant la formalisation des ADRs
- **Date :** 2025 (pré-ADR) · Formalisé août 2026
- **Portée :** Nœud — gestion des transactions en attente de confirmation.
- **Décideur :** VinX Labs.
- **Crate :** `crates/vinx-node` — `src/mempool.rs`

---

## 1. Contexte

Les transactions soumises via l'API RPC ou reçues via P2P ne sont pas immédiatement incluses
dans un bloc — elles doivent attendre dans une file (mempool) jusqu'à ce qu'un leader les
sélectionne. La conception du mempool a un impact direct sur :
- Le débit (combien de tx le leader peut extraire par bloc).
- La résistance au spam.
- La gestion des nonces (les tx hors-séquence arrivent souvent avant les tx précédentes).

## 2. Décision

### 2.1 Structure

```rust
Mempool {
    pending:  BTreeMap<(Address, u64), Transaction>,  // (from, nonce) → tx
    staged:   Vec<Transaction>,                        // tx prêtes à aller dans un bloc
    max_size: usize,                                   // MAX_MEMPOOL_SIZE = 100_000
}
```

**Indexé par `(from, nonce)`** → ordre déterministe par adresse et nonce.

### 2.2 Règles d'admission (`add`)

1. La tx est d'abord pré-validée (signature, chain_id, fee >= BASE_FEE).
2. `account.balance >= admission_cost_atoms` (solde estimé en tenant compte des tx déjà en attente).
3. Si `mempool.size() >= MAX_MEMPOOL_SIZE` : rejet (`MempoolFull`).
4. Les tx avec `expiry_height` dépassé sont rejetées immédiatement.

### 2.3 Ordonnancement

- `drain(limit)` : extrait au maximum `limit` (= `MAX_BLOCK_TXS = 3000`) tx pour un bloc.
- Sélection en priorité des nonces contigus depuis le nonce de compte actuel.
- Les tx à nonce futur (gap) restent dans le mempool mais ne sont pas extraites.

### 2.4 Nettoyage

- `update_confirmed_nonces(confirmed)` : après validation d'un bloc, supprime les tx dont
  le nonce est ≤ au nonce confirmé de chaque adresse.
- `prune_expired(height)` : supprime les tx dont `expiry_height < height`.
- `requeue(txs)` : remet des tx dans le mempool après une réorganisation (ADR 0031).

### 2.5 Tx « staged »

Mécanisme d'anti-récursion : une tx reçue du réseau P2P est d'abord `staged` (file tampon),
puis `flush_staged()` les intègre dans le mempool principal après le traitement du bloc.
Évite une mutation du mempool pendant qu'on itère dessus pour construire un bloc.

## 3. Critères de validation

- [x] `MAX_MEMPOOL_SIZE = 100_000` respecté (tx supplémentaires rejetées).
- [x] Les tx à nonce futur attendent sans bloquer les tx à nonce séquentiel d'autres adresses.
- [x] `prune_expired` retire les tx expirées avant la production du prochain bloc.
- [x] `requeue` après réorg remet les tx non-confirmées dans le bon ordre.
- [x] `cargo test --workspace` vert (tests unitaires complets dans `mempool.rs`).

## 4. Conséquences

- **Positif :** `BTreeMap<(Address, nonce)>` → ordre déterministe, debug facile.
- **Positif :** `admission_cost_atoms` prend en compte les tx déjà en attente → pas de
  double-dépense dans le mempool.
- **Compromis :** pas de priorisation par fee (toutes les tx au-dessus du plancher sont égales).
  Une amélioration future pourrait trier par fee décroissant pour les périodes de congestion
  (ADR 0035).
