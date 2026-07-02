# VinX Ledger

**Cash numérique artisanal — L1 de paiement souverain, rapide et précis au centime.**

---

## Qu'est-ce que VinX Ledger ?

VinX Ledger est une blockchain L1 conçue exclusivement pour les paiements du quotidien. Pas de smart contracts, pas de spéculation — une seule promesse : envoyer de l'argent vite, pas cher, et sans intermédiaire opaque.

Sa monnaie suit le modèle **La Fonderie** : une supply fixe de 100 milliards, **sans burn**, où le métal *fond* (frais) et se *reforge* (récompenses) à l'infini.

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
| **Supply** | 100 milliards VINX (immuable, sans burn) |
| **Modèle** | La Fonderie — melt/forge, invariant `circulation + Fonderie = 100 Md` |

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

## Tokenomics — La Fonderie

- **100 milliards VINX**, supply fixe et **immuable** — forgée une fois à la genèse.
- **Genèse** : 1 Md (1 %) au fondateur pour amorcer · 99 Md (99 %) dans **La Fonderie**.
- **Melt** : 100 % des frais fondent dans La Fonderie (ce n'est **pas** un burn).
- **Forge** : les récompenses de staking sont forgées depuis La Fonderie (une fraction à chaque distribution → la réserve ne se vide jamais).
- **Invariant** vérifié à chaque bloc : `circulation + Fonderie = 100 000 000 000 VINX`.
- **Aucun burn** · Aucune inflation · Cap immuable.

> Détails complets : [whitepaper.md](./whitepaper.md)

---

## Roadmap

- **Actuel** — protocole complet (L1 Rust, PoA Threshold, Fonderie), exploité en local.
- **Ensuite** — redondance 1 → 3 validateurs, distribution du milliard fondateur, micro-économie réelle.
- **Plus tard (optionnel)** — réseau public, *token factory* (émission d'autres actifs sur VinX).

---

*VinX Labs — juillet 2026*
