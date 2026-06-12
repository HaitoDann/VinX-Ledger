# VinX Ledger — Guide d'utilisation

## Sommaire

1. [Démarrage rapide](#1-démarrage-rapide)
2. [Wallet — commandes de base](#2-wallet--commandes-de-base)
3. [Staking](#3-staking)
4. [Admin — gel et dégel de comptes](#4-admin--gel-et-dégel-de-comptes)
5. [Admin — mise à jour du protocole](#5-admin--mise-à-jour-du-protocole)
6. [Explorateur via le wallet CLI](#6-explorateur-via-le-wallet-cli)
7. [Interface web (port 8545)](#7-interface-web-port-8545)
8. [Configuration avancée du nœud](#8-configuration-avancée-du-nœud)
9. [Réseau P2P multi-nœuds](#9-réseau-p2p-multi-nœuds)
10. [Référence des endpoints RPC](#10-référence-des-endpoints-rpc)

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

Affiche : solde, montant staké, nonce, statut (actif / gelé).

### Envoyer des VINX

```bash
cargo run -p vinx-wallet -- transfer \
  --wallet my-wallet.json \
  --to vinx1destinataire... \
  --amount 100.50
```

Le fee (0,05% avec plancher 0,0001 VINX) est calculé automatiquement.

### Statut du nœud

```bash
cargo run -p vinx-wallet -- status
```

---

## 3. Staking

Les stakers reçoivent 80% des frais collectés, distribués toutes les 100 blocs.

### Staker des VINX

```bash
cargo run -p vinx-wallet -- stake \
  --wallet my-wallet.json \
  --amount 5000
```

Minimum : 1 VINX. Les VINX stakés sont bloqués jusqu'à unstake.

### Récupérer des VINX stakés

```bash
cargo run -p vinx-wallet -- unstake \
  --wallet my-wallet.json \
  --amount 5000
```

---

## 4. Admin — gel et dégel de comptes

Seul le wallet admin (créé au genesis dans `devnet/admin.json`) peut geler ou dégeler des comptes.

### Geler un compte

```bash
cargo run -p vinx-wallet -- freeze \
  --wallet devnet/admin.json \
  --target vinx1compte_a_geler...
```

Un compte gelé ne peut plus émettre de transactions. Le gel est automatiquement levé après **12 mois** (~3 153 600 blocs à 10s/bloc).

### Dégeler un compte manuellement

```bash
cargo run -p vinx-wallet -- unfreeze \
  --wallet devnet/admin.json \
  --target vinx1compte_a_degeler...
```

---

## 5. Admin — mise à jour du protocole

Les mises à jour de protocole nécessitent un préavis minimum :

| Type | Préavis minimum |
|---|---|
| Patch (x.y.**Z**) | 7 jours (~60 480 blocs) |
| Minor (x.**Y**.0) | 30 jours (~259 200 blocs) |
| Major (**X**.0.0) | 90 jours (~777 600 blocs) |

### Annoncer une mise à jour

```bash
cargo run -p vinx-wallet -- announce-upgrade \
  --wallet devnet/admin.json \
  --version 1.1.0 \
  --activation-height 300000
```

La mise à jour s'active automatiquement au bloc indiqué.

### Vérifier l'état du protocole

```bash
cargo run -p vinx-wallet -- protocol
```

---

## 6. Explorateur via le wallet CLI

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

## 7. Interface web (port 8545)

Ouvrez **http://localhost:8545** dans votre navigateur.

- **Réseau** : hauteur de bloc, statut, mempool — rafraîchi toutes les 3 s
- **Wallet** : chargez votre `.json` — la clé ne quitte jamais le navigateur (Ed25519 local)
- **Envoyer / Staker** : transfer, stake, unstake depuis l'interface
- **Compte** : consulter n'importe quelle adresse
- **Explorateur de blocs** : state root, signatures, finalisation
- **Transaction** : recherche par hash hexadécimal
- **Validateurs** : liste en temps réel
- **Protocole** : version actuelle et upgrade en attente

---

## 8. Configuration avancée du nœud

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
| `--block-time` | `block_time_secs` | 3 |
| — | `max_block_txs` | 1000 |
| `--rpc-listen` | `rpc_listen` | `0.0.0.0:8545` |
| `--p2p-listen` | `p2p_listen` | désactivé |
| `--data-dir` | `data_dir` | `devnet` |
| — | `validator_key_file` | `devnet/validator.json` |
| — | `admin_key_file` | `devnet/admin.json` |
| `--peers` | `peers` | aucun |

---

## 9. Réseau P2P multi-nœuds

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

## 10. Référence des endpoints RPC

| Méthode | Chemin | Description |
|---|---|---|
| GET | `/health` | Statut, hauteur, mempool |
| GET | `/chain/height` | Hauteur de la chaîne |
| GET | `/block/:height` | Détails d'un bloc |
| GET | `/account/:address` | Solde et infos d'un compte |
| POST | `/tx/submit` | Soumettre une transaction |
| GET | `/tx/:hash` | Détails d'une transaction |
| GET | `/mempool/size` | Taille du mempool |
| GET | `/validators` | Ensemble des validateurs |
| GET | `/protocol/version` | Version du protocole |
