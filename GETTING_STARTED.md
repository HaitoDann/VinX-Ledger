# VinX Ledger — Guide de démarrage rapide

## ⚡ Tester à la maison en une commande

```bash
git clone https://github.com/HaitoDann/vinx-ledger && cd vinx-ledger
./vinx start          # compile, crée un réseau local, lance un validateur
```

Ouvrir **http://127.0.0.1:8545/** puis :
1. **Nouveau portefeuille** : la clé est créée dans le navigateur et un fichier de
   sauvegarde est téléchargé. Gardez-le, il permet de recharger le portefeuille.
2. **Obtenir** (faucet) : 100 VINX de test arrivent au bloc suivant.
3. **Envoyer** : vers une adresse `vinx1…` ou `nom@domaine`, avec un mémo optionnel.

| Commande | Effet |
|---|---|
| `./vinx start -n 4` | 4 validateurs (tolère 1 panne) — après `./vinx reset` |
| `./vinx status` | hauteur de chaque nœud, quorum |
| `./vinx kill 3` / `./vinx revive 3` | simuler une panne, puis relancer |
| `./vinx start --lan` | accessible depuis les autres appareils du réseau local (téléphone…) |
| `./vinx wallet balance vinx1…` | le wallet en ligne de commande, branché sur le réseau local |
| `./vinx logs 2` | journal du nœud 2 |
| `./vinx stop` / `./vinx reset` | arrêter (données gardées) / tout effacer |

Prérequis : Rust (rustup.rs), `python3`, `curl`. Tout fonctionne hors ligne : l'interface
n'utilise aucune ressource externe.

---

> **Prérequis système**
> - Rust ≥ 1.78 → [rustup.rs](https://rustup.rs)
> - Git
> - `curl` ou un client HTTP (HTTPie, Postman…)
> - Docker + Docker Compose *(uniquement pour le testnet multi-nœuds)*

---

## Partie 1 — Compiler le projet

```bash
git clone https://github.com/HaitoDann/vinx-ledger
cd vinx-ledger

# Compiler les deux binaires
cargo build --release -p vinx-node
cargo build --release -p vinx-wallet

# Les binaires se trouvent ici :
./target/release/vinx-node   --version
./target/release/vinx-wallet --version
```

> **Astuce** : ajouter `./target/release` au `PATH` pour éviter de préfixer chaque commande.
> ```bash
> export PATH="$PWD/target/release:$PATH"
> ```

---

## Partie 2 — Lancer un premier nœud solo

### 2.1 Démarrage minimal

```bash
vinx-node \
  --data-dir ./data-node1 \
  --rpc-listen 127.0.0.1:8545 \
  --block-time 3
```

Le nœud :
- génère automatiquement une clé validateur dans `./data-node1/`
- expose le RPC sur `http://127.0.0.1:8545`
- produit des blocs à **cadence fixe 12 s** (ADR 0045) : un bloc toutes les 12 s, vide ou non — la congestion passe par le base-fee dynamique

La bannière de démarrage affiche l'adresse du validateur et la commande curl de base.

### 2.2 Vérifier que le nœud tourne

```bash
# Santé : statut, hauteur, mempool, chain_id
curl http://127.0.0.1:8545/health | jq

# Statistiques économiques : base_fee, émission, circulation, admin
curl http://127.0.0.1:8545/network/stats | jq
```

Réponse attendue de `/health` :
```json
{ "status": "ok", "height": 12, "mempool_pending": 0, "chain_id": 42 }
```

Réponse attendue de `/network/stats` (au démarrage : **aucun pre-mine** — 0 émis, 0 en circulation) :
```json
{
  "base_fee_atoms": "100000000000000",
  "emitted_atoms": "0",
  "circulating_supply": "0.00 VINX",
  "admin_address": "vinx1..."
}
```

> Les VINX apparaissent en circulation **au fur et à mesure** que les validateurs produisent des blocs (émission par le travail — minting progressif, ADR 0040). Il n'y a pas de réserve pré-allouée : les tokens n'existent pas avant d'être mintés.

### 2.3 Activer le faucet (optionnel pour tests)

Créer un fichier `config.toml` :

```toml
# config.toml
data_dir          = "./data-node1"
rpc_listen        = "127.0.0.1:8545"
block_time_secs   = 3

# Faucet — génère la clé automatiquement au premier démarrage
faucet_key_file       = "./data-node1/faucet.json"
faucet_amount_atoms   = 100_000_000_000_000_000_000   # 100 VinX
faucet_cooldown_secs  = 60                             # 1 min en devnet
```

Lancer avec la config :

```bash
vinx-node --config config.toml
```

La bannière affiche maintenant l'adresse du faucet et la commande curl pour le recharger.

> **Fair launch — le faucet démarre à zéro.** Comme il n'y a aucun pre-mine, le compte faucet n'a pas de solde initial. Il faut d'abord l'**approvisionner** : laissez le validateur produire quelques blocs (il accumule de l'émission), puis transférez des VINX de son compte vers l'adresse du faucet. Le faucet ne peut distribuer que ce qu'il détient.

---

## Partie 3 — Créer un wallet et interagir

### 3.1 Générer un wallet

```bash
# Crée wallet.json dans le dossier courant
vinx-wallet keygen --output wallet.json

# Afficher l'adresse
vinx-wallet address --wallet wallet.json
# → Address : vinx1abc...
```

### 3.2 Demander des tokens au faucet

```bash
# Récupérer ton adresse
ADDR=$(vinx-wallet address --wallet wallet.json | awk '{print $3}')

# Appeler le faucet
curl -s -X POST http://127.0.0.1:8545/faucet/request \
  -H "Content-Type: application/json" \
  -d "{\"address\": \"$ADDR\"}" | jq
```

Réponse :
```json
{
  "accepted": true,
  "tx_hash": "d4e5f6...",
  "amount_atoms": "100000000000000000000",
  "to": "vinx1abc..."
}
```

Attendre ~1 bloc (3 s), puis vérifier le solde :

```bash
vinx-wallet balance $ADDR --node http://127.0.0.1:8545
# → Balance : 100 VinX
```

### 3.3 Envoyer une transaction

```bash
# Créer un second wallet destinataire
vinx-wallet keygen --output wallet2.json
DEST=$(vinx-wallet address --wallet wallet2.json | awk '{print $3}')

# Envoyer 10 VinX
vinx-wallet transfer \
  --wallet wallet.json \
  --to $DEST \
  --amount 10 \
  --node http://127.0.0.1:8545

# Vérifier le solde du destinataire (après ~1 bloc)
vinx-wallet balance $DEST --node http://127.0.0.1:8545
```

### 3.4 Consulter l'historique d'un compte

```bash
curl http://127.0.0.1:8545/account/$ADDR | jq
```

---

## Partie 4 — Explorer la chaîne via l'API RPC

### Lire un bloc

```bash
# Dernier bloc
HEIGHT=$(curl -s http://127.0.0.1:8545/chain/height | jq .height)
curl http://127.0.0.1:8545/block/$HEIGHT | jq
```

### Lire une transaction

```bash
# Hash récupéré depuis le retour de `transfer` ou dans un bloc
curl http://127.0.0.1:8545/tx/<HASH> | jq
```

### Vérifier une preuve Merkle

```bash
curl http://127.0.0.1:8545/account/$ADDR/proof | jq
```

Réponse :
```json
{
  "state_root": "a3f1...",
  "proof": [
    { "sibling": "b2c3...", "sibling_is_right": true }
  ]
}
```

### Métriques Prometheus

```bash
curl http://127.0.0.1:8545/metrics
```

```
vinx_chain_height 42
vinx_mempool_size 0
vinx_base_fee 100000000000000
vinx_emitted_atoms <tokens émis depuis la genèse, croît avec le temps>
vinx_circulating_supply <part déjà émise aux validateurs, croît avec le temps>
vinx_validator_count 1
vinx_blocks_produced_total 42
vinx_tx_submitted_total{status="ok"} 5
vinx_tx_in_block_total 5
```

> `vinx_emitted_atoms` et `vinx_circulating_supply` évoluent avec l'**émission par le travail** (calculée sur le temps réel écoulé). Invariant garanti : `circulating + epoch_pot + destroyed = emitted ≤ 1 Md`.

---

## Partie 5 — Block explorer (UI)

Ouvrir dans un navigateur : **http://127.0.0.1:8545/**

Fonctionnalités :
- Statut réseau en temps réel (SSE)
- Carte économie : base_fee, supply circulante, émission progressive, faucet intégré
- Graphique des frais des 30 derniers blocs
- Recherche universelle (adresse / hash de TX / numéro de bloc)
- Historique de TX paginé par compte
- Wallet web : signature Ed25519 **locale** (la clé ne quitte jamais le navigateur)

**Console d'admin** : **http://127.0.0.1:8545/admin** — dashboard, gestion des validateurs, upgrades et maintenance, signés localement avec la clé admin (lecture seule sinon).

---

## Partie 6 — Testnet 3 validateurs (Docker)

```bash
# Lancer le testnet complet + Caddy TLS + Grafana
docker compose up --build

# En arrière-plan
docker compose up --build -d
```

| Service | URL |
|---|---|
| Nœud 1 RPC (direct) | http://localhost:8545 |
| Nœud 2 RPC (direct) | http://localhost:8546 |
| Nœud 3 RPC (direct) | http://localhost:8547 |
| Explorer + faucet (TLS) | https://localhost |
| Prometheus | http://localhost:9090 |
| Grafana | http://localhost:3000 (admin / vinxadmin) |

> Le certificat Caddy est auto-signé en local. Accepter l'alerte navigateur ou :
> ```bash
> caddy trust   # installe le CA Caddy dans le store système
> ```

### Activer le faucet sur le testnet Docker

```bash
# 1. Générer la clé faucet
vinx-wallet keygen --output deploy/faucet.json

# 2. Créer deploy/node1.toml
cat > deploy/node1.toml <<EOF
faucet_key_file      = "/data/faucet.json"
faucet_amount_atoms  = 100000000000000000000
faucet_cooldown_secs = 60
EOF

# 3. Décommenter la ligne dans docker-compose.yml :
#    - ./deploy/node1.toml:/config.toml:ro

# 4. Relancer
docker compose up --build -d node1
```

---

## Partie 7 — Tests automatisés

### Suite complète (230 tests)

```bash
cargo test --workspace
```

### Tests d'intégration uniquement

```bash
# Tests qui démarrent un vrai nœud en mémoire
cargo test -p vinx-node --test integration
```

### Proptest (invariants de conservation)

```bash
# Invariants : conservation de supply, émission, Merkle, nonces
cargo test -p vinx-state --test property_tests
```

### Tests du SDK TypeScript

```bash
cd sdk/vinx-sdk
npm ci
npm test
```

---

## Partie 8 — Scénario de test bout en bout

Script bash reproductible pour valider l'ensemble du flux :

```bash
#!/usr/bin/env bash
set -e

NODE="http://127.0.0.1:8545"

echo "=== 1. Démarrer le nœud en arrière-plan ==="
vinx-node --data-dir /tmp/vinx-test --rpc-listen 127.0.0.1:8545 \
          --block-time 2 --config config.toml &
NODE_PID=$!
sleep 3  # laisser le nœud démarrer

echo "=== 2. Générer deux wallets ==="
vinx-wallet keygen --output /tmp/alice.json
vinx-wallet keygen --output /tmp/bob.json
ALICE=$(vinx-wallet address --wallet /tmp/alice.json | awk '{print $3}')
BOB=$(vinx-wallet address --wallet /tmp/bob.json | awk '{print $3}')
echo "Alice : $ALICE"
echo "Bob   : $BOB"

echo "=== 3. Faucet → Alice ==="
curl -s -X POST $NODE/faucet/request \
  -H "Content-Type: application/json" \
  -d "{\"address\": \"$ALICE\"}" | jq .accepted

sleep 4  # attendre 2 blocs

echo "=== 4. Vérifier le solde d'Alice ==="
vinx-wallet balance $ALICE --node $NODE

echo "=== 5. Alice envoie 25 VinX à Bob ==="
vinx-wallet transfer --wallet /tmp/alice.json \
  --to $BOB --amount 25 --node $NODE

sleep 4

echo "=== 6. Vérifier les soldes finaux ==="
echo "Alice :"
vinx-wallet balance $ALICE --node $NODE
echo "Bob :"
vinx-wallet balance $BOB --node $NODE

echo "=== 7. État de la chaîne ==="
curl -s $NODE/health | jq '{height, mempool_pending}'
curl -s $NODE/network/stats | jq '{base_fee_atoms, emitted_atoms, circulating_supply}'

echo "=== OK — test terminé ==="
kill $NODE_PID
```

---

## Référence rapide — commandes essentielles

```bash
# Nœud
vinx-node --help
vinx-node --config config.toml

# Wallet
vinx-wallet keygen    --output wallet.json
vinx-wallet address   --wallet wallet.json
vinx-wallet balance   <addr>  --node http://127.0.0.1:8545
vinx-wallet transfer  --wallet wallet.json --to <addr> --amount <n> --node ...
vinx-wallet stake     --wallet wallet.json --amount <n> --node ...
vinx-wallet unstake   --wallet wallet.json --amount <n> --node ...

# API (exemples curl)
curl http://localhost:8545/health
curl http://localhost:8545/network/stats
curl http://localhost:8545/chain/height
curl http://localhost:8545/block/<N>
curl http://localhost:8545/tx/<HASH>
curl http://localhost:8545/account/<ADDR>
curl http://localhost:8545/account/<ADDR>/txs
curl http://localhost:8545/account/<ADDR>/proof
curl http://localhost:8545/mempool/size
curl http://localhost:8545/metrics
curl -X POST http://localhost:8545/faucet/request \
     -H "Content-Type: application/json" \
     -d '{"address":"vinx1..."}'
```
