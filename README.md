# VinX Ledger

**Cash numérique artisanal — L1 de paiement souverain, rapide et précis au centime.**

---

## Qu'est-ce que VinX Ledger ?

VinX Ledger est une blockchain L1 conçue exclusivement pour les paiements du quotidien. Pas de smart contracts, pas de spéculation — une seule promesse : envoyer de l'argent vite, pas cher, et sans intermédiaire opaque.

Sa monnaie suit un modèle de **fair launch** : une supply fixe de 100 milliards, **sans burn** et **sans pre-mine**, où **tous les jetons entrent en circulation par le travail des validateurs** (émission décroissante), puis les frais de transaction prennent le relais.

Implémenté intégralement en Rust, sans framework blockchain tiers.

---

## Caractéristiques clés

| | |
|---|---|
| **Consensus** | PoA Threshold — >66% des validateurs co-signent chaque bloc |
| **Finalité** | Déterministe — un bloc quorum-signé n'est jamais réorganisé |
| **Cadence** | Adaptative à la demande — repos → 0 bloc · normal → ~5s · charge → l'écart se resserre · saturation → dos à dos |
| **Référence de temps** | Timestamp des blocs (temps réel), pas la hauteur de bloc |
| **Capacité** | 10 000 tx/bloc · mempool 100 000 · plusieurs milliers de TPS (config-dépendant) |
| **Frais** | Forfait 0,0001 VINX × poids × congestion (×1–3) · **100 % au validateur producteur** |
| **Cryptographie** | Ed25519 · SHA-256 · Bech32 (`vinx1`) |
| **Supply** | 100 milliards VINX (immuable, sans burn, **sans pre-mine**) |
| **Émission** | Par le travail des validateurs · décroissance exponentielle, **halving tous les 8 ans** → 100 Md · puis fees-only |
| **Staking** | **Bond de validateur** (min 100k VINX) · déliaison 3 jours · slash équivocation 100 % · **aucun rendement** |
| **Exploitation** | Console d'admin web (`/admin`) · mises à jour **sans wipe** (migration de schéma) |

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

Le nœud expose sur **http://localhost:8545** :
- **`/`** — explorateur + wallet web (signature locale, la clé ne quitte jamais le navigateur)
- **`/admin`** — console d'administration (dashboard, validateurs, upgrades), en lecture seule tant que la clé admin n'est pas chargée

Une **application desktop native** (Tauri) est aussi disponible dans [`apps/vinx-desktop`](./apps/vinx-desktop) : même wallet + console admin, signature 100 % locale.

L'état est sauvegardé automatiquement sur disque (`devnet/`), et **migré vers l'avant** à chaque montée de version — plus jamais besoin d'effacer le dossier.

> Guide complet : [GUIDE.md](./GUIDE.md)

---

## Structure du projet

```
crates/
├── vinx-crypto/        Ed25519, adresses Bech32, SHA-256, Merkle
├── vinx-core/          Amount, Account, Transaction, Block
├── vinx-state/         WorldState, genesis, apply_transaction, émission
├── vinx-node/          Nœud, mempool, RPC HTTP, producteur de blocs, persistance
├── vinx-wallet/        CLI wallet
└── vinx-desktop-core/  Logique wallet partagée (keystore, signature) — Tauri-agnostique
apps/
└── vinx-desktop/       Application desktop Tauri (wallet + console admin, signature locale)
sdk/
└── vinx-sdk/           SDK TypeScript
docs/
└── adr/                Décisions d'architecture (ADR)
```

---

## Tokenomics — Fair launch & émission par le travail

- **100 milliards VINX**, supply fixe et **immuable**.
- **Genèse** : **0 en circulation, 100 Md scellés dans La Fonderie** (la réserve d'émission). **Aucun pre-mine, aucune allocation fondateur** — le fondateur gagne ses VINX comme tout le monde, en faisant tourner des validateurs.
- **Émission** : les VINX sortent de La Fonderie **uniquement pour rémunérer la production de blocs**. Le débit décroît de façon exponentielle et est **divisé par deux tous les 8 ans** (`débit(t) = R₀ · 2^(−t/8 ans)`, R₀ ≈ 8,66 Md/an) — l'intégrale totale vaut exactement 100 Md.
- **Temps réel** : l'émission est calculée sur les **timestamps** des blocs, jamais sur la hauteur (la cadence est variable).
- **Égalité entre validateurs** : l'émission est créditée au producteur du bloc et **n'est pas pondérée par le bond** — en round-robin, chacun gagne ~1/*n*.
- **Relais automatique** : quand La Fonderie se vide, l'émission s'efface et les **frais de transaction** deviennent la rémunération — bascule en **fees-only**, sans intervention.
- **Frais** : forfaitaires (indépendants du montant), **100 % au validateur producteur** (plus de *melt*).
- **Invariant** vérifié à chaque bloc : `circulation + Fonderie = 100 000 000 000 VINX`.

> Détails complets : [whitepaper.md](./whitepaper.md)

---

## Staking = bond de sécurité (pas un rendement)

En PoA permissionné, la sécurité vient de l'identité des validateurs, pas d'un jeton. Le staking ne sert donc qu'à **une** chose : poser la caution qu'un validateur perd s'il triche.

- **Bond minimum 100 000 VINX** (gouvernable) pour rejoindre le set ; le validateur genesis est dispensé (bootstrap).
- **Aucun rendement** — le bond sécurise, le travail (émission + frais) rémunère.
- **Déliaison 3 jours** de temps réel : le retrait est différé pour rester saisissable pendant la fenêtre de preuve.
- **Slashing** : équivocation prouvée → 100 % du bond (10 % au rapporteur, reste fondu dans La Fonderie) ; downtime → suspension du round-robin, sans slash.

Un détenteur lambda ne stake pas : il garde son VINX pour **l'utiliser comme cash**.

---

## Roadmap

- **Fait** — protocole L1 Rust complet (PoA Threshold) et modèle *fair launch* (émission par le travail, bond de validateur avec slashing prouvable, frais au producteur) **implémentés et testés**. Exploité en local.
- **Ensuite** — redondance 1 → 3 validateurs, finalité au quorum, amorçage de la micro-économie par l'émission, premiers usages réels.
- **Plus tard (optionnel)** — réseau public, et **surcouches / modules hors-nœud** (token factory, etc.) reliés par **ancrage bondé**, sans jamais salir le cœur — voir [ADR 0001](./docs/adr/0001-l1-monnaie-pure-modules-ancrage-bonde.md).

---

*VinX Labs — juillet 2026*
