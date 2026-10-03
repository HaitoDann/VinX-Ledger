#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# VinX — Passage des box : nœuds derrière un NAT (simulé par iptables).
#
# node1 et node4 sont joignables ; node2 et node3 refusent toute connexion entrante
# (comme derrière une box sans port ouvert). Ils doivent le détecter (AutoNAT),
# réserver un relais, et la chaîne doit survivre à l'arrêt de node1.
# Nécessite root (iptables).
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
BASE="${BENCH_DIR:-/tmp/vinx-bench-nat}"
export VINX_NO_MDNS=1   # pas de découverte locale : seul Kademlia peut relier les nœuds
BLOCK_TIME="${BLOCK_TIME:-2}"
CHAIN_ID=42
GENESIS_TS=$(( $(date +%s) - 60 ))
FAIL=0

for b in "$NODE" "$WALLET"; do
    [[ -x "$b" ]] || { echo "ERREUR: $b introuvable (cargo build --release -p vinx-node -p vinx-wallet)"; exit 1; }
done

rm -rf "$BASE"; mkdir -p "$BASE"/node{1,2,3,4}
declare -A PID
nat_on() {  # refuse les connexions entrantes vers les ports P2P de node2 et node3
    for port in 9002 9003; do
        iptables -I INPUT -p tcp --dport "$port" --syn -j DROP
        iptables -I INPUT -p udp --dport "$port" -m conntrack --ctstate NEW -j DROP
    done
}
nat_off() {
    for port in 9002 9003; do
        iptables -D INPUT -p tcp --dport "$port" --syn -j DROP 2>/dev/null || true
        iptables -D INPUT -p udp --dport "$port" -m conntrack --ctstate NEW -j DROP 2>/dev/null || true
    done
}
cleanup() { for p in "${PID[@]:-}"; do kill "$p" 2>/dev/null || true; done; nat_off; }
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
    [[ $i != 1 ]] && peers+=("/ip4/127.0.0.1/tcp/9001")   # seul point d'entrée : node1
    # shellcheck disable=SC2046
    VINX_GENESIS_SPEC="$BASE/genesis.json" "$NODE" --data-dir "$BASE/node$i" $(owner_args "$i") \
        --rpc-listen "127.0.0.1:$((8544 + i))" --p2p-listen "/ip4/127.0.0.1/tcp/$((9000 + i))" \
        ${peers[@]:+--peers "${peers[@]}"} >>"$BASE/node$i.log" 2>&1 &
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

field() { rpc "$((8544 + $1))" health | jfield "['$2']"; }

echo "=== Phase 1 : node2 et node3 derrière une « box » ==="
nat_on
for i in 1 2 3 4; do start "$i"; done
if wait_height 5 120 1 2 3 4; then ok "les 4 nœuds avancent"; else ko "pas de progression"; fi
deadline=$(( $(date +%s) + 240 ))
while (( $(date +%s) < deadline )); do
    [[ "$(field 2 reachable)" == private && "$(field 3 reachable)" == private \
       && "$(field 2 relays)" -ge 1 && "$(field 3 relays)" -ge 1 ]] && break
    sleep 5
done
for i in 2 3; do
    r=$(field $i reachable); n=$(field $i relays)
    [[ "$r" == private ]] && ok "node$i se sait derrière une box (AutoNAT)" || ko "node$i : joignabilité $r"
    [[ "${n:-0}" -ge 1 ]] && ok "node$i a $n relais" || ko "node$i sans relais"
done
for i in 1 4; do
    r=$(field $i reachable)
    [[ "$r" != private ]] && ok "node$i joignable ($r)" || ko "node$i se croit derrière une box"
done
grep -h "Direct connection through the NAT" "$BASE"/node[23].log | head -1 | grep -q . \
    && ok "connexion directe percée entre nœuds derrière des box (DCUtR)" \
    || echo "  · pas de perçage direct observé (relais utilisé)"

echo "=== Phase 2 : node1 (le point d'entrée) est coupé ==="
stop 1
H=$(height 2)
if wait_height $((H + 4)) 180 2 3 4; then ok "la chaîne continue sans node1 ($H → $(height 2))"; else ko "bloquée sans node1 à $(height 2)"; fi
agree "$(height 2)" 2 3 4 || true

echo "=== Phase 3 : node3 redémarre derrière sa box, sans point d'entrée ==="
stop 3; start 3
H=$(height 2)
if wait_height $((H + 3)) 240 2 3 4; then ok "node3 a retrouvé le réseau ($(field 3 peers) pairs)"; else ko "node3 isolé"; fi

echo
if (( FAIL )); then echo "=== ÉCHEC — logs : $BASE/node*.log ==="; exit 1; fi
echo "=== Passage des box : réussi — logs : $BASE/node*.log ==="
