# VinX Ledger — État du Projet

> Document de référence interne — mis à jour à chaque sprint.
> Dernière mise à jour : juillet 2026 (v4 — warm-up staking, console admin `/admin`, robustesse & migration de schéma sans wipe, clés typées + ahash, capacités relevées, cadence de bloc adaptative à la demande).

---

## Table des matières

1. [Ce qu'on a construit](#1-ce-quon-a-construit)
2. [Architecture — les 5 briques](#2-architecture--les-5-briques)
3. [Le protocole en détail](#3-le-protocole-en-détail)
4. [L'API RPC (endpoints HTTP)](#4-lapi-rpc-endpoints-http)
5. [Le wallet CLI — toutes les commandes](#5-le-wallet-cli--toutes-les-commandes)
6. [Le SDK TypeScript](#6-le-sdk-typescript)
7. [Infrastructure & outils](#7-infrastructure--outils)
8. [Ce qui reste à faire](#8-ce-qui-reste-à-faire)
9. [Commandes utiles du quotidien](#9-commandes-utiles-du-quotidien)

---

## 1. Ce qu'on a construit

VinX Ledger est une blockchain L1 de paiement écrite intégralement en Rust, sans framework tiers. Voici la liste complète des fonctionnalités implémentées :

### Protocole de base
- [x] Cryptographie Ed25519 + SHA-256 + adresses Bech32 (`vinx1...`)
- [x] Arbre de Merkle avec preuves d'inclusion vérifiables
- [x] 8 types de transactions (transfer, stake, unstake, announce-upgrade, add/remove/slash validator, admin action)
- [x] État mondial (`WorldState`) avec validation complète
- [x] État de genèse configurable (admin + validateur initial)
- [x] Consensus PoA Threshold — quorum `⌈2n/3⌉`, round-robin leader
- [x] Finalité déterministe et immédiate (pas de réorganisation possible)

### Production de blocs
- [x] Producteur **à la demande, cadence adaptative** : repos → aucun bloc ; activité légère → ~`block_time` (5 s) ; charge → l'écart se resserre avec le remplissage ; saturation → blocs dos-à-dos. Garde anti-spin (pas de blocs vides en boucle)
- [x] Capacités : **10 000 tx/bloc**, mempool **100 000** (réglables `max_block_txs` / `max_mempool_size`)
- [x] Frais dynamiques style EIP-1559 (×1 à ×3 selon la charge mémoire)
- [x] **Melt intégral** : 100 % des frais fondent dans La Fonderie
- [x] Vérification des signatures en parallèle (rayon, tous les cœurs CPU)
- [x] Détection des slots manqués (leader timeout ≥ 3 slots consécutifs)

### Réseau P2P
- [x] Gossipsub libp2p pour la propagation des blocs et transactions
- [x] mDNS — découverte automatique des pairs en LAN
- [x] Vérification cryptographique des blocs avant application
- [x] Détection d'équivocation (double-signature sur deux blocs différents)
- [x] Synchronisation de blocs par P2P (`SyncRequest` / `SyncResponse`)
- [x] Synchronisation au démarrage depuis un pair de confiance (HTTP)

### Économie — La Fonderie (melt / forge)
- [x] Supply totale : 100 milliards de VinX, **immuable, sans burn** (précision : 18 décimales)
- [x] **Genèse** : 1 Md (1 %) forgé au fondateur · 99 Md (99 %) scellés dans **La Fonderie** (`foundry`)
- [x] **Invariant vérifié à chaque bloc** : `circulating_supply + foundry == 100 Md`
- [x] **Melt** : 100 % des frais fondent dans La Fonderie (`melt_to_foundry`) — pas un burn
- [x] **Forge** : récompenses de staking forgées depuis La Fonderie (`FORGE_RATE_BPS = 10`, soit 0,1 % par distribution), toutes les 100 blocs → la réserve ne se vide jamais
- [x] **Warm-up de staking** (`STAKE_WARMUP_BLOCKS = 100`) : un stake n'est éligible aux récompenses qu'après une époque complète (anti *just-in-time*) ; l'ancienneté `stake_since` est pondérée par le capital sur les top-ups

### Gouvernance — clé admin unique

La gouvernance est **centralisée** : une **clé admin unique** (le fondateur), **rotatable à chaud**, exécute les décisions de protocole on-chain. Pas de vote de validateurs, pas de DAO, **pas de gel de compte**.

- [x] 5 actions admin exécutables directement : `AddValidator`, `RemoveValidator`, `UpdateFeeFloor`, `ScheduleUpgrade`, `RotateAdmin`
- [x] Transaction `AdminAction` (0x08) : encapsule une `GovernanceAction` dans la payload, requiert la signature de la clé admin, s'exécute immédiatement
- [x] Rotation de la clé admin possible via `AdminAction::RotateAdmin` (sans redémarrage du nœud)

### Sécurité
- [x] Slashing : preuve d'équivocation → 10 % bounty au rapporteur, 90 % **fondu dans La Fonderie**, validateur exclu
- [x] Rate limiting : 100 requêtes/minute par IP (middleware axum)
- [x] Auth token Bearer sur les routes `/snapshot` et `/admin/compact`
- [x] Compaction de chaîne (`compact_old_txs`) — supprime les tx anciennes, conserve les headers

### API & interfaces
- [x] Endpoints HTTP/REST (voir section 4)
- [x] Server-Sent Events `/events` — push en temps réel à chaque bloc
- [x] WebSocket `/ws` — identique aux SSE, protocole bidirectionnel
- [x] Métriques Prometheus sur `/metrics` (dont `vinx_foundry`)
- [x] Interface web embarquée sur `/` (explorateur + wallet, signature locale)
- [x] **Console d'administration `/admin`** — dashboard, validateurs (ajout/retrait/approbation), upgrades, maintenance ; actions signées localement avec la clé admin, lecture seule sinon
- [x] Preuves Merkle via `/account/:address/proof`

### Robustesse & performance
- [x] **Migration de schéma forward** : les anciennes données sont migrées en place à l'ouverture — plus de wipe au changement de version (repli snapshot documenté sinon)
- [x] Démarrage **sans panique** (ouverture du stockage faillible et gracieuse)
- [x] Clés typées (`Address`/`Hash32`) + **ahash** sur les index et le mempool (hot-path sans allocation de String)
- [x] Persistance incrémentale (seuls les comptes modifiés réécrits) + Merkle incrémental

### Outillage
- [x] Wallet CLI — 24 commandes (voir section 5)
- [x] SDK TypeScript — 15 méthodes, 22 types, 19 tests Jest
- [x] Docker Compose — testnet 3 validateurs prêt à l'emploi
- [x] CI/CD GitHub Actions — 4 jobs automatiques
- [x] Script devnet local `scripts/devnet.sh`
- [x] HD wallet BIP-39 — mnémonique 12 mots → clé ed25519

---

## 2. Architecture — les 5 briques

```
crates/
├── vinx-crypto/     Primitives cryptographiques
├── vinx-core/       Types du protocole (blocs, transactions, gouvernance)
├── vinx-state/      État de la chaîne (WorldState + genèse)
├── vinx-node/       Nœud complet (P2P, consensus, RPC, mempool)
└── vinx-wallet/     Wallet CLI et client RPC
sdk/
└── vinx-sdk/        SDK TypeScript
```

### `vinx-crypto` — Cryptographie

| Élément | Description |
|---------|-------------|
| `sha256(data)` | Hachage SHA-256 |
| `KeyPair` | Paire de clés Ed25519 |
| `PublicKey` | Clé publique 32 octets |
| `VinxSignature` | Signature Ed25519 64 octets |
| `Address` | Adresse Bech32 `vinx1...` |
| `merkle_root(leaves)` | Racine d'un arbre de Merkle |
| `merkle_proof_for(leaves, index)` | Preuve d'inclusion pour la feuille n°`index` |
| `verify_merkle_proof(leaf, proof, root)` | Vérifie une preuve sans l'arbre complet |

### `vinx-core` — Protocole

| Élément | Description |
|---------|-------------|
| `Amount` | Montant en atomes (10⁻¹⁸ VinX), précision maximale |
| `Account` | Adresse, solde, staked, nonce, `stake_since` |
| `Block` / `BlockHeader` | Bloc avec hauteur, hash, validateur, frais de base |
| `BlockSignature` | Co-signature d'un validateur |
| `SlashEvidence` | Preuve de double-signature |
| `Transaction` | 8 types encodés dans un objet unique |
| `ValidatorSet` | Ensemble des validateurs autorisés + calcul quorum |
| `GovernanceAction` | Actions admin exécutables (5 variants) |

**Les 8 types de transactions :**

| Code | Type | Rôle |
|------|------|------|
| `0x01` | `Transfer` | Envoi de VinX |
| `0x02` | `Stake` | Verrouillage en staking |
| `0x03` | `Unstake` | Déverrouillage du staking |
| `0x04` | `AnnounceUpgrade` | Annonce d'une mise à jour protocole (admin) |
| `0x05` | `AddValidator` | Ajout d'un validateur (admin) |
| `0x06` | `RemoveValidator` | Suppression d'un validateur (admin) |
| `0x07` | `SlashValidator` | Slashing pour équivocation |
| `0x08` | `AdminAction` | Action de gouvernance directe (admin, exécution immédiate) |

### `vinx-state` — WorldState

Le `WorldState` est l'état complet de la chaîne. Il est sérialisé sur disque après chaque bloc.

| Champ | Type | Description |
|-------|------|-------------|
| `accounts` | `BTreeMap<Address, Account>` | Tous les comptes (clé = adresse 20 octets, itération triée) |
| `circulating_supply` | `Amount` | Tokens détenus par les comptes (= `MAX_SUPPLY - foundry`) |
| `block_height` | `u64` | Hauteur actuelle |
| `foundry` | `Amount` | **La Fonderie** — réserve melt/forge. `circulation + foundry == 100 Md` |
| `fee_floor` | `Amount` | Plancher de frais minimum |
| `base_fee` | `Amount` | Frais dynamiques actuels |
| `admin_address` | `Option<Address>` | Clé admin (rotatable via `AdminAction::RotateAdmin`) |
| `current_version` | `ProtocolVersion` | Version actuelle du protocole |
| `pending_upgrade` | `Option<ScheduledUpgrade>` | Mise à jour planifiée |
| `validator_set` | `ValidatorSet` | Validateurs actifs |

### `vinx-node` — Nœud complet

Modules internes :

| Module | Rôle |
|--------|------|
| `chain` | Suivi du tip de chaîne, détection d'équivocation |
| `consensus` | Validation des blocs, signatures de co-validateurs |
| `mempool` | File de transactions (tri par frais, vérification nonce) |
| `producer` | Production de blocs, frais dynamiques, récompenses |
| `p2p` | Gossipsub + mDNS (libp2p) |
| `sync` | Synchronisation depuis un pair RPC au démarrage |
| `storage` | Persistance bincode sur disque |
| `rpc` | Serveur HTTP axum, WebSocket, SSE, rate limiting |
| `node` | Orchestration : boucle de production, events, persist |

---

## 3. Le protocole en détail

### Consensus PoA Threshold

1. Les validateurs sont listés dans le `ValidatorSet` (triés par adresse)
2. Le leader du slot `height` = `validators[height % n]`
3. Le leader produit le bloc et le broadcast via P2P
4. Les autres validateurs co-signent le hash du header
5. Un bloc est finalisé quand `valid_sigs ≥ ⌈2n/3⌉`
6. La genèse (height 0) est toujours considérée finalisée

### Frais dynamiques (EIP-1559 adapté)

```
charge = mempool_pending / max_block_txs
multiplier = 1.0 + 2.0 × max(0, charge - 0.8) / 0.2
base_fee = fee_floor × multiplier  (cap à 3×)
```

**Melt de chaque fee : 100 %** → `foundry` (La Fonderie). Aucun frais direct au validateur, aucune trésorerie séparée. Ce n'est **pas** un burn : le métal est conservé et sera reforgé en récompenses.

### Gouvernance — clé admin unique

La gouvernance de VinX est **centralisée** : une clé admin unique (le fondateur) prend les décisions de protocole. Les échanges communautaires se font hors-chaîne.

On-chain, l'admin soumet une transaction `AdminAction` avec la `GovernanceAction` souhaitée en payload. Elle est vérifiée (signature admin) et exécutée immédiatement dans le même bloc. Pas de vote, pas de délai.

Actions disponibles :

| Action | Effet |
|--------|-------|
| `AddValidator(addr)` | Ajoute un validateur au PoA set |
| `RemoveValidator(addr)` | Retire un validateur du PoA set |
| `UpdateFeeFloor { atoms }` | Modifie le plancher de frais |
| `ScheduleUpgrade { version, height }` | Planifie une mise à jour protocole |
| `RotateAdmin(addr)` | Change la clé admin sans redémarrage |

### Le cycle melt / forge

VinX n'a **aucun burn**. « Melter » signifie que les jetons **fondent** dans La Fonderie (`foundry`), la réserve unique — ils ne sont pas détruits. « Forger » signifie que le protocole en **forge** de nouveaux depuis La Fonderie vers les stakers.

Comme la forge ne prend qu'une **fraction** de La Fonderie (`FORGE_RATE_BPS = 10`) et que les frais la refont fondre en continu, **la réserve ne se vide jamais** : c'est le même métal qui circule à l'infini. L'invariant `circulation + Fonderie = 100 Md` tient à chaque bloc.

---

## 4. L'API RPC (endpoints HTTP)

Le nœud expose un serveur HTTP sur `0.0.0.0:8545` par défaut.

| Méthode | Route | Description |
|---------|-------|-------------|
| GET | `/` | Interface web (explorateur de blocs) |
| GET | `/admin` | Console d'administration (dashboard + gouvernance signée en local) |
| GET | `/health` | Santé du nœud : hauteur, mempool, statut |
| GET | `/chain/height` | Hauteur actuelle du tip |
| GET | `/chain/sync?from=N&limit=N` | Synchronisation d'une plage de blocs |
| GET | `/block/:height` | Bloc complet par hauteur |
| GET | `/tx/:hash` | Transaction par hash hex |
| GET | `/account/:address` | Solde, nonce, staked d'un compte |
| GET | `/account/:address/txs?limit=N&offset=N` | Historique des transactions |
| GET | `/account/:address/proof` | Preuve Merkle d'inclusion dans l'état |
| POST | `/tx/submit` | Soumettre une transaction signée |
| GET | `/mempool/size` | Nombre de transactions en attente |
| GET | `/validators` | Ensemble des validateurs actifs + quorum |
| GET | `/protocol/version` | Version courante + upgrade planifiée |
| GET | `/network/stats` | Statistiques économiques (base_fee, foundry, supply, admin) |
| GET | `/metrics` | Métriques Prometheus |
| GET | `/events` | Server-Sent Events — push par bloc |
| GET | `/ws` | WebSocket — push par bloc |
| GET | `/snapshot` | État complet JSON (⚠ admin auth si token configuré) |
| POST | `/admin/compact` | Supprime les vieilles tx des blocs (⚠ admin auth) |
**Auth admin :** si `admin_token` est configuré dans `config.toml`, les routes `/snapshot` et `/admin/compact` requièrent le header `Authorization: Bearer <token>`.

**Rate limiting :** 100 requêtes / 60 secondes par IP. Retourne HTTP 429 en cas de dépassement.

---

## 5. Le wallet CLI — toutes les commandes

```bash
cargo run -p vinx-wallet -- <commande> [options]
```

**Gestion des clés :**

| Commande | Options clés | Description |
|----------|-------------|-------------|
| `keygen` | `--output wallet.json` | Génère une nouvelle paire de clés |
| `new-wallet` | `--output wallet.json` | Génère un wallet HD (mnémonique BIP-39 12 mots) |
| `restore-wallet` | `--output wallet.json` | Restaure un wallet depuis un mnémonique |
| `address` | `--wallet wallet.json` | Affiche l'adresse du wallet |

**Consultation :**

| Commande | Options clés | Description |
|----------|-------------|-------------|
| `balance` | `--address vinx1...` | Solde d'un compte |
| `block` | `--height N` | Détails d'un bloc |
| `tx` | `--hash abc123` | Détails d'une transaction |
| `status` | | Hauteur de chaîne + mempool |
| `history` | `--address vinx1... --limit 20` | Historique des transactions |
| `validators` | | Liste des validateurs actifs |
| `protocol` | | Version protocole + upgrade planifiée |
| `network-stats` | | Frais de base, pools, supply |

**Transactions :**

| Commande | Options clés | Description |
|----------|-------------|-------------|
| `transfer` | `--to vinx1... --amount 100` | Envoyer des VinX |
| `stake` | `--amount 500` | Verrouiller des VinX en staking |
| `unstake` | `--amount 500` | Déverrouiller du staking |

**Admin — transactions directes (requiert la clé admin) :**

| Commande | Options clés | Description |
|----------|-------------|-------------|
| `add-validator` | `--validator vinx1...` | Ajouter un validateur |
| `remove-validator` | `--validator vinx1...` | Retirer un validateur |
| `announce-upgrade` | `--version 1.1.0 --activation-height 100000` | Planifier un upgrade |

**Admin — actions de gouvernance (requiert la clé admin) :**

| Commande | Options clés | Description |
|----------|-------------|-------------|
| `admin-action` | `--action '{"AddValidator":"vinx1..."}'` | Exécute une `GovernanceAction` immédiatement |

Exemple d'actions JSON valides :
```bash
# Ajouter un validateur
--action '{"AddValidator":"vinx1abc..."}'

# Retirer un validateur
--action '{"RemoveValidator":"vinx1abc..."}'

# Modifier le plancher de frais (en atomes)
--action '{"UpdateFeeFloor":{"atoms":100000000000000}}'

# Faire tourner la clé admin
--action '{"RotateAdmin":"vinx1nouveau..."}'
```

> Toutes les commandes de transaction acceptent `--node http://127.0.0.1:8545` (défaut) et `--wallet wallet.json` (défaut).

---

## 6. Le SDK TypeScript

**Installation :**
```bash
cd sdk/vinx-sdk && npm install && npm run build
```

**Utilisation :**
```typescript
import VinxClient from '@vinx/sdk';

const client = new VinxClient('http://localhost:8545');

// Santé du nœud
const health = await client.health();

// Solde d'un compte
const account = await client.account('vinx1abc...');

// Envoyer une transaction
const result = await client.submitTx({ ... });

// Stream temps réel (SSE)
const es = client.blockEvents();
es.onmessage = (e) => console.log(JSON.parse(e.data));
```

**Méthodes disponibles :**

| Méthode | Description |
|---------|-------------|
| `health()` | Santé du nœud |
| `height()` | Hauteur de chaîne |
| `account(address)` | Détails d'un compte |
| `accountTxs(address, opts?)` | Historique des transactions |
| `block(height)` | Bloc par hauteur |
| `tx(hash)` | Transaction par hash |
| `submitTx(tx)` | Soumettre une transaction |
| `mempool()` | Taille du mempool |
| `validators()` | Validateurs actifs |
| `protocolStatus()` | Version protocole |
| `networkStats()` | Statistiques économiques |
| `chainSync(from, limit?)` | Synchronisation de blocs |
| `proposals()` | Liste des propositions |
| `proposal(id)` | Proposition par ID |
| `blockEvents()` | Stream SSE temps réel |

**Tests :** 19 tests Jest couvrant chaque méthode + gestion d'erreurs.

---

## 7. Infrastructure & outils

### Docker Compose (3 validateurs)

```bash
docker compose up --build
```

Lance 3 nœuds en réseau isolé :
- **node1** : `localhost:8545` (RPC) + `localhost:9001` (P2P) — nœud genesis
- **node2** : `localhost:8546` (RPC) + `localhost:9002` (P2P) — sync depuis node1
- **node3** : `localhost:8547` (RPC) + `localhost:9003` (P2P) — sync depuis node1

### Devnet local (sans Docker)

```bash
./scripts/devnet.sh           # démarre 3 nœuds en local
./scripts/devnet.sh --clean   # repart de zéro
```

### CI/CD GitHub Actions

4 jobs se déclenchent à chaque push / PR :

| Job | Ce qu'il vérifie |
|-----|-----------------|
| `cargo test` | 186 tests unitaires et d'intégration |
| `clippy` | Qualité du code Rust (zéro warning autorisé) |
| `rustfmt` | Formatage du code |
| `sdk-test` | 19 tests TypeScript Jest |

### Configuration du nœud (`config.toml`)

```toml
block_time_secs = 5           # Écart entre blocs sous activité légère ; se resserre à charge, dos à dos à saturation
max_block_txs = 10000         # Transactions max par bloc
max_mempool_size = 100000     # Transactions max en attente dans le mempool
rpc_listen = "0.0.0.0:8545"  # Adresse RPC
data_dir = "data"             # Répertoire des données
admin_token = "secret"        # Token Bearer pour les routes admin (optionnel)

p2p_listen = "/ip4/0.0.0.0/tcp/9000"   # Activer le P2P
peers = ["/ip4/1.2.3.4/tcp/9000"]       # Pairs de démarrage
sync_peer_rpc = "http://1.2.3.4:8545"  # Sync depuis un pair au démarrage
```

---

## 8. Ce qui reste à faire

| Priorité | Fonctionnalité | Détail |
|----------|---------------|--------|
| 🔴 Haute | **Usage réel** | Faire tourner la chaîne, distribuer le 1 Md à un premier cercle, micro-économie |
| 🟡 Moyenne | **Console admin Phase 2** | Plancher de frais + rotation admin en types de tx dédiés (signature triviale) |
| 🟡 Moyenne | **Run 3 validateurs** | Valider co-signing / quorum / tolérance de panne en réel (le P2P existe, testé à 1) |
| 🟡 Moyenne | **Leviers éco** | Taux de forge gouvernable on-chain (E2), forge dynamique (E3) |
| 🟢 Future | **Token factory** | Émettre d'autres actifs sur VinX (interaction avec la Fonderie mère à concevoir) |
| 🟢 Future | **Exécution parallèle** | Pertinent seulement à des dizaines de milliers de TPS soutenus — chantier d'architecture, risque de déterminisme |
| 🟢 Future | **TLS natif** (rustls) | HTTPS sur le RPC sans dépendance à un reverse-proxy |

> **Fait cette itération** : warm-up de staking (anti-JIT), console admin `/admin`, robustesse au démarrage, clés typées + ahash, capacités relevées (10k tx/bloc, mempool 100k), cadence de bloc adaptative à la demande, **migration de schéma sans wipe**.

---

## 9. Commandes utiles du quotidien

```bash
# Lancer le devnet local
./scripts/devnet.sh

# Générer un wallet
cargo run -p vinx-wallet -- new-wallet --output mon-wallet.json

# Vérifier le solde
cargo run -p vinx-wallet -- balance --address vinx1...

# Envoyer des VinX
cargo run -p vinx-wallet -- transfer \
  --wallet mon-wallet.json \
  --to vinx1... \
  --amount 1000

# Voir les propositions de gouvernance
cargo run -p vinx-wallet -- show-proposals

# Lancer les tests
cargo test --workspace

# Construire le projet
cargo build --release

# Lancer avec Docker
docker compose up --build
```
