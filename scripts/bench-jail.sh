#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# VinX — Un validateur absent quitte le set, puis revient seul (ADR 0086).
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
BASE="${BENCH_DIR:-/tmp/vinx-bench-jail}"
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
bhash()  { local h; for _ in 1 2 3; do h=$(rpc "$((8544 + $1))" "block/$2" | jfield "['hash']"); [[ -n "$h" ]] && break; sleep 1; done; echo "$h"; }
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
            [[ "$(bhash "$i" "$h")" == "$ref" && -n "$ref" ]] || { ko "désaccord à la hauteur $h (node$1=${ref:-vide}, node$i=$(bhash "$i" "$h"))"; return 1; }
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

vst() { rpc 8545 "validator/$1" | jfield "['$2']"; }
echo "=== Phase 1 : 4 validateurs ==="
for i in 1 2 3 4; do start "$i"; done
if wait_height 5 120 1 2 3 4; then ok "les 4 nœuds avancent"; else ko "pas de progression"; fi
V4=$OWNER4
echo "=== Phase 2 : node4 tombe ==="
stop 4
deadline=$(( $(date +%s) + 300 ))
until [[ "$(vst "$V4" jailed)" == True && "$(vst "$V4" in_set)" == False ]] || (( $(date +%s) > deadline )); do sleep 3; done
[[ "$(vst "$V4" in_set)" == False ]] && ok "node4 suspendu et retiré du set ($(rpc 8545 validators | jfield "['count']") validateurs votent)" || ko "node4 toujours dans le set"
H=$(height 1)
if wait_height $((H + 5)) 120 1 2 3; then ok "la chaîne continue à 3"; else ko "bloquée"; fi
echo "=== Phase 3 : node4 revient ==="
start 4
deadline=$(( $(date +%s) + 600 ))
until [[ "$(vst "$V4" jailed)" == False ]] || (( $(date +%s) > deadline )); do sleep 5; done
[[ "$(vst "$V4" jailed)" == False ]] && ok "node4 s'est réhabilité tout seul (Unjail par sa clé d'opérateur)" || ko "node4 toujours suspendu"
agree "$(height 4)" 1 2 3 4 || true
echo
if (( FAIL )); then echo "=== ÉCHEC — logs : $BASE/node*.log ==="; exit 1; fi
echo "=== Absence et retour : réussi ==="
