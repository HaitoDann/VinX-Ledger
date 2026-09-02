# ADR 0066 — Hash de transaction mémoïsé

- **Statut :** Décidé
- **Date :** Septembre 2026
- **Portée :** Performance — hot path validation/compact blocks
- **Décideur :** VinX Labs
- **Crates :** `crates/vinx-core` — `src/transaction.rs`

---

## 1. Contexte

Le hash d'une transaction est recalculé à chaque point de contact :

1. **Admission mempool** — déduplication et indexation.
2. **Construction de compact block** — le producteur émet les hashes de tx au lieu des tx complètes (ADR 0037).
3. **Validation de bloc** — vérification que les hashes correspondent aux tx reçues.
4. **Réponse TxRequest** — lookup dans `recent_block_txs` par hash.
5. **Sig cache** (ADR 0067) — clé d'index du cache.

Une transaction est immuable après création. Recalculer son hash est du travail inutile.

Référence : `crates/vinx-core/src/transaction.rs:174` — `fn hash(&self) -> [u8; 32]`.

## 2. Décision

Ajouter un champ `cached_hash` à la struct `Transaction`, calculé au premier appel à `hash()` et retourné depuis le cache ensuite.

```rust
#[derive(Clone, Debug, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Transaction {
    // ... champs existants ...

    /// Hash calculé à la demande, non sérialisé.
    #[serde(skip)]
    #[borsh(skip)]
    cached_hash: std::sync::OnceLock<[u8; 32]>,
}

impl Transaction {
    pub fn hash(&self) -> [u8; 32] {
        *self.cached_hash.get_or_init(|| {
            // calcul existant inchangé
            let mut hasher = blake3::Hasher::new(); // ou SHA-256 selon ADR 0069
            hasher.update(&borsh::to_vec(self).expect("tx borsh"));
            hasher.finalize().into()
        })
    }
}
```

**`OnceLock`** (stable depuis Rust 1.70) est thread-safe sans `Mutex`. L'initialisation est garantie exactement une fois.

## 3. Pourquoi OnceLock et non Option<[u8;32]> + Mutex

`OnceLock<T>` est conçu exactement pour ce cas : initialisation paresseuse thread-safe sans overhead d'un `Mutex` après la première initialisation. Un `Option<[u8;32]>` nécessiterait soit un `Mutex` (overhead), soit `unsafe` (incorrect).

## 4. Impact wire format

`#[serde(skip)]` + `#[borsh(skip)]` : le champ est **absent du wire format et du stockage**. Aucun impact sur la sérialisation, le protocole, ou la compatibilité avec les nœuds existants.

La désérialisation d'une `Transaction` depuis le réseau ou le disque produit un `cached_hash` vide — il sera calculé au premier appel à `hash()`. C'est le comportement attendu.

## 5. Conséquences

**Positif**
- Élimine les recalculs répétés dans les hot paths (compact blocks, sig cache, validation).
- Zéro impact protocole.
- Compatible avec `Clone` — `OnceLock` est clonable : le clone hérite du hash déjà calculé si disponible.

**Coûts / compromis**
- `Transaction` n'est plus `Copy` si elle l'était (elle ne l'est probablement pas déjà, vu ses `Vec<u8>`).
- Ajoute 8-16 octets de mémoire par transaction en vol (taille d'un `OnceLock<[u8;32]>`).
- Implémentation de `PartialEq` / `Hash` manuels si `Transaction` les dérive : exclure `cached_hash` de la comparaison (déjà implicite via `#[serde(skip)]` si les derives ignorent ce champ, à vérifier).

## 6. Critères de validation

- [ ] `cargo test --workspace` vert.
- [ ] `transaction.hash()` appelé 1000x sur la même instance = 1 seul calcul (vérifié avec un compteur de test).
- [ ] Sérialisation/désérialisation roundtrip identique avant/après.
