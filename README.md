# VinX Ledger

**Cash numérique artisanal — L1 de paiement souverain, rapide et précis au centime.**

---

## Qu'est-ce que VinX Ledger ?

VinX Ledger est une blockchain L1 conçue exclusivement pour les paiements du quotidien. Pas de smart contracts, pas de spéculation — une seule promesse : envoyer de l'argent vite, pas cher, et sans intermédiaire opaque.

Implémenté intégralement en Rust, sans framework blockchain tiers.

---

## Caractéristiques clés

| | |
|---|---|
| **Consensus** | PoA Threshold — >66% des validateurs co-signent chaque bloc |
| **Finalité** | Déterministe et immédiate — zéro réorganisation possible |
| **Bloc** | 10 secondes (3s devnet) |
| **TPS** | ~4 000 |
| **Frais** | 0,05% · plancher 0,0001 VINX |
| **Cryptographie** | Ed25519 · SHA-256 · Bech32 (`vinx1`) |
| **Supply** | 100 milliards VINX (immuable) |
| **Phase 1** | 21M VINX Sandbox active · 99,979Md Coffre Maturité verrouillé |

---

## Démarrage rapide

```bash
git clone https://github.com/HaitoDann/VinX-Ledger.git
cd VinX-Ledger

# Lancer les tests
cargo test --workspace

# Démarrer le nœud devnet
cargo run -p vinx-node

# Dans un second terminal — créer un wallet
cargo run -p vinx-wallet -- keygen --output my-wallet.json

# Vérifier son solde
cargo run -p vinx-wallet -- balance <VOTRE_ADRESSE>
```

Le nœud expose une interface web sur **http://localhost:8545** et sauvegarde automatiquement l'état sur disque (`devnet/`).

> Guide complet : [GUIDE.md](./GUIDE.md)

---

## Structure du projet

```
crates/
├── vinx-crypto/   Ed25519, adresses Bech32, SHA-256
├── vinx-core/     Amount, Account, Transaction, Block
├── vinx-state/    WorldState, genesis, apply_transaction
├── vinx-node/     Nœud, mempool, RPC HTTP, producteur de blocs, persistance
└── vinx-wallet/   CLI wallet
```

---

## Tokenomics

- **21 000 000 VINX** — Sandbox Phase 1, distribués aux contributeurs
- **99 979 000 000 VINX** — Coffre Maturité, verrouillé jusqu'à 3 conditions cumulatives (statut CASP MiCA · audit indépendant · politique de distribution publique)
- **Frais** : 80% au pool de staking · 20% à la Treasury VinX Labs
- **Aucun burn** · Aucune inflation · Cap immuable

> Détails complets : [whitepaper.md](./whitepaper.md)

---

## Roadmap

- **Phase 1** — MVP devnet, Sandbox active *(en cours)*
- **Phase 2** — Testnet public, multi-validateurs PoA, audit
- **Phase 3** — Mainnet, déverrouillage Coffre Maturité sous conditions strictes

---

*VinX Labs — juin 2026*
