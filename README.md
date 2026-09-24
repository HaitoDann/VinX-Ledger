<p align="center">
  <img src="./assets/vinx-logo.png" alt="VinX" width="540" />
</p>

<h1 align="center">VinX Ledger</h1>

<p align="center"><strong>Rail de paiement L1 PoS — rapide, souverain, sans intermédiaire.</strong></p>

<p align="center">Une chaîne de paiement minimaliste : finalité déterministe au quorum, consensus BFT Tendermint, émission fair-launch. Rien d'autre.</p>

---

## Vision

VinX fait **une seule chose, et la fait bien : transférer de la valeur.** Pas de machine virtuelle, pas de smart contracts, pas de modules applicatifs — un rail de paiement L1 en Proof-of-Stake, volontairement minimaliste (ADR 0064). Sur le L1 :

1. **Arithmétique de solde** — transferts, comptes, bonds de validateurs
2. **Consensus BFT** — finalité déterministe au quorum, un bloc, aucune réorganisation sous finalité
3. **Émission fair-launch** — minting progressif par le travail, supply immuable

Pas d'EVM. Pas de WASM. Pas de logique applicative sur L1. La surface d'attaque minimale **est** la feature.

> **Recentrage v6.0 (ADR 0064) :** les subnets / modules / Appchains ZK des versions antérieures sont **gelés hors scope**. VinX n'est plus un settlement layer ZK — c'est un rail de paiement.

---

## Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│  Clients (wallets, PSP, intégrations paiement)                    │
│  → signent des transactions Ed25519 → soumettent au L1            │
└────────────────────────────┬─────────────────────────────────────┘
                             │ Transaction (Transfer / Stake / …)
                             ▼
┌──────────────────────────────────────────────────────────────────┐
│  VinX L1 — rail de paiement PoS                                   │
│  • Comptes VINX · transferts · bond/slashing · frais forfaitaires │
│  • Consensus : BFT Tendermint, ≤ 100 validateurs, BLS agrégé      │
│  • Finalité immédiate (> 2/3 de la puissance, 1 bloc)             │
│  • Émission progressive fair-launch (T_half ~20 ans)              │
│  • Hachage BLAKE3 · adresses Bech32m vinx1 · anti-replay complet   │
└──────────────────────────────────────────────────────────────────┘
```

---

## Consensus — BFT par étapes (Tendermint, ADR 0082)

| Propriété | Valeur |
|-----------|--------|
| **Étapes** | Proposition → prevote → precommit, avec verrou (lock) |
| **Finalité** | Immédiate : un bloc commité (> 2/3 de la puissance) n'est jamais réorganisé |
| **Leader** | Rotation pondérée par le stake (priorités Tendermint), validateurs jailed sautés |
| **Votes** | Pondérés par le stake, plafonnés à 10 % par validateur |
| **Validateurs** | Jusqu'à 100 actifs, sélection automatique par le bond |
| **Signatures** | BLS12-381 agrégé — certificat de commit = 1 agrégat + bitmap |
| **Temps de bloc** | Paramètre de genèse (12 s au départ), dérive d'horloge tolérée 15 s |

> Tolérance aux pannes : moins d'1/3 de la puissance. À n=3 les 3 validateurs sont requis ;
> une panne est tolérée à partir de n=4. Sans quorum, la chaîne s'arrête au lieu de diverger.

---

## Démarrage rapide

**Prérequis :** Rust stable (≥ 1.80) — [rustup.rs](https://rustup.rs)

```bash
git clone https://github.com/HaitoDann/VinX-Ledger.git
cd VinX-Ledger

# Lancer les tests
cargo test --workspace

# Démarrer un nœud local (devnet)
cargo run -p vinx-node

# Dans un second terminal — créer un wallet
cargo run -p vinx-wallet -- keygen --output mon-wallet.json

# Vérifier son solde
cargo run -p vinx-wallet -- balance <VOTRE_ADRESSE>
```

Le nœud expose **http://localhost:8545** :
- **`/`** — explorateur de blocs + wallet web
- **`/admin`** — console d'administration

> Guide complet : [GUIDE.md](./GUIDE.md)

---

## Tokenomics — Fair launch

- **1 milliard de VINX**, supply fixe et immuable, **9 décimales** (ADR 0081).
- **Aucun pre-mine, aucune réserve.** Les premiers VINX n'existent qu'au moment où le premier bloc est produit.
- **Émission par le travail** : minting progressif, décroissance exponentielle continue, demi-vie ~20 ans.
- **Validateurs rémunérés par l'émission + les frais, sans burn** : le producteur reçoit 20 % de l'émission et 50 % des frais de son bloc ; le reste va aux co-signataires au prorata de leur participation (ADR 0028, ADR 0081).
- **Gouvernance à la Linux** : la clé admin on-chain s'éteint 365 jours après le premier bloc ; ensuite, le mainteneur dirige le logiciel et les validateurs choisissent la version qu'ils exécutent (ADR 0081).
- **Invariant** : `circulation + pot_époque + détruits = émis ≤ 1 Md` — garanti à chaque bloc.

> Whitepaper : [whitepaper.md](./whitepaper.md)

---

## Roadmap

| Phase | Période | Contenu |
|-------|---------|---------|
| ✅ **Phase 1 — Fondations** | Terminé | L1 complet (consensus PoA Threshold → base PoS), finalité BFT, fair launch, BLS12-381, jailing, fork-choice, P2P anti-DoS, gouvernance K-of-M |
| 🔄 **Phase 1.5 — Durcissement & lancement** | En cours | Série post-audit (ADRs 0069–0080) : BLAKE3, auth proposeur, verrou de vote, `consensus_root` complet, sérialisation injective, checkpoints, enrôlement BLS au bonding, banc adversarial |
| 🔄 **Phase 2 — PoS Algorand** | Q4 2026 | Comité VRF (ECVRF RFC 9381), sélection par VRF, récompenses par époque (ADR 0029/0038/0028) |
| 🔭 **Phase 3 — Réseau public** | 2027+ | Mainnet, décentralisation à l'échelle, light client, post-quantique |
| ❄️ **Gelé hors scope (ADR 0064)** | — | Appchains ZK / SP1 (0050), ForceExit (0048), Clearinghouse (0049), Celestia DA (0034), modules bondés (0010/0024) — abandonnés au profit du rail de paiement pur |

> Index complet des décisions d'architecture : [docs/adr/README.md](./docs/adr/README.md)

---

## Structure du projet

```
crates/
├── vinx-crypto/        Ed25519, BLS12-381, adresses Bech32, BLAKE3, Merkle
├── vinx-core/          Amount, Account, Transaction, Block
├── vinx-state/         WorldState, genesis, apply_transaction, émission
├── vinx-node/          Nœud, mempool, RPC HTTP, producteur de blocs, persistance
├── vinx-wallet/        CLI wallet
└── vinx-desktop-core/  Logique wallet partagée (keystore, signature)
apps/
└── vinx-desktop/       Application desktop Tauri (wallet + console admin)
sdk/
└── vinx-sdk/           SDK TypeScript
docs/
└── adr/                Décisions d'architecture (ADR 0001 → 0081)
```

---

*VinX Labs — août 2026*
