# ADR 0065 — Allocateur global mimalloc

- **Statut :** Décidé
- **Date :** Septembre 2026
- **Portée :** Performance système — allocation mémoire
- **Décideur :** VinX Labs
- **Crates :** `crates/vinx-node` — `Cargo.toml`, `src/main.rs`

---

## 1. Contexte

Le runtime Rust utilise l'allocateur système par défaut (glibc malloc sur Linux). Les workloads serveur avec de nombreuses petites allocations courtes (transactions, messages P2P, blocs) souffrent de la fragmentation de glibc malloc et de sa contention sur les accès multi-threadés.

Le nœud VinX est un serveur multi-threadé (runtime Tokio) avec un profil d'allocation typique : beaucoup de petits objets (txs ~200 octets, hashes 32 octets, messages P2P) avec une durée de vie courte.

## 2. Décision

Remplacer l'allocateur système par **mimalloc** (Microsoft Research) via `#[global_allocator]` dans `main.rs`.

```toml
# Cargo.toml
[dependencies]
mimalloc = { version = "0.1", default-features = false }
```

```rust
// src/main.rs
use mimalloc::MiMalloc;
#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;
```

## 3. Pourquoi mimalloc

| Critère | mimalloc | jemalloc | tcmalloc |
|---------|----------|----------|----------|
| Gain documenté (workloads serveur) | 10-20% | 10-15% | 5-15% |
| Crate Rust disponible | ✅ `mimalloc` | ✅ `jemallocator` | ✅ `tcmalloc` |
| Dépendance C externe | Non (header-only) | Oui (libjemalloc) | Oui (libtcmalloc) |
| Profil allocation petits objets | Excellent | Très bon | Bon |
| Portabilité (Linux + Mac + Windows) | ✅ | Linux/Mac | Linux |

mimalloc est retenu pour sa portabilité (Windows utile pour les développeurs testnet) et l'absence de dépendance C externe à compiler séparément.

## 4. Conséquences

**Positif**
- 10-20% de gain en throughput sur les workloads serveur Rust, documenté par Microsoft Research et confirmé par des benchmarks communautaires (TiKV, Databend).
- Zéro changement applicatif — le changement est transparent pour le code métier.
- Zéro impact sur le protocole, le wire format ou le stockage.
- 4 lignes de code au total.

**Coûts / compromis**
- Ajoute une dépendance `mimalloc` au `Cargo.toml`.
- Le comportement de l'allocateur est différent en cas d'OOM — les messages d'erreur peuvent différer de glibc.

## 5. Critères de validation

- [ ] `cargo build --release` réussit.
- [ ] Le nœud démarre et produit des blocs normalement.
- [ ] Aucune régression sur `cargo test --workspace`.
