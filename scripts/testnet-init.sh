#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# VinX Testnet — Initialisation Docker 3 nœuds
#
# Génère les clés du validateur genesis, la spec de genèse partagée, et les
# config.toml. Doit être lancé une seule fois avant le premier
# "docker compose -f docker-compose.testnet.yml up".
#
# Stratégie clés : on démarre node1 brièvement pour qu'il auto-génère ses
# validator.json et admin.json, puis on l'arrête, on nettoie l'état de
# genèse provisoire et on écrit la spec partagée (même approche que bench-n3.sh).
#
# Prérequis : Docker installé, jq installé (apt install -y jq), openssl.
# Usage     : ./scripts/testnet-init.sh [dossier-deploy]
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

DEPLOY="${1:-./deploy}"
CHAIN_ID=7
GENESIS_TS=1000           # timestamp fixe → genèse déterministe identique sur tous les nœuds
PREFUND_VINX=1000000      # 1M VinX pré-financés sur le validateur genesis (pour le faucet)
FAUCET_AMOUNT_ATOMS=100000000000000000000   # 100 VinX par requête faucet
FAUCET_COOLDOWN_SECS=3600                   # 1h entre deux requêtes (adapté au testnet)

# Admin token aléatoire (32 hex chars)
ADMIN_TOKEN="${ADMIN_TOKEN:-$(openssl rand -hex 16)}"

# ─── Couleurs ────────────────────────────────────────────────────────────────
G="\033[0;32m"; B="\033[0;34m"; Y="\033[0;33m"; R="\033[0;31m"; N="\033[0m"
header() { echo -e "\n${B}══ $1${N}"; }
ok()     { echo -e "  ${G}✓${N}  $1"; }
warn()   { echo -e "  ${Y}⚠${N}  $1"; }
err()    { echo -e "  ${R}✗${N}  $1"; exit 1; }

echo -e "\n${B}╔══════════════════════════════════════════════╗"
echo -e "║   VinX Testnet — Initialisation (chain 7)   ║"
echo -e "╚══════════════════════════════════════════════╝${N}"

# ─── Prérequis ───────────────────────────────────────────────────────────────
header "Vérification des prérequis"
command -v docker  &>/dev/null || err "Docker non trouvé. Installez-le d'abord."
command -v jq      &>/dev/null || err "jq non trouvé. Installez-le : apt install -y jq"
command -v openssl &>/dev/null || err "openssl non trouvé."
ok "Docker, jq et openssl disponibles"

# ─── Si deploy/ existe déjà ──────────────────────────────────────────────────
if [[ -d "$DEPLOY" ]]; then
    warn "Le dossier $DEPLOY existe déjà."
    read -r -p "  Réinitialiser ? Cela effacera les clés et données existantes. [y/N] " yn
    [[ "${yn,,}" == "y" ]] || { echo "  Annulé."; exit 0; }
    rm -rf "$DEPLOY"
fi
mkdir -p "$DEPLOY"/{node1,node2,node3}
DEPLOY_ABS="$(cd "$DEPLOY" && pwd)"

# ─── Build de l'image ────────────────────────────────────────────────────────
header "Build de l'image Docker vinx-testnet"
echo "  (peut prendre 5-10 min au premier lancement — le cache accélère les suivants)"
docker build -t vinx-testnet . --quiet
ok "Image vinx-testnet construite"

# ─── Phase 0 : démarrer node1 brièvement pour auto-générer ses clés ──────────
header "Génération des clés de genèse (node1)"
echo "  → démarrage temporaire de node1..."

# Nettoyer un éventuel conteneur zombie du même nom
docker rm -f vinx-keygen-tmp >/dev/null 2>&1 || true

docker run --name vinx-keygen-tmp -d \
    -v "${DEPLOY_ABS}/node1:/data" \
    -e RUST_LOG=warn \
    vinx-testnet \
    --data-dir=/data --rpc-listen=127.0.0.1:8545 --block-time=3 \
    >/dev/null

# Attendre que validator.json et admin.json soient générés
echo "  → attente des clés..."
for _ in $(seq 1 150); do
    [[ -f "$DEPLOY/node1/validator.json" && -f "$DEPLOY/node1/admin.json" ]] && break
    sleep 0.2
done

docker stop  vinx-keygen-tmp >/dev/null 2>&1 || true
docker rm -f vinx-keygen-tmp >/dev/null 2>&1 || true

[[ -f "$DEPLOY/node1/validator.json" ]] || err "validator.json non généré (timeout ou erreur Docker)"
[[ -f "$DEPLOY/node1/admin.json"     ]] || err "admin.json non généré (timeout ou erreur Docker)"

# Nettoyer l'état de genèse provisoire — garder seulement les .json
find "$DEPLOY/node1" -type f ! -name '*.json' -delete 2>/dev/null || true
find "$DEPLOY/node1" -type d -empty -delete   2>/dev/null || true

N1_VALIDATOR=$(jq -r .address "$DEPLOY/node1/validator.json")
N1_ADMIN=$(jq -r     .address "$DEPLOY/node1/admin.json")
ok "Node 1 validator : $N1_VALIDATOR"
ok "Node 1 admin     : $N1_ADMIN"

# ─── Générer la clé faucet via vinx-wallet keygen ────────────────────────────
header "Génération de la clé faucet"
docker run --rm \
    --entrypoint /usr/local/bin/vinx-wallet \
    -v "${DEPLOY_ABS}/node1:/out" \
    vinx-testnet \
    keygen --output "/out/faucet.json" 2>/dev/null
[[ -f "$DEPLOY/node1/faucet.json" ]] || err "faucet.json non généré"
N1_FAUCET=$(jq -r .address "$DEPLOY/node1/faucet.json")
ok "Faucet : $N1_FAUCET"

# ─── Genesis.json partagée (déterministe, identique sur les 3 nœuds) ─────────
header "Création de la genèse partagée"
cat > "$DEPLOY/genesis.json" <<EOF
{
  "chain_id": ${CHAIN_ID},
  "genesis_timestamp": ${GENESIS_TS},
  "admin_address": "${N1_ADMIN}",
  "initial_validator": "${N1_VALIDATOR}",
  "prefund_initial_validator_vinx": ${PREFUND_VINX}
}
EOF
ok "deploy/genesis.json créé (ts=${GENESIS_TS}, chain_id=${CHAIN_ID})"

# ─── Config node1 : faucet + admin token ─────────────────────────────────────
header "Configuration des nœuds"
cat > "$DEPLOY/node1/config.toml" <<EOF
# VinX Testnet — Node 1 (validateur genesis + faucet)
chain_id = ${CHAIN_ID}
admin_token = "${ADMIN_TOKEN}"

faucet_key_file      = "/data/faucet.json"
faucet_amount_atoms  = ${FAUCET_AMOUNT_ATOMS}
faucet_cooldown_secs = ${FAUCET_COOLDOWN_SECS}
EOF
ok "deploy/node1/config.toml créé"

# ─── Résumé ──────────────────────────────────────────────────────────────────
echo -e "\n${G}╔══════════════════════════════════════════════════╗"
echo -e "║       Initialisation terminée avec succès !      ║"
echo -e "╚══════════════════════════════════════════════════╝${N}"
echo ""
echo "  Chain ID          : ${CHAIN_ID} (testnet)"
echo "  Genesis timestamp : ${GENESIS_TS} (fixe, déterministe)"
echo "  Validateur genesis: ${N1_VALIDATOR}"
echo "  Admin             : ${N1_ADMIN}"
echo "  Faucet            : ${N1_FAUCET}"
echo ""
echo -e "  ${Y}⚠  Sauvegardez l'admin token — il n'est pas stocké ailleurs :${N}"
echo -e "  ${Y}   ${ADMIN_TOKEN}${N}"
echo ""
echo "  ─── Étapes suivantes ────────────────────────────────────────"
echo ""
echo "  1. Démarrer les 3 nœuds :"
echo -e "     ${B}docker compose -f docker-compose.testnet.yml up -d${N}"
echo ""
echo "  2. Vérifier que les nœuds sont sains (attendre ~30s) :"
echo -e "     ${B}curl http://localhost:8545/health | jq .${N}"
echo ""
echo "  3. Alimenter le faucet (depuis node1) :"
echo -e "     ${B}docker exec vinx-node1 /usr/local/bin/vinx-wallet transfer \\${N}"
echo -e "     ${B}  --wallet /data/validator.json --to ${N1_FAUCET} \\${N}"
echo -e "     ${B}  --amount 500000 --node http://localhost:8545${N}"
echo ""
echo "  4. Explorateur : ouvrir http://localhost:8545 dans un navigateur"
echo ""
