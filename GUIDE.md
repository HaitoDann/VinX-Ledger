# VinX Ledger — Guide d'utilisation

## Sommaire

1. [Démarrage rapide](#1-démarrage-rapide)
2. [Wallet — commandes de base](#2-wallet--commandes-de-base)
3. [Staking — bond de validateur](#3-staking--bond-de-validateur)
4. [Admin — mise à jour du protocole](#4-admin--mise-à-jour-du-protocole)
5. [Explorateur via le wallet CLI](#5-explorateur-via-le-wallet-cli)
6. [Interface web (port 8545)](#6-interface-web-port-8545)
7. [Configuration avancée du nœud](#7-configuration-avancée-du-nœud)
8. [Réseau P2P multi-nœuds](#8-réseau-p2p-multi-nœuds)
9. [Référence des endpoints RPC](#9-référence-des-endpoints-rpc)

---

## 1. Démarrage rapide

```bash
git clone https://github.com/HaitoDann/VinX-Ledger.git
cd VinX-Ledger

# Compiler tout
cargo build --release

# Lancer le nœud (crée devnet/ avec les clés au premier démarrage)
cargo run -p vinx-node

# Dans un autre terminal — créer un wallet utilisateur
cargo run -p vinx-wallet -- keygen --output my-wallet.json

# Vérifier le solde du wallet admin (adresse dans devnet/admin.json)
cargo run -p vinx-wallet -- balance <ADRESSE_ADMIN>
```

L'interface web est disponible sur **http://localhost:8545**.

---

## 2. Wallet — commandes de base

### Générer un nouveau wallet

```bash
cargo run -p vinx-wallet -- keygen --output my-wallet.json
```

Affiche l'adresse (`vinx1…`) et sauvegarde la clé secrète dans `my-wallet.json`.  
⚠️ Ce fichier contient votre clé secrète — ne le partagez jamais.

### Afficher l'adresse d'un wallet

```bash
cargo run -p vinx-wallet -- address --wallet my-wallet.json
```

### Vérifier un solde

```bash
cargo run -p vinx-wallet -- balance vinx1abc...xyz
```

Affiche : solde, montant staké, nonce.

### Envoyer des VINX

```bash
cargo run -p vinx-wallet -- transfer \
  --wallet my-wallet.json \
  --to vinx1destinataire... \
  --amount 100.50
```

Le fee est un **forfait** (0,0001 VINX × poids × congestion), **indépendant du montant** — envoyer 1 ou 1 000 000 VINX coûte la même chose. Il est calculé automatiquement et va **intégralement au validateur qui produit le bloc**.

### Statut du nœud

```bash
cargo run -p vinx-wallet -- status
```

---

## 3. Staking — bond de validateur

> **Le staking n'est pas un placement à rendement.** En PoS permissionless, la sécurité vient du bond économique des validateurs, pas de leur identité. Le stake sert donc uniquement de **caution** (bond) : la peau dans le jeu qu'un validateur perd s'il triche. Un détenteur lambda ne stake pas — il garde son VINX pour **l'utiliser comme cash**. Il n'y a **aucune récompense de staking** : les validateurs sont rémunérés par leur **travail** (émission + frais), pas par leur bond.

**En pratique :**
- **Bond minimum : 100 000 VINX** (gouvernable) pour être éligible au set des validateurs. Le validateur défini à la genèse est dispensé (bootstrap).
- **Aucun rendement** sur le bond.
- **Déliaison différée : 3 jours de temps réel.** Le retrait n'est pas instantané — les fonds restent saisissables pendant la fenêtre où une preuve d'équivocation peut émerger.
- **Slashing** : équivocation prouvée → 100 % du bond (10 % de prime au rapporteur, le reste versé dans le pot d'époque (redistribué aux validateurs honnêtes, ADR 0028) ; downtime → suspension du round-robin, sans slash économique.

### Poser un bond (staker)

```bash
cargo run -p vinx-wallet -- stake \
  --wallet my-wallet.json \
  --amount 100000
```

### Retirer son bond (unstake — déliaison 3 jours)

```bash
cargo run -p vinx-wallet -- unstake \
  --wallet my-wallet.json \
  --amount 100000
```

Les fonds reviennent sur le solde **après la période de déliaison** (3 jours de temps réel), pas immédiatement.

---

## 4. Admin — mise à jour du protocole

> **Console d'admin web** : la page **http://localhost:8545/admin** offre un tableau de bord (hauteur, mempool, émission, version), la gestion des validateurs (ajout/retrait, approbation des demandes), la planification d'upgrades et la maintenance — le tout signé localement avec la clé admin. La page reste en lecture seule tant que la clé de l'admin on-chain n'est pas chargée. Les commandes CLI ci-dessous restent équivalentes pour un usage scripté.

Les mises à jour de protocole nécessitent un préavis minimum. La **cible** est un délai en temps réel ; l'implémentation actuelle l'applique encore en hauteur de bloc (migration vers les timestamps planifiée, cf. `ETAT_DU_PROJET.md` §8) :

| Type | Préavis minimum (cible) |
|---|---|
| Patch (x.y.**Z**) | 7 jours |
| Minor (x.**Y**.0) | 30 jours |
| Major (**X**.0.0) | 90 jours |

### Annoncer une mise à jour

```bash
cargo run -p vinx-wallet -- announce-upgrade \
  --wallet devnet/admin.json \
  --version 1.1.0 \
  --activation-ts 1793000000
```

`--activation-ts` est un **timestamp Unix en secondes** (ADR 0006). La mise à jour s'active
automatiquement dès que l'horloge des blocs atteint ce timestamp, à condition que le préavis
minimal en temps réel soit respecté (patch ≥ 7 j, minor ≥ 30 j, major ≥ 90 j).

### Vérifier l'état du protocole

```bash
cargo run -p vinx-wallet -- protocol
```

---

## 5. Explorateur via le wallet CLI

### Détails d'un bloc

```bash
cargo run -p vinx-wallet -- block 42
```

### Détails d'une transaction

```bash
cargo run -p vinx-wallet -- tx a1b2c3d4...
```

Affiche : type, bloc, de, vers, montant, fee, nonce.

### Liste des validateurs

```bash
cargo run -p vinx-wallet -- validators
```

Affiche le nombre de validateurs, le quorum requis et la liste des adresses.

---

## 6. Interface web (port 8545)

Deux pages sont servies par le nœud :

**`http://localhost:8545/` — Explorateur + wallet**
- **Réseau** : hauteur de bloc, statut, mempool, émission — en temps réel (SSE)
- **Wallet** : chargez votre `.json` — la clé ne quitte jamais le navigateur (Ed25519 local)
- **Envoyer / Staker** : transfer, stake, unstake depuis l'interface
- **Compte** : consulter n'importe quelle adresse · **Explorateur de blocs** · recherche par hash
- **Validateurs** et **Protocole** en temps réel

**`http://localhost:8545/admin` — Console d'administration**
- Tableau de bord (hauteur, mempool, émission, circulation, version de protocole)
- Validateurs : ensemble actif, ajout/retrait, approbation des demandes en attente
- Mises à jour : planification d'upgrade · Maintenance : compactage, faucet
- Actions signées localement avec la clé admin ; lecture seule tant que la clé admin n'est pas chargée

---

## 7. Configuration avancée du nœud

```bash
cp config.example.toml config.toml
# Éditez config.toml
cargo run -p vinx-node -- --config config.toml
```

Les flags CLI ont priorité sur le fichier de config :

```bash
cargo run -p vinx-node -- \
  --block-time 10 \
  --rpc-listen 0.0.0.0:9000 \
  --data-dir /var/lib/vinx
```

### Options disponibles

| Option CLI | Config TOML | Défaut |
|---|---|---|
| `--block-time` | `block_time_secs` | 12 (fixe) |
| — | `max_block_txs` | 3000 |
| — | `max_mempool_size` | 100000 |
| `--rpc-listen` | `rpc_listen` | `0.0.0.0:8545` |
| `--p2p-listen` | `p2p_listen` | désactivé |
| `--data-dir` | `data_dir` | `devnet` |
| — | `validator_key_file` | `devnet/validator.json` |
| — | `admin_key_file` | `devnet/admin.json` |
| — | `admin_token` | aucun (Bearer pour `/validators/pending`, `/admin/compact`) |
| `--peers` | `peers` | aucun |

---

## 8. Réseau P2P multi-nœuds

### Nœud 1

```toml
# config-node1.toml
data_dir        = "node1"
rpc_listen      = "0.0.0.0:8545"
p2p_listen      = "/ip4/0.0.0.0/tcp/9000"
block_time_secs = 10
```

### Nœud 2 (se connecte au nœud 1)

```toml
# config-node2.toml
data_dir        = "node2"
rpc_listen      = "0.0.0.0:8546"
p2p_listen      = "/ip4/0.0.0.0/tcp/9001"
block_time_secs = 10
peers = ["/ip4/127.0.0.1/tcp/9000"]
```

Les blocs sont propagés via gossipsub. Chaque validateur co-signe les blocs. Le quorum est `⌈2n/3⌉` parmi `n` validateurs.

---

## 9. Référence des endpoints RPC

| Méthode | Chemin | Description |
|---|---|---|
| GET | `/health` | Statut, hauteur, mempool, chain_id |
| GET | `/chain/height` | Hauteur de la chaîne |
| GET | `/chain/sync?from=N&limit=N` | Synchronisation d'une plage de blocs |
| GET | `/block/:height` | Détails d'un bloc |
| GET | `/account/:address` | Solde et infos d'un compte |
| GET | `/account/:address/txs` | Historique des transactions (paginé) |
| GET | `/account/:address/proof` | Preuve Merkle d'inclusion |
| POST | `/tx/submit` | Soumettre une transaction |
| POST | `/tx/batch` | Soumettre un lot (≤ 100) |
| GET | `/tx/:hash` | Détails d'une transaction |
| GET | `/tx/:hash/receipt` | Reçu d'exécution d'une transaction |
| GET | `/mempool/size` | Taille du mempool |
| GET | `/validators` | Ensemble des validateurs (+ liveness) |
| GET | `/validators/pending` | Demandes de validateur en attente (token admin) |
| GET | `/protocol/version` | Version du protocole + upgrade en attente |
| GET | `/network/stats` | base_fee, émission, circulation, adresse admin |
| GET | `/metrics` | Métriques Prometheus |
| GET | `/events` | Server-Sent Events (push par bloc) |
| GET/POST | `/snapshot` | Export/import de l'état complet (token admin) |
| POST | `/faucet/request` | Demander des tokens (si faucet activé) |
| POST | `/admin/compact` | Compacter le stockage (token admin) |
