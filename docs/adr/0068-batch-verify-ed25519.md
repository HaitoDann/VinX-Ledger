# ADR 0068 — Batch verify Ed25519

- **Statut :** Décidé
- **Date :** Septembre 2026
- **Portée :** Performance — vérification de signatures
- **Décideur :** VinX Labs
- **Crates :** `crates/vinx-node` — `src/p2p/mod.rs`
- **Liens :** ADR 0067 (sig cache — s'applique aux sigs non cachées ; batch verify traite ce sous-ensemble)

---

## 1. Contexte

La vérification Ed25519 signature par signature n'exploite pas la structure algébrique de la courbe de Edwards. La bibliothèque `ed25519-dalek` (déjà utilisée dans le projet) expose `verify_batch()` qui vérifie N signatures en un seul passage.

**Pourquoi c'est plus rapide :** la vérification individuelle effectue deux multiplications scalaires par signature. La vérification batch utilise un algorithme de Bos-Coster/Pippenger qui réduit le coût à O(n / log n) multiplications pour n signatures simultanées — soit ~2x pour des batches de 10+.

Référence code : `crates/vinx-node/src/p2p/mod.rs:461` — point de vérification actuel.

## 2. Décision

Grouper les signatures à vérifier (celles non couvertes par le sig cache ADR 0067) en un seul appel `ed25519_dalek::verify_batch()` lors de la validation de bloc.

### 2.1 Implémentation

```rust
use ed25519_dalek::verify_batch;

// Collecte les tx dont la sig n'est pas dans le sig_cache
let to_verify: Vec<_> = block.transactions.iter()
    .filter(|tx| !mempool.sig_cache_contains(&tx.hash()))
    .collect();

if !to_verify.is_empty() {
    let messages: Vec<&[u8]> = to_verify.iter().map(|tx| tx.signing_bytes()).collect();
    let signatures: Vec<_> = to_verify.iter().map(|tx| tx.signature()).collect();
    let pubkeys: Vec<_>    = to_verify.iter().map(|tx| tx.public_key()).collect();

    match verify_batch(&messages, &signatures, &pubkeys) {
        Ok(()) => {} // toutes valides
        Err(_) => {
            // fallback individuel pour identifier la tx fautive
            for tx in &to_verify {
                tx.verify_signature()
                    .map_err(|_| BlockError::InvalidSignature(tx.hash()))?;
            }
        }
    }
}
```

### 2.2 Fallback en cas d'échec de batch

`verify_batch()` retourne une erreur si *une quelconque* signature est invalide, sans identifier laquelle. En cas d'échec, le code repasse en mode individuel pour identifier la transaction fautive et produire un message d'erreur exploitable (pour le ban du pair, les logs, les métriques).

Ce fallback n'a de coût que lorsqu'un bloc contient une signature invalide — cas anormal qui entraîne de toute façon le rejet du bloc.

## 3. Compatibilité avec ed25519-dalek

`verify_batch` est disponible dans `ed25519-dalek` avec la feature `batch` (activée par défaut depuis la v2.0). La crate est déjà dans l'arbre de dépendances — aucune nouvelle dépendance.

```toml
# Vérifier que la feature batch est active
ed25519-dalek = { version = "2", features = ["batch"] }
```

## 4. Conséquences

**Positif**
- ~2x de gain sur la vérification des signatures non-cachées (le sous-ensemble qui échappe à ADR 0067).
- Combiné avec ADR 0067, en charge normale (90%+ des tx dans le mempool local), le gain net est limité à quelques tx — mais ces quelques tx sont vérifiées 2x plus vite.
- Zéro impact protocole ou wire format.
- Zéro nouvelle dépendance.

**Coûts / compromis**
- Légère complexité : collecte des structures avant le batch, fallback individuel.
- Le batch nécessite que toutes les données (messages, signatures, clés publiques) soient en mémoire simultanément — négligeable pour un bloc de ≤3000 tx.

## 5. Critères de validation

- [ ] Un bloc valide avec 100 tx est validé sans appel à `verify_individual()` (batch seul).
- [ ] Un bloc avec une signature invalide déclenche le fallback et identifie la tx fautive.
- [ ] `cargo test --workspace` vert.
- [ ] Benchmark `cargo bench` : validation de bloc 2x plus rapide qu'avant sur le chemin de vérification.
