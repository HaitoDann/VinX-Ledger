#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# VinX — Banc n=3 multi-process (fiable, reproductible).
#
# Amène un vrai testnet 3 validateurs à un quorum fonctionnel, puis démontre :
#   1. rotation round-robin + finalité qui avance (co-signatures P2P réelles) ;
#   2. tolérance à 1 panne (2/3 finalise encore) ;
#   3. sûreté : à 2 pannes (1/3) la finalité STALLE, le tip continue.
#
# Deux verrous levés vs l'approche naïve :
#   - genèse identique sur les 3 nœuds via une SPEC partagée (VINX_GENESIS_SPEC) —
#     sans quoi la sync casse (elle exige prev_hash == tip dès la hauteur 1) ;
#   - bond de 100k amorcé instantanément via un PRÉ-FINANCEMENT de genèse dev-only
#     (gated non-mainnet) au lieu d'attendre ~17 min d'émission.
#
# Les clés de nœud sont en clair ({address, secret_key_hex}) et lues telles quelles
# par le wallet CLI (format legacy plaintext) — aucune passphrase.
#
# Usage :  cargo build --release -p vinx-node -p vinx-wallet && scripts/bench-n3.sh
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

NODE="./target/release/vinx-node"
WALLET="./target/release/vinx-wallet"
BASE="${BENCH_DIR:-/tmp/vinx-bench-n3}"
BLOCK_TIME="${BLOCK_TIME:-2}"
CHAIN_ID=42                 # devnet (défaut du nœud) — doit égaler spec.chain_id
GENESIS_TS=1000            # fixe → genèse déterministe identique partout
PREFUND_VINX=1000000      # pré-financement dev du validateur de genèse (node1)
BOND_VINX=100000          # bond minimal de validateur
FUND_VINX=100001          # bond + petit tampon de frais

for b in "$NODE" "$WALLET"; do
    [[ -x "$b" ]] || { echo "ERREUR: $b introuvable. Lance: cargo build --release -p vinx-node -p vinx-wallet"; exit 1; }
done

rm -rf "$BASE"; mkdir -p "$BASE"/{node1,node2,node3}
PIDS=()
cleanup() { echo; echo "=== arrêt du banc ==="; for p in "${PIDS[@]:-}"; do kill "$p" 2>/dev/null || true; done; }
trap cleanup EXIT

# ── Helpers ──────────────────────────────────────────────────────────────────
rpc() { curl -s "http://127.0.0.1:$1/$2" 2>/dev/null || true; }
jfield() { python3 -c "import sys,json
try: print(json.load(sys.stdin).get('$1',''))
except Exception: print('')"; }
addr_of() { python3 -c "import json;print(json.load(open('$1'))['address'])"; }

wait_health() { # port
    for _ in $(seq 1 100); do
        [[ "$(rpc "$1" health | jfield status)" == "ok" ]] && return 0
        sleep 0.2
    done
    echo "timeout: node port $1 pas prêt"; return 1
}
finalized() { rpc "$1" health | jfield finalized_height; }
height()    { rpc "$1" health | jfield height; }
staked_atoms() { rpc "$1" "account/$2" | jfield staked_atoms; }
balance_atoms() { rpc "$1" "account/$2" | jfield balance_atoms; }
vcount()    { rpc "$1" validators | jfield count; }

start_node() { # id rpc p2p extra...
    local id="$1" rpc_p="$2" p2p_p="$3"; shift 3
    VINX_GENESIS_SPEC="$BASE/genesis.json" "$NODE" \
        --data-dir "$BASE/node$id" --rpc-listen "127.0.0.1:$rpc_p" \
        --p2p-listen "/ip4/127.0.0.1/tcp/$p2p_p" --block-time "$BLOCK_TIME" "$@" \
        >"$BASE/node$id.log" 2>&1 &
    PIDS+=($!); eval "PID$id=$!"
    echo "  node$id démarré (PID $!) — RPC 127.0.0.1:$rpc_p"
}

# ── Phase 0 : générer les clés de node1 (pour référencer son adresse dans la spec) ──
echo "=== Phase 0 : génération des clés de genèse (node1) ==="
VINX_DEV_PREFUND_VINX=0 "$NODE" --data-dir "$BASE/node1" --rpc-listen 127.0.0.1:8545 \
    --p2p-listen /ip4/127.0.0.1/tcp/9001 --block-time "$BLOCK_TIME" >"$BASE/node1.boot.log" 2>&1 &
BOOT=$!
for _ in $(seq 1 100); do [[ -f "$BASE/node1/validator.json" && -f "$BASE/node1/admin.json" ]] && break; sleep 0.1; done
kill "$BOOT" 2>/dev/null || true; wait "$BOOT" 2>/dev/null || true
N1_VAL=$(addr_of "$BASE/node1/validator.json")
N1_ADMIN=$(addr_of "$BASE/node1/admin.json")
echo "  node1 validator : $N1_VAL"
echo "  node1 admin     : $N1_ADMIN"
# On efface l'état de chaîne de la phase 0 (genèse jetable) mais on GARDE les clés json.
find "$BASE/node1" -type f ! -name '*.json' -delete

# ── Spec de genèse partagée (déterministe, identique sur les 3 nœuds) ──
cat >"$BASE/genesis.json" <<EOF
{
  "chain_id": $CHAIN_ID,
  "genesis_timestamp": $GENESIS_TS,
  "admin_address": "$N1_ADMIN",
  "initial_validator": "$N1_VAL",
  "prefund_initial_validator_vinx": $PREFUND_VINX
}
EOF
echo "  spec écrite : $BASE/genesis.json (pré-financement $PREFUND_VINX VINX, dev)"

# ── Phase 1 : démarrer les 3 nœuds sur la genèse partagée ──
echo "=== Phase 1 : démarrage des 3 nœuds (genèse partagée) ==="
start_node 1 8545 9001
wait_health 8545
start_node 2 8546 9002 --peers /ip4/127.0.0.1/tcp/9001 --sync-peer http://127.0.0.1:8545
start_node 3 8547 9003 --peers /ip4/127.0.0.1/tcp/9001 --sync-peer http://127.0.0.1:8545
wait_health 8546; wait_health 8547
N2_VAL=$(addr_of "$BASE/node2/validator.json")
N3_VAL=$(addr_of "$BASE/node3/validator.json")
echo "  node2 validator : $N2_VAL"
echo "  node3 validator : $N3_VAL"

# ── Bootstrap du set : financer → bonder → ajouter node2 & node3 ──
echo "=== Bootstrap du set de validateurs ==="
BOND_ATOMS=$(python3 -c "print($BOND_VINX*10**18)")

# Matérialise le compte admin : sans solde, il n'a pas de compte on-chain, donc le wallet ne
# peut pas lire son nonce pour signer une action de gouvernance (les actions admin sont
# fee-exempt, mais le compte doit exister). Un petit transfert le crée.
echo "  → matérialisation du compte admin"
"$WALLET" transfer --to "$N1_ADMIN" --amount 2 \
    --wallet "$BASE/node1/validator.json" --node http://127.0.0.1:8545 >/dev/null
for _ in $(seq 1 60); do b=$(balance_atoms 8545 "$N1_ADMIN"); [[ -n "$b" && "$b" != "0" ]] && break; sleep 0.5; done
fund_and_bond() { # valaddr nodedir rpcport(inutilisé)
    local val="$1" dir="$2"
    # Toutes les tx sont soumises au RPC de node1 (le producteur) : la clé de signature
    # (--wallet) fixe l'émetteur, pas l'endpoint. Évite de dépendre du gossip de tx d'un
    # non-producteur vers le producteur.
    "$WALLET" transfer --to "$val" --amount "$FUND_VINX" \
        --wallet "$BASE/node1/validator.json" --node http://127.0.0.1:8545 >/dev/null
    for _ in $(seq 1 60); do
        b=$(balance_atoms 8545 "$val"); [[ -n "$b" && "$b" != "0" ]] && break; sleep 0.5
    done
    "$WALLET" stake --amount "$BOND_VINX" \
        --wallet "$dir/validator.json" --node http://127.0.0.1:8545 >/dev/null
    for _ in $(seq 1 60); do
        s=$(staked_atoms 8545 "$val"); [[ -n "$s" && "$s" != "0" ]] && \
            python3 -c "import sys;sys.exit(0 if int('$s')>=int('$BOND_ATOMS') else 1)" && break
        sleep 0.5
    done
    "$WALLET" add-validator --validator "$val" \
        --wallet "$BASE/node1/admin.json" --node http://127.0.0.1:8545 >/dev/null
    # Attendre que l'ajout soit appliqué (évite la collision de nonce admin sur l'ajout suivant).
    for _ in $(seq 1 60); do rpc 8545 validators | grep -q "$val" && break; sleep 0.5; done
}
echo "  → node2 : financement + bond + add-validator"
fund_and_bond "$N2_VAL" "$BASE/node2" 8546
echo "  → node3 : financement + bond + add-validator"
fund_and_bond "$N3_VAL" "$BASE/node3" 8547

for _ in $(seq 1 60); do [[ "$(vcount 8545)" == "3" ]] && break; sleep 0.5; done
echo "  set actif : $(rpc 8545 validators | python3 -c "import sys,json;d=json.load(sys.stdin);print(d['count'],'validateurs · quorum',d['quorum'])")"
[[ "$(vcount 8545)" == "3" ]] || { echo "ÉCHEC: le set n'a pas atteint 3"; exit 1; }

# ── Observation : la finalité avance sur les 3 nœuds ──
drive() { "$WALLET" transfer --to "$N2_VAL" --amount 1 \
    --wallet "$BASE/node1/validator.json" --node http://127.0.0.1:8545 >/dev/null 2>&1 || true; }

echo "=== Observation : finalité (co-signatures réelles) ==="
for i in $(seq 1 6); do drive; sleep "$BLOCK_TIME"
    printf "  round %d — node1 h=%s/final=%s · node2 h=%s/final=%s · node3 h=%s/final=%s\n" \
        "$i" "$(height 8545)" "$(finalized 8545)" "$(height 8546)" "$(finalized 8546)" "$(height 8547)" "$(finalized 8547)"
done

# ── Tolérance de panne : tuer node3 (2/3 finalise encore) ──
echo "=== Tolérance : node3 tué → 2/3 doit continuer à finaliser ==="
kill "${PID3:-0}" 2>/dev/null || true
F0=$(finalized 8545)
for i in $(seq 1 5); do drive; sleep "$BLOCK_TIME"
    printf "  round %d — node1 h=%s/final=%s · node2 h=%s/final=%s\n" \
        "$i" "$(height 8545)" "$(finalized 8545)" "$(height 8546)" "$(finalized 8546)"
done
F1=$(finalized 8545)
python3 -c "import sys;sys.exit(0 if int('$F1')>int('$F0') else 1)" \
    && echo "  ✓ finalité a avancé malgré 1 panne ($F0 → $F1)" \
    || echo "  ✗ finalité n'a pas avancé (attendu : elle avance à 2/3)"

# ── Sûreté : tuer node2 aussi (1/3) → finalité doit STALLER ──
echo "=== Sûreté : node2 tué aussi → 1/3, la finalité doit staller ==="
kill "${PID2:-0}" 2>/dev/null || true
F2=$(finalized 8545); T2=$(height 8545)
for i in $(seq 1 5); do drive; sleep "$BLOCK_TIME"
    printf "  round %d — node1 tip=%s / finalized=%s\n" "$i" "$(height 8545)" "$(finalized 8545)"
done
F3=$(finalized 8545); T3=$(height 8545)
python3 -c "import sys;sys.exit(0 if int('$F3')==int('$F2') else 1)" \
    && echo "  ✓ sûreté : finalité gelée à $F3 sous le quorum (tip $T2→$T3)" \
    || echo "  ⚠ finalité a bougé ($F2 → $F3) — à investiguer"

echo
echo "=== Banc terminé. Logs : $BASE/node{1,2,3}.log ==="
