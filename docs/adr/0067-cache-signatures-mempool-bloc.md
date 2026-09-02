# ADR 0067 — Cache de signatures mempool → validation de bloc

- **Statut :** Décidé
- **Date :** Septembre 2026
- **Portée :** Performance — chemin critique de validation de bloc
- **Décideur :** VinX Labs
- **Crates :** `crates/vinx-node` — `src/mempool.rs`, handler de validation de bloc
- **Liens :** ADR 0066 (hash mémoïsé — clé du cache) ; ADR 0068 (batch verify — s'applique aux sigs non cachées)

---

## 1. Contexte

Quand un bloc arrive, le nœud re-vérifie la signature Ed25519 de **chaque** transaction. Or, si la transaction était déjà dans le mempool local, sa signature a été vérifiée à l'admission. Cette double vérification est redondante en charge normale.

Référence code : `crates/vinx-node/src/p2p/mod.rs:460-461` — validation des signatures à la réception d'un bloc.

En charge normale (réseau sain, propagation gossip efficace), la quasi-totalité des transactions d'un bloc sont déjà dans le mempool local avant que le bloc arrive. Le gain est donc de 5-10x sur ce chemin en conditions réelles.

Ce mécanisme est identique à l'optimisation `CScriptCheck` cache de Bitcoin Core — une pratique établie dans les implémentations blockchain production.

## 2. Décision

Ajouter un `sig_cache: HashSet<[u8; 32]>` au `Mempool`, indexé par hash de transaction.

### 2.1 Structure

```rust
pub struct Mempool {
    // ... champs existants ...
    sig_cache: HashSet<[u8; 32]>, // tx hashes dont la sig est vérifiée
}
```

### 2.2 Remplissage du cache

À l'admission d'une transaction dans le mempool (après vérification réussie de la signature) :

```rust
// mempool.rs — fn try_insert()
if self.verify_signature(&tx)? {
    let h = tx.hash();
    self.txs.insert(h, tx);
    self.sig_cache.insert(h); // ← sig vérifiée, cache l'info
}
```

### 2.3 Utilisation à la validation de bloc

```rust
// validation de bloc — pour chaque tx du bloc
for tx in &block.transactions {
    let h = tx.hash();
    if mempool.sig_cache_contains(&h) {
        // sig déjà vérifiée à l'admission mempool → skip
        continue;
    }
    // tx hors mempool local → vérification normale
    verify_signature(tx)?;
}
```

### 2.4 Éviction du cache

Le sig_cache suit le cycle de vie du mempool :
- **Expulsion normale** : quand une tx est retirée du mempool (confirmée dans un bloc ou expirée), son hash est retiré du sig_cache dans la même opération.
- **Flush complet** : si le mempool est vidé (restart, fork), le sig_cache est vidé avec lui.

```rust
pub fn remove(&mut self, tx_hash: &[u8; 32]) {
    self.txs.remove(tx_hash);
    self.sig_cache.remove(tx_hash); // sync
}
```

## 3. Invariant de sécurité

Le cache ne dispense de vérification que si **la transaction est toujours présente dans le mempool local**. La présence dans le mempool est la preuve que la signature a été vérifiée localement.

Il n'existe pas de chemin par lequel une tx puisse être dans `sig_cache` sans avoir passé `verify_signature` : le seul point d'insertion du cache est après une vérification réussie (§2.2).

**Contre les tx forgées :** un attaquant qui forgerait une tx avec un hash identique mais une signature invalide ne peut pas insérer cette tx dans le mempool local (la vérification d'admission la rejette). Donc la présence dans le sig_cache implique bien que *cette* tx a une signature valide.

## 4. Conséquences

**Positif**
- 5-10x de gain sur le chemin de validation de bloc en charge normale.
- Zéro impact protocole ou wire format.
- `HashSet<[u8;32]>` : overhead mémoire de ~40 octets par tx en vol (négligeable).

**Coûts / compromis**
- Légère complexité dans `Mempool::remove()` : deux collections à synchroniser.
- En réseau dégradé (compact blocks avec nombreuses tx manquantes), le gain est réduit — mais la vérification normale reste le fallback.

## 5. Critères de validation

- [ ] Un bloc dont toutes les tx sont dans le mempool passe la validation sans aucun appel à `verify_signature` pour ces tx.
- [ ] Une tx avec signature invalide ne peut pas être insérée dans le sig_cache (test unitaire).
- [ ] Après confirmation d'un bloc, les tx confirmées sont absentes du sig_cache.
- [ ] `cargo test --workspace` vert.
