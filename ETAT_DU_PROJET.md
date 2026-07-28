# VinX Ledger — État du Projet

> Document de référence interne — mis à jour à chaque sprint.
> Dernière mise à jour : juillet 2026 (**refonte économique v5 adoptée — fair launch / émission par le travail** ; voir whitepaper v4.0).

---

> ## ✅ Refonte économique v5 — fair launch (implémentée)
>
> Le modèle économique de VinX a été **entièrement repensé et implémenté** (whitepaper v4.0). Le cycle *melt/forge* est remplacé par un **fair launch** :
>
> - **Aucun pre-mine** : à la genèse, 0 en circulation, 100 Md scellés dans La Fonderie.
> - **Émission par le travail des validateurs** : décroissance par **halving tous les 8 ans** (arithmétique entière déterministe), calculée en **temps réel** (timestamps). Créditée au producteur, **non pondérée par le bond**. Relais automatique vers les frais quand La Fonderie se vide.
> - **Frais forfaitaires** (indépendants du montant), **100 % au validateur producteur** (plus de melt).
> - **Staking = bond de validateur** (min 100 000 VINX, gouvernable), **déliaison 3 jours** temps réel, **slash équivocation 100 %** avec preuve réellement vérifiée. **Aucun rendement de staking.**
> - Le **timestamp** devient la référence de temps.
>
> **État du code : implémenté, testé** (`cargo test --workspace` vert, clippy `-D warnings` & fmt propres). Les sections ci-dessous portant la mention *« modèle actuel melt/forge »* décrivent l'**ancien** code désormais remplacé — l'annotation *« → v5 »* indique le comportement en vigueur. **Différé** : préavis d'upgrade en temps réel + cosmétique UI admin / SDK ([§8](#8-ce-qui-reste-à-faire)).

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

### Économie — ⚠️ modèle actuel (melt / forge), en cours de remplacement par le fair launch (v5)
> Ce qui suit décrit le **code actuel**. Il sera remplacé par l'émission par le travail (voir bandeau en tête et [§8](#8-ce-qui-reste-à-faire)).
- [x] Supply totale : 100 milliards de VinX, **immuable, sans burn** (précision : 18 décimales) — *conservé en v5*
- [x] **Genèse** : 1 Md (1 %) forgé au fondateur · 99 Md (99 %) dans **La Fonderie** — *v5 : plus de pre-mine, 100 % en Fonderie*
- [x] **Invariant vérifié à chaque bloc** : `circulating_supply + foundry == 100 Md` — *conservé en v5*
- [x] **Melt** : 100 % des frais fondent dans La Fonderie (`melt_to_foundry`) — *v5 : frais 100 % au validateur, plus de melt*
- [x] **Forge** : récompenses de staking (`FORGE_RATE_BPS = 10`, toutes les 100 blocs) — *v5 : supprimé, remplacé par l'émission par le travail (halving 8 ans)*
- [x] **Warm-up de staking** (`STAKE_WARMUP_BLOCKS = 100`) — *v5 : supprimé (plus de récompense de staking)*

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
| `foundry` | `Amount` | **La Fonderie** — réserve (melt/forge actuel · réserve d'émission en v5). `circulation + foundry == 100 Md` |
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

Le multiplicateur de congestion (×1–3) est **conservé en v5** — il est basé sur la demande, pas sur la valeur.

> **v5 :** deux changements. (1) Le frais devient un **forfait × poids(type)** (indépendant du montant), au lieu de 0,05 % du montant. (2) Le frais va **100 % au validateur producteur**, plus de melt. Le multiplicateur s'applique désormais au `base_fee` **gouvernable** courant (correction : le code actuel repart de la constante `DEFAULT_FEE_FLOOR_ATOMS`).

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

### Le cycle melt / forge — ⚠️ modèle actuel, remplacé en v5

> **Code actuel.** VinX n'a aucun burn : les frais **fondent** dans La Fonderie (`melt_to_foundry`) et des récompenses de staking en sont **forgées** (`FORGE_RATE_BPS = 10`, toutes les 100 blocs). L'invariant `circulation + Fonderie = 100 Md` tient à chaque bloc.
>
> **En v5 (fair launch), ce cycle est remplacé** par :
> - **Émission par le travail** : La Fonderie se vide *uniquement* pour rémunérer la production de blocs, selon une décroissance exponentielle (halving 8 ans, `débit(t) = R₀·2^(−t/8 ans)`, R₀ ≈ 8,66 Md/an), calculée sur les **timestamps**. La Fonderie ne se recharge plus (plus de melt) — elle décroît monotone jusqu'à la poussière, puis c'est **fees-only**.
> - **Frais 100 % au producteur** : plus de melt, le frais change simplement de main.
> - L'invariant `circulation + Fonderie = 100 Md` reste vrai, trivialement.

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
| ✅ Fait | **Bascule fair launch (v5)** | **Implémentée et testée** (voir décomposition ci-dessous). Reste différé : préavis d'upgrade en temps réel, cosmétique UI admin / SDK. |
| 🔴 Haute | **Usage réel** | Faire tourner la chaîne, amorcer la micro-économie par l'émission, premiers usages |
| 🟡 Moyenne | **Run 3 validateurs** | Valider co-signing / quorum / tolérance de panne en réel (le P2P existe, testé à 1) |
| 🟢 Future | **Surcouches / modules** | Monnaie pure + modules hors-nœud par **ancrage bondé** (token factory, traçabilité…). Design gravé dans [ADR 0001](./docs/adr/0001-l1-monnaie-pure-modules-ancrage-bonde.md). Socle déjà présent : bond/slashing, Merkle, payload générique. |
| 🟢 Future | **Exécution parallèle** | Pertinent seulement à des dizaines de milliers de TPS soutenus — chantier d'architecture, risque de déterminisme |
| 🟢 Future | **TLS natif** (rustls) | HTTPS sur le RPC sans dépendance à un reverse-proxy |

### Décomposition du chantier fair launch (v5) — ✅ étapes 1-6 & 8 faites

Les étapes suivantes sont **implémentées et testées** ; l'étape 7 (préavis d'upgrade en temps réel) est **différée** avec la cosmétique UI admin / SDK.

1. **Réparer le slashing** (bug de sécurité préexistant) : `SlashEvidence` porte les deux `BlockHeader` signés ; `apply_slash_validator` vérifie réellement les deux signatures Ed25519 (aujourd'hui aucune n'est vérifiée — n'importe qui peut faire slasher un validateur).
2. **Genèse sans pre-mine** : retirer `FOUNDER_ALLOCATION_ATOMS` ; `foundry = MAX_SUPPLY`, `circulating = 0` ; validateur genesis dispensé de bond.
3. **Émission par le travail** : champ `last_emission_ts` (persisté) ; `emit(elapsed)` forge l'intégrale de `R₀·2^(−t/8 ans)` sur `[last, timestamp]`, créditée au producteur ; seuil de poussière → fees-only. Supprimer `distribute_staking_rewards`, `FORGE_RATE_*`, `STAKING_DISTRIBUTION_INTERVAL`, `STAKE_WARMUP_BLOCKS`.
4. **Bornes de timestamp** dans `validate_block` (monotonie + plafond horloge+tolérance) — requis puisque l'émission fait confiance au timestamp.
5. **Frais forfaitaires au producteur** : `calculate_fee` → forfait × poids ; `apply_transfer` accumule le frais dans un compteur de bloc au lieu de `melt_to_foundry` ; `produce_block` crédite le producteur. Corriger `update_base_fee` (partir du `fee_floor` gouvernable).
6. **Bond de validateur** : `MIN_VALIDATOR_BOND_ATOMS` (gouvernable) ; `AddValidator` exige le bond (genesis dispensé) ; `apply_unstake` → file de déliaison `Vec<(Amount, u64_timestamp)>` avec `UNBONDING_SECS = 3 j` ; maturation par timestamp ; l'unbond en attente rejoint `has_pending_time_sensitive_ops`.
7. **Préavis d'upgrade en temps réel** : `UPGRADE_NOTICE_*` en secondes au lieu de blocs.
8. **Tests + SDK + genesis** : réécrire les tests économiques ; nouvelle genèse (schéma stockage bumpé).

> **Fait cette itération** : refonte économique fair launch v5 **implémentée et testée** (genèse sans pre-mine, émission par le travail avec halving 8 ans, frais forfaitaires au producteur, bond de validateur + déliaison temps réel, slashing réparé avec vérification cryptographique), **plus** l'alignement complet de la documentation (whitepaper v4.0, README, GUIDE, GETTING_STARTED, ce document).
>
> **Itérations précédentes** : warm-up de staking, console admin `/admin`, robustesse au démarrage, clés typées + ahash, capacités relevées (10k tx/bloc, mempool 100k), cadence de bloc adaptative à la demande, migration de schéma sans wipe.

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
