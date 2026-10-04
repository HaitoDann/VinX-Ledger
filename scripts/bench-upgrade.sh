#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# VinX — Mise à jour sans redémarrage (ADR 0086).
#
# NODE = version actuelle, NEW_NODE = version suivante (NODE_PROTOCOL_VERSION + 1), les
# deux construites avec un préavis d'annonce court (UPGRADE_NOTICE_PATCH_SECS = 60).
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
BASE="${BENCH_DIR:-/tmp/vinx-bench-upgrade}"
NEW_NODE="${NEW_NODE:?NEW_NODE=<binaire de la nouvelle version>}"
declare -A BIN
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
    VINX_GENESIS_SPEC="$BASE/genesis.json" "${BIN[$i]:-$NODE}" --data-dir "$BASE/node$i" $(owner_args "$i") \
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

field() { rpc "$((8544 + $1))" health | jfield "['$2']"; }
echo "=== Phase 1 : 4 nœuds en version actuelle ==="
for i in 1 2 3 4; do start "$i"; done
if wait_height 5 120 1 2 3 4; then ok "les 4 nœuds avancent (protocole $(field 1 protocol))"; else ko "pas de progression"; fi
OLD=$(field 1 software_protocol)
echo "=== Phase 2 : annonce de la nouvelle version ==="
ACT=$(( $(date +%s) + 75 ))
TARGET=$(python3 -c "v='$OLD'.split('.');v[2]=str(int(v[2])+1);print('.'.join(v))")
"$WALLET" announce-upgrade --version "$TARGET" --activation-ts "$ACT" --wallet "$BASE/node1/admin.json" \
    --node http://127.0.0.1:8545 >"$BASE/announce.log" 2>&1 && grep -q accepted "$BASE/announce.log" && ok "mise à jour $TARGET annoncée pour dans 75 s" || ko "annonce refusée : $(tail -1 "$BASE/announce.log")"
while (( $(date +%s) < ACT + 20 )); do sleep 5; done
[[ "$(field 1 protocol)" == "$OLD" ]] && ok "échéance passée, personne ne l'a installée : reste en attente ($OLD)" \
    || ko "activée sans signal : $(field 1 protocol)"
echo "=== Phase 3 : 3 validateurs sur 4 installent la nouvelle version ==="
for i in 1 2 3; do stop "$i"; BIN[$i]="$NEW_NODE"; start "$i"; done
deadline=$(( $(date +%s) + 300 ))
until [[ "$(field 1 protocol)" == "$TARGET" ]] || (( $(date +%s) > deadline )); do sleep 3; done
[[ "$(field 1 protocol)" == "$TARGET" ]] && ok "activée quand plus des 2/3 l'ont signalée ($TARGET)" || ko "pas activée"
sleep 10
[[ "$(field 4 upgrade_required)" == True ]] && ok "node4 (ancienne version) s'arrête proprement : upgrade_required" \
    || ko "node4 : upgrade_required=$(field 4 upgrade_required)"
H=$(height 1)
if wait_height $((H + 4)) 180 1 2 3; then ok "la chaîne continue avec les nœuds à jour"; else ko "bloquée après l'activation"; fi
echo "=== Phase 4 : node4 se met à jour ==="
stop 4; BIN[4]="$NEW_NODE"; start 4
H=$(height 1)
if wait_height $((H + 3)) 240 1 2 3 4; then ok "node4 à jour rattrape la chaîne, sans redémarrage depuis la genèse"; else ko "node4 bloqué"; fi
agree "$(height 4)" 1 2 3 4 || true
echo
if (( FAIL )); then echo "=== ÉCHEC — logs : $BASE/node*.log ==="; exit 1; fi
echo "=== Mise à jour sans redémarrage : réussi ==="
