<p align="center">
  <img src="./assets/vinx-logo.png" alt="VinX" width="540" />
</p>

<h1 align="center">VinX Ledger</h1>

<p align="center"><strong>Settlement layer ZK-natif — rapide, souverain, sans intermédiaire.</strong></p>

<p align="center">L1 de paiement avec finalité déterministe, consensus Algorand-style et écosystème d'Appchains ZK.</p>

---

## Vision

VinX est ce qu'Ethereum essaie de devenir : un **settlement layer ZK-natif**, sans la dette technique de l'EVM. Il fait exactement trois choses sur le L1 :

1. **Arithmétique de solde** — transferts, comptes, bonds de validateurs
2. **Vérification de preuves ZK SP1** — les Appchains prouvent leur état, le L1 vérifie
3. **Consensus BFT** — finalité déterministe, un bloc, aucune réorganisation

Pas d'EVM. Pas de WASM. Pas de logique applicative sur L1. Plus minimaliste qu'Ethereum L1.

---

## Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│  Appchains (hors-L1, permissionless)                              │
│  DEX · Lending · Identité · etc.                                  │
│  → génèrent des preuves SP1 → soumettent au L1                   │
└────────────────────────────┬─────────────────────────────────────┘
                             │ AnchorState + SP1 proof
                             ▼
┌──────────────────────────────────────────────────────────────────┐
│  VinX L1 — settlement layer ZK-natif                              │
│  • Comptes VINX · bond/slashing · frais                           │
│  • Vérification preuves SP1 Groth16 (ADR 0050)                   │
│  • Clearinghouse cross-Appchain (ADR 0049)                        │
│  • ForceExit / Escape Hatch (ADR 0048)                            │
│  • Consensus : PoS Algorand-style, comité VRF n≈100, BLS agrégé  │
└──────────────────────────────────────────────────────────────────┘
                             │ DA (state diffs)
                             ▼
                         Celestia
```

---

## Consensus — PoS Algorand-style

Le consensus VinX est un **PoS pur avec comité réduit à sélection VRF** :

| Propriété | Valeur |
|-----------|--------|
| **Sélection** | VRF ECVRF (RFC 9381) — tirage uniforme parmi les bondés |
| **Comité** | n ≈ 100 validateurs par bloc |
| **Signatures** | BLS12-381 agrégé — 100 signatures → 1, vérification O(1) |
| **Finalité** | BFT déterministe (1 bloc, ≥ 67 % du comité) |
| **Admission** | Permissionless — bond suffit, pas d'approbation admin |
| **Leader** | Validateur avec la sortie VRF la plus faible (imprévisible) |

**Résistance DoS** : le leader est imprévisible jusqu'au dernier moment — contrairement au round-robin, un attaquant ne sait pas qui cibler.

---

## Appchains

Les Appchains s'exécutent hors-L1, génèrent des preuves SP1, et les soumettent pour vérification + settlement :

- **Permissionless** : n'importe qui peut déployer une Appchain avec un bond
- **Sécurité cryptographique** : ZK proof = L1 ne fait pas confiance au séquenceur pour l'exécution
- **Séquenceur unique + Escape Hatch** : si le séquenceur censure ou tombe, le `ForceExit` (ADR 0048) permet de récupérer ses fonds directement sur le L1
- **Cross-chain** : le Clearinghouse L1 (ADR 0049) gère les transferts inter-Appchains via message passing asynchrone

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

- **100 milliards VINX**, supply fixe et immuable.
- **Aucun pre-mine, aucune réserve.** Les premiers VINX n'existent qu'au moment où le premier bloc est produit.
- **Émission par le travail** : minting progressif, décroissance exponentielle continue, demi-vie ~20 ans.
- **Validateurs = fees only (cible)** : à terme, l'émission va aux Appchains via usage réel (melt), les validateurs vivent des frais de transaction.
- **Invariant** : `circulation + pot_époque + détruits = émis ≤ 100 Md` — garanti à chaque bloc.

> Whitepaper : [whitepaper.md](./whitepaper.md)

---

## Roadmap

| Phase | Période | Contenu |
|-------|---------|---------|
| ✅ **Phase 1 — Fondations** | Terminé | L1 complet (consensus PoA Threshold → base PoS), finalité BFT, fair launch, BLS12-381, jailing, fork-choice, P2P anti-DoS, gouvernance K-of-M, modules bondés |
| 🔄 **Phase 2 — PoS Algorand** | Q4 2026 | Comité VRF (ECVRF RFC 9381), sélection par VRF, admission PoS permissionless (ADR 0029/0038) |
| 📅 **Phase 3 — Appchains ZK** | Q1 2027 | SP1 proof verification (ADR 0050), ForceExit / Escape Hatch (ADR 0048), Clearinghouse (ADR 0049), Celestia DA (ADR 0034) |
| 🔭 **Phase 4 — Écosystème** | 2027+ | Appchains communautaires, émission élastique (ADR 0047), light client, réseau public |

> Index complet des décisions d'architecture : [docs/adr/README.md](./docs/adr/README.md)

---

## Structure du projet

```
crates/
├── vinx-crypto/        Ed25519, BLS12-381, adresses Bech32, SHA-256, Merkle
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
└── adr/                Décisions d'architecture (ADR 0001 → 0050)
```

---

*VinX Labs — août 2026*
