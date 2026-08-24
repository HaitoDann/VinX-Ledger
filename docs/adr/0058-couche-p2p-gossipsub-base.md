# ADR 0058 — Couche P2P de base (libp2p + gossipsub)

- **Statut :** Vérifié — ✅ implémenté avant la formalisation des ADRs
- **Date :** 2025 (pré-ADR) · Formalisé août 2026
- **Portée :** Réseau — transport des blocs, transactions et co-signatures entre nœuds.
- **Décideur :** VinX Labs.
- **Crate :** `crates/vinx-node` — `src/p2p/`

---

## 1. Contexte

VinX est un réseau décentralisé — les nœuds doivent se découvrir, propager les blocs et les
transactions, et échanger les co-signatures. La couche réseau doit être fiable, résistante
aux attaques de type DoS (ADR 0022), et découplée du consensus.

## 2. Décision — libp2p + gossipsub

**Pourquoi libp2p ?**
- Bibliothèque réseau P2P mûre, utilisée par Ethereum 2, Filecoin, Polkadot.
- Supporte plusieurs transports (TCP, QUIC) et protocoles (gossipsub, kademlia, identify).
- Implémentation Rust (`rust-libp2p`) activement maintenue, compatible async/tokio.

**Pourquoi gossipsub ?**
- Propagation de messages en O(log N) sur le réseau (mesh gossip).
- Natif à libp2p, avec scoring de pairs (anti-spam).
- Adapté aux blocs et tx (many-to-many broadcast).

### 2.1 Topics gossipsub

| Topic | Contenu | Émetteur |
|---|---|---|
| `vinx/blocks/v1` | `NewBlock { block }` | Proposeur |
| `vinx/txs/v1` | `NewTransaction { tx }` | Tous |
| `vinx/cosigs/v1` | `BlockSignature { height, sig }` | Validateurs |
| `vinx/blscosigs/v1` | `BlsCosig { height, bls_sig, pubkey }` | Validateurs (ADR 0046) |

### 2.2 Types de messages P2P

```rust
enum P2pCommand {
    BroadcastBlock(Block),
    BroadcastTx(Transaction),
    BroadcastSignature(u64, BlockSignature),
    BroadcastBlsCosig(u64, BlsSignature, BlsPublicKey),
    Shutdown,
}
```

**Handle (thread-safe) :** `P2pHandle` — canal `mpsc` vers le daemon P2P, utilisé par le
consensus et le RPC pour diffuser.

### 2.3 Compression

Les messages bloc sont compressés avec **zstd** (ADR 0022 : borne de décompression à 16 Mio
pour éviter les bombes de décompression).

### 2.4 Sécurité de base

- Borne `max_transmit_size` sur les messages P2P.
- Rate-limiting par pair (`p2p::guard`) — token bucket (ADR 0022).
- Voir ADR 0022 pour le détail du durcissement.

## 3. Critères de validation

- [x] 3 nœuds se connectent, propagent des blocs et co-signatures (banc n=3).
- [x] Un bloc produit par le nœud A arrive chez B et C en < 2 s (réseau local).
- [x] Les messages trop grands sont rejetés (borne `max_transmit_size`).
- [x] `cargo test --workspace` vert.

## 4. Conséquences

- **Positif :** libp2p gère la découverte de pairs (mDNS + DHT Kademlia en option).
- **Positif :** gossipsub supporte nativement le scoring de pairs (base pour ADR 0022 tranche 2).
- **Compromis :** gossipsub a une latence de quelques secondes sur grand réseau — acceptable
  pour un block time de 12 s (ADR 0043).
- **Compromis :** la couche P2P est actuellement sur le même thread tokio que le consensus
  → à découpler si la charge P2P impacte la latence de consensus à grand N.
