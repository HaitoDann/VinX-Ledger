<p align="center">
  <img src="./assets/vinx-logo.png" alt="VinX" width="540" />
</p>

<h1 align="center">VinX Ledger</h1>

<p align="center"><strong>Envoyer de l'argent devrait être aussi simple que d'envoyer un message.</strong></p>

<p align="center">L1 de paiement souverain — rapide, transparent, sans intermédiaire.</p>

---

## Pourquoi VinX ?

Aujourd'hui, envoyer de l'argent implique des banques, des frais cachés, des délais, et des tiers qui peuvent bloquer, censurer ou geler ton compte. VinX est une alternative : **une monnaie numérique qui fonctionne comme du cash** — tu l'envoies directement à qui tu veux, elle arrive en quelques secondes, et personne ne peut t'en empêcher.

Pas de smart contracts. Pas de spéculation. Une seule promesse : **des paiements du quotidien, vite, pas cher, et sous ton contrôle.**

VinX est entièrement construit en Rust, sans dépendance à un framework blockchain tiers. Chaque ligne de code, chaque règle du protocole est lisible et auditable.

---

## Comment ça marche (en simple)

Imagine un registre public partagé entre plusieurs serveurs indépendants (les **validateurs**). Quand tu envoies du VINX, ta transaction est vérifiée par plus de 66 % de ces validateurs simultanément. Une fois signée par ce quorum, elle est **définitive** — personne ne peut l'annuler, ni la réorganiser.

Les validateurs sont rémunérés par le protocole lui-même (émission progressive) et par les frais de transaction. Plus le réseau est utilisé, plus les frais suffisent — jusqu'au jour où l'émission s'éteint et les frais prennent entièrement le relais.

---

## Caractéristiques clés

| | |
|---|---|
| **Consensus** | PoA Threshold — >66 % des validateurs co-signent chaque bloc |
| **Finalité** | Déterministe — un bloc quorum-signé n'est jamais réorganisé |
| **Cadence** | Fixe à 12 s — un bloc produit toutes les 12 secondes |
| **Capacité** | 3 000 tx/bloc · ~250 TPS (configurable) |
| **Frais** | Forfait fixe × congestion (×1–3) · **100 % au validateur producteur** |
| **Cryptographie** | Ed25519 · BLS12-381 · SHA-256 · Bech32 (`vinx1`) |
| **Supply** | 100 milliards VINX · fixe · **sans pre-mine · sans burn** |
| **Émission** | Décroissance exponentielle continue · demi-vie ~20 ans · puis fees-only |
| **Validateurs** | Bond minimum 100 000 VINX · slash équivocation 100 % · jailing sur downtime |
| **Staking** | Bond de sécurité uniquement — **aucun rendement passif** |

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
- **`/`** — explorateur de blocs + wallet web (la clé privée ne quitte jamais ton navigateur)
- **`/admin`** — console d'administration (dashboard, validateurs, upgrades)

Une **application desktop** (Tauri) est disponible dans [`apps/vinx-desktop`](./apps/vinx-desktop) — même wallet + console admin, 100 % local.

L'état est sauvegardé automatiquement (`devnet/`) et migré à chaque montée de version — aucun wipe nécessaire.

> Guide complet : [GUIDE.md](./GUIDE.md)

---

## Tokenomics — Fair launch

- **100 milliards VINX**, supply fixe et immuable.
- **Aucun pre-mine, aucune réserve, aucune allocation fondateur.** Les premiers VINX n'existent qu'au moment où le premier bloc est produit.
- **Émission par le travail** : les VINX sont mintés progressivement en rémunération des blocs produits, selon une courbe de décroissance exponentielle continue (`R₀ · e^(−λt)`, demi-vie ~20 ans). Le total de cette courbe vaut exactement 100 milliards.
- **Rémunération par le travail** : aujourd'hui, l'émission va **100 % au producteur du bloc** (frais inclus). Une distribution par époque entre proposeurs et co-signataires est planifiée (Phase 2), **sans pondération par le bond** — chaque co-signature comptera pour le même poids.
- **Frais** : forfaitaires, **100 % au validateur producteur immédiatement** — ils ne passent pas par le pot d'époque.
- **Slashing** : équivocation prouvée → 100 % du bond (10 % au rapporteur, 90 % redistribués aux validateurs honnêtes). Aucun token détruit.
- **Invariant** vérifié à chaque bloc : `circulation + pot_époque + détruits = émis ≤ 100 000 000 000 VINX`.

> Whitepaper complet : [whitepaper.md](./whitepaper.md)

---

## Staking = caution, pas rendement

Le bond n'est pas un investissement — c'est une **caution de bonne conduite**. Un validateur qui triche perd son bond. Un validateur qui travaille honnêtement est rémunéré par l'émission et les frais.

- Bond minimum : **100 000 VINX** (gouvernable)
- Déliaison : **3 jours** de temps réel (reste saisissable en cas d'équivocation)
- Downtime : suspension du round-robin, sans slash
- Un utilisateur lambda ne stake pas — il garde son VINX pour **l'utiliser comme cash**.

---

## Roadmap

| Phase | Période | Contenu |
|-------|---------|---------|
| ✅ **Phase 1 — Fondations** | Terminé | Protocole L1 complet (PoA Threshold, finalité déterministe, fair launch, slashing prouvable, fork-choice, P2P anti-DoS, gouvernance K-of-M, registre de modules, **BLS12-381**, jailing / rotation active) |
| 🔄 **Phase 2 — Récompenses & ouverture** | Q3 2026 | Distribution par époque (émission + slash entre proposeurs et co-signataires), **Open PoA** (admission sans permission sur bond), accountability co-signatures, tx `Unjail` |
| 📅 **Phase 3 — Subnets & ancrage** | Q4 2026 | Subnets (escrow + récompense par usage), Data Availability, preuves d'ancre, émission élastique à réservoir |
| 🔭 **Phase 4 — Décentralisation** | 2027 | Comité VRF, light client, réseau public, documentation multilingue |

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
└── adr/                Décisions d'architecture (ADR 0001 → ...)
```

---

*VinX Labs — août 2026*
