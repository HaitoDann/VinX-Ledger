#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# VinX — Banc n=4 multi-processus du consensus BFT (ADR 0082).
#
# Lance 4 vrais nœuds (P2P libp2p, RPC HTTP) sur une genèse partagée à 4 validateurs,
# puis vérifie :
#   1. progression : les 4 nœuds commitent et ont le même hash à chaque hauteur ;
#   2. tolérance  : 1 nœud tué → les 3 autres continuent (3/4 > 2/3) ;
#   3. sûreté     : 2 nœuds tués → la chaîne s'ARRÊTE (2/4 ≤ 2/3), aucun fork ;
#   4. reprise    : les nœuds relancés se resynchronisent et la chaîne repart.
#
# Usage :  cargo build --release -p vinx-node && scripts/bench-n4.sh
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

NODE="${NODE:-./target/release/vinx-node}"
WALLET="${WALLET:-./target/release/vinx-wallet}"
BASE="${BENCH_DIR:-/tmp/vinx-bench-n4}"
BLOCK_TIME="${BLOCK_TIME:-2}"
CHAIN_ID=42
GENESIS_TS=$(( $(date +%s) - 60 ))
FAIL=0

for b in "$NODE" "$WALLET"; do
    [[ -x "$b" ]] || { echo "ERREUR: $b introuvable (cargo build --release -p vinx-node -p vinx-wallet)"; exit 1; }
done

rm -rf "$BASE"; mkdir -p "$BASE"/node{1,2,3,4}
declare -A PID
cleanup() { for p in "${PID[@]:-}"; do kill "$p" 2>/dev/null || true; done; }
trap cleanup EXIT

rpc()    { curl -s --max-time 2 "http://127.0.0.1:$1/$2" 2>/dev/null || true; }
jfield() { python3 -c "import sys,json
try: print(json.load(sys.stdin)$1)
except Exception: print('')"; }
height() { rpc "$((8544 + $1))" health | jfield "['height']"; }
bhash()  { rpc "$((8544 + $1))" "block/$2" | jfield "['hash']"; }
ok()     { echo "  ✓ $*"; }
ko()     { echo "  ✗ $*"; FAIL=1; }

start() { # id
    local i="$1" peers=()
    for j in 1 2 3 4; do [[ $j != "$i" ]] && peers+=("/ip4/127.0.0.1/tcp/$((9000 + j))"); done
    # shellcheck disable=SC2046
    VINX_GENESIS_SPEC="$BASE/genesis.json" "$NODE" --data-dir "$BASE/node$i" $(owner_args "$i") \
        --rpc-listen "127.0.0.1:$((8544 + i))" --p2p-listen "/ip4/127.0.0.1/tcp/$((9000 + i))" \
        --peers "${peers[@]}" >>"$BASE/node$i.log" 2>&1 &
    PID[$i]=$!
}
stop() { kill "${PID[$1]}" 2>/dev/null || true; wait "${PID[$1]}" 2>/dev/null || true; unset "PID[$1]"; }

wait_height() { # target timeout_s nodes...
    local target="$1" deadline=$(( $(date +%s) + $2 )); shift 2
    while (( $(date +%s) < deadline )); do
        local all=1
        for i in "$@"; do local h; h=$(height "$i"); [[ -n "$h" ]] && (( h >= target )) || all=0; done
        (( all )) && return 0
        sleep 1
    done
    return 1
}

agree() { # upto nodes...
    local upto="$1"; shift
    for h in $(seq 1 "$upto"); do
        local ref; ref=$(bhash "$1" "$h")
        for i in "$@"; do
            [[ "$(bhash "$i" "$h")" == "$ref" && -n "$ref" ]] || { ko "désaccord à la hauteur $h"; return 1; }
        done
    done
    ok "même chaîne sur les nœuds $* jusqu'à $upto"
}

echo "=== Phase 0 : clés et genèse à 4 validateurs ==="
# node4 runs with SEPARATE keys (ADR 0084 S5): its owner key lives elsewhere (here a
# directory standing for the owner's offline wallet); the node only gets the operator
# and BLS keys.
"$NODE" --data-dir "$BASE/owner4" --chain-id $CHAIN_ID --genesis-entry >/dev/null 2>&1
OWNER4=$(python3 -c "import json;print(json.load(open('$BASE/owner4/validator.json'))['address'])")
owner_args() { [[ "$1" == 4 ]] && echo "--validator-owner $OWNER4"; }
ENTRIES=()
for i in 1 2 3 4; do
    # shellcheck disable=SC2046
    ENTRIES+=("$("$NODE" --data-dir "$BASE/node$i" --chain-id $CHAIN_ID $(owner_args "$i") \
        --genesis-entry 2>/dev/null | grep "^{")")
done
ADMIN=$(python3 -c "import json;print(json.load(open('$BASE/node1/admin.json'))['address'])")
python3 - "$BASE/genesis.json" "$ADMIN" "$GENESIS_TS" "$BLOCK_TIME" "${ENTRIES[@]}" <<'PY'
import json, sys
out, admin, ts, bt, *entries = sys.argv[1:]
e = [json.loads(x) for x in entries]
json.dump({
    "chain_id": 42, "genesis_timestamp": int(ts), "admin_address": admin,
    "initial_validator": e[0]["address"],
    "initial_validator_bls_pub_key": e[0]["bls_pub_key"],
    "initial_validator_bls_pop": e[0]["bls_pop"],
    "block_time_secs": int(bt), "validators": e[1:],
    "prefund_initial_validator_vinx": 1000,
}, open(out, "w"), indent=2)
PY
echo "  spec : $BASE/genesis.json"

echo "=== Phase 1 : 4 nœuds — progression ==="
for i in 1 2 3 4; do start "$i"; done
if wait_height 5 120 1 2 3 4; then ok "les 4 nœuds atteignent la hauteur 5"; else ko "pas de progression à 4"; fi
agree 5 1 2 3 4 || true

PROPOSED=0
for h in $(seq 1 "$(height 1)"); do
    [[ "$(rpc 8545 "block/$h" | jfield "['validator']")" == "$OWNER4" ]] && PROPOSED=$((PROPOSED + 1))
done
if (( PROPOSED > 0 )); then
    ok "node4 (clés séparées, sans la clé du propriétaire) a proposé $PROPOSED bloc(s) au nom de $OWNER4"
else ko "aucun bloc proposé par node4"; fi

echo "=== Phase 1b : paiement réel + reçu vérifié depuis un autre nœud ==="
DEST=$(python3 -c "import json;print(json.load(open('$BASE/node2/validator.json'))['address'])")
OUT=$("$WALLET" transfer --to "$DEST" --amount 5 --wallet "$BASE/node1/validator.json" \
    --node http://127.0.0.1:8545 2>&1 || true)
TXH=$(echo "$OUT" | grep -oE '[0-9a-f]{64}' | head -1)
if [[ -z "$TXH" ]]; then ko "transfert refusé : $OUT"; else
    GOT=""
    for _ in $(seq 1 30); do
        if "$WALLET" receipt "$TXH" --out "$BASE/receipts" --node http://127.0.0.1:8547 >/dev/null 2>&1; then GOT=1; break; fi
        sleep 1
    done
    if [[ -n "$GOT" ]] && "$WALLET" verify-receipt "$BASE/receipts/$TXH.json" >/dev/null; then
        ok "paiement inclus, reçu obtenu de node3 et vérifié hors-ligne"
    else ko "pas de reçu pour $TXH"; fi
    BAL=$(rpc 8546 "account/$DEST" | jfield "['balance_atoms']")
    [[ "$BAL" == "5000000000" ]] && ok "solde du destinataire = 5 VINX (vu par node2)" || ko "solde inattendu : $BAL"
fi

echo "=== Phase 2 : node4 tué — tolérance à 1 panne ==="
stop 4
H=$(height 1)
if wait_height $((H + 4)) 180 1 2 3; then ok "3/4 continue ($H → $(height 1))"; else ko "3/4 bloqué à $(height 1)"; fi

echo "=== Phase 3 : node3 tué aussi — la chaîne doit s'arrêter ==="
stop 3
sleep 3
H1=$(height 1); sleep $((BLOCK_TIME * 8)); H2=$(height 1)
if [[ "$H1" == "$H2" ]]; then ok "arrêt sous le quorum (hauteur figée à $H2)"; else ko "a commité sans quorum ($H1 → $H2)"; fi
[[ "$(height 1)" == "$(height 2)" ]] && ok "node1 et node2 d'accord ($H2)" || true

echo "=== Phase 4 : relance de node3 et node4 — reprise ==="
start 3; start 4
if wait_height $((H2 + 3)) 240 1 2 3 4; then ok "la chaîne repart et tous rattrapent"; else ko "pas de reprise"; fi
agree "$(height 1)" 1 2 3 4 || true

echo
if (( FAIL )); then echo "=== ÉCHEC — logs : $BASE/node*.log ==="; exit 1; fi
echo "=== Banc n=4 réussi — logs : $BASE/node*.log ==="
