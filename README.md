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
| **Frais** | Forfait 0,0001 VINX × poids × congestion (×1–3) · **100 % au validateur producteur** (immédiat) |
| **Cryptographie** | Ed25519 · SHA-256 · Bech32 (`vinx1`) |
| **Supply** | 100 milliards VINX (immuable, sans burn, **sans pre-mine**) |
| **Émission** | Par le travail des validateurs · **décroissance exponentielle continue**, demi-vie ~20 ans → 100 Md · distribuée par **époque** (proposeurs + co-signataires) · puis fees-only |
| **Admission validateur** | **Open PoA** — bond → file automatique · veto collectif >66 % · admin fixe seulement le bond · expansion phasée immuable |
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
- **Genèse** : **0 en circulation, 0 émis** — les tokens n'existent pas avant d'être produits par le travail. **Aucun pre-mine, aucune réserve pré-allouée, aucune allocation fondateur.**
- **Émission** : les VINX sont **mintés progressivement** en rémunération des blocs produits. Le débit suit une **décroissance exponentielle continue**, sans événement discret, avec une demi-vie de ~20 ans (`R(t) = R₀ · e^(−λt)`, R₀ ≈ 3,47 Md/an) — l'intégrale totale vaut exactement 100 Md. L'émission démarre basse et diminue régulièrement jusqu'à la poussière, puis les frais de transaction prennent le relais.
- **Temps réel** : l'émission est calculée sur les **timestamps** des blocs, jamais sur la hauteur (la cadence est variable).
- **Égalité entre validateurs** : l'émission est distribuée par **époque** (fenêtre temporelle) entre proposeurs et co-signataires, **non pondérée par le bond** — chaque co-signature a le même poids. Le montant du bond ne multiplie pas les gains ; seul le travail effectif compte.
- **Open PoA** : n'importe qui peut candidater au set en postant le bond — pas de sélection individuelle par l'admin. Le set s'élargit par phases automatiques et immuables (gravées à la genèse).
- **Relais automatique** : quand l'émission atteint la poussière, les **frais de transaction** deviennent la rémunération principale — bascule en **fees-only**, sans intervention.
- **Frais** : forfaitaires (indépendants du montant), **100 % au validateur producteur immédiatement**. Les frais ne passent pas par l'époque — seule l'émission est époquée.
- **Slashing** : équivocation prouvée → 100 % du bond (10 % au rapporteur, 90 % redistribués aux validateurs honnêtes via le pot d'époque) — aucun token détruit, aucune réinjection dans l'émission.
- **Invariant** vérifié à chaque bloc : `circulation + pot_époque + détruits = émis ≤ 100 000 000 000 VINX`.

> Détails complets : [whitepaper.md](./whitepaper.md)

---

## Staking = bond de sécurité (pas un rendement)

En PoA permissionné, la sécurité vient de l'identité des validateurs, pas d'un jeton. Le staking ne sert donc qu'à **une** chose : poser la caution qu'un validateur perd s'il triche.

- **Bond minimum 100 000 VINX** (gouvernable) pour rejoindre le set ; le validateur genesis est dispensé (bootstrap).
- **Aucun rendement** — le bond sécurise, le travail (émission + frais) rémunère.
- **Déliaison 3 jours** de temps réel : le retrait est différé pour rester saisissable pendant la fenêtre de preuve.
- **Slashing** : équivocation prouvée → 100 % du bond (10 % au rapporteur, 90 % redistribués aux validateurs honnêtes via le pot d'époque) ; downtime → suspension du round-robin, sans slash.

Un détenteur lambda ne stake pas : il garde son VINX pour **l'utiliser comme cash**.

---

## Roadmap

> Feuille de route détaillée et priorisée : **[`docs/adr/README.md`](./docs/adr/README.md)** (index de tous les ADR avec statut).

- **Fait** — protocole L1 Rust complet (PoA Threshold, fair launch, slashing prouvable, frais au producteur) + durcissements : vérif parallèle des signatures (ADR 0015), dépôt existentiel anti-bloat (0026), P2P anti-DoS (0022), gouvernance **K-of-M** (0011), **registre de modules bondés** (0010), vecteurs dorés canoniques (0020), **émission progressive sans La Fonderie** (0040 : minting pur, T_half ~20 ans, slash → pot d'époque, invariant `circ + pot + détruits = émis`). `cargo test --workspace` vert.
- **Chemin critique** — **banc 3 validateurs** puis finalité au quorum (0002), fork-choice (0031), jailing (0027), accountability co-sign (0030).
- **Ensuite** — **récompenses par époque** (0028 : distribuer l'émission + slash entre proposeurs et co-signataires), **Open PoA** (0038), **rémunération modules par escrow** (0039), DA & preuves d'ancre (0034).
- **Plus tard** — décentralisation à l'échelle (BLS + comité VRF, 0029), light client (0014), réseau public. Vision d'ensemble : [ADR 0001](./docs/adr/0001-l1-monnaie-pure-modules-ancrage-bonde.md).

---

*VinX Labs — août 2026*
