#!/usr/bin/env bash
# Sets up a local 3-validator VinX testnet using the compiled binaries.
# Run from the repo root after: cargo build --release
set -euo pipefail

BINARY="./target/release/vinx-node"
WALLET="./target/release/vinx-wallet"
BASE_DIR="${TESTNET_DIR:-/tmp/vinx-testnet}"
BLOCK_TIME="${BLOCK_TIME:-3}"

echo "=== VinX Testnet Setup ==="
echo "Base directory : $BASE_DIR"
echo "Block time     : ${BLOCK_TIME}s"
echo ""

if [[ ! -x "$BINARY" ]]; then
    echo "ERROR: $BINARY not found. Run 'cargo build --release' first."
    exit 1
fi

# ── Clean previous run ───────────────────────────────────────────────────────
rm -rf "$BASE_DIR"
mkdir -p "$BASE_DIR"/{node1,node2,node3}

# ── Helper: start a node in the background ───────────────────────────────────
start_node() {
    local id="$1"
    local rpc_port="$2"
    local p2p_port="$3"
    local extra_args="${4:-}"
    local log="$BASE_DIR/node${id}.log"

    $BINARY \
        --data-dir "$BASE_DIR/node${id}" \
        --rpc-listen "127.0.0.1:${rpc_port}" \
        --p2p-listen "/ip4/127.0.0.1/tcp/${p2p_port}" \
        --block-time "$BLOCK_TIME" \
        $extra_args \
        >"$log" 2>&1 &
    echo $! > "$BASE_DIR/node${id}.pid"
    echo "Node $id started (PID $(cat $BASE_DIR/node${id}.pid))"
    echo "  RPC : http://127.0.0.1:${rpc_port}"
    echo "  P2P : /ip4/127.0.0.1/tcp/${p2p_port}"
    echo "  Log : $log"
}

stop_all() {
    echo ""
    echo "=== Stopping testnet ==="
    for f in "$BASE_DIR"/*.pid; do
        [[ -f "$f" ]] && kill "$(cat "$f")" 2>/dev/null && echo "Stopped PID $(cat $f)"
    done
}
trap stop_all EXIT

# ── Launch nodes ─────────────────────────────────────────────────────────────
start_node 1 8545 9001
sleep 2

start_node 2 8546 9002 "--peers /ip4/127.0.0.1/tcp/9001 --sync-peer http://127.0.0.1:8545"
start_node 3 8547 9003 "--peers /ip4/127.0.0.1/tcp/9001 --sync-peer http://127.0.0.1:8545"
sleep 2

# ── Print node info ───────────────────────────────────────────────────────────
echo ""
echo "=== Validator keys ==="
for i in 1 2 3; do
    addr_file="$BASE_DIR/node${i}/validator.json"
    if [[ -f "$addr_file" ]]; then
        addr=$(python3 -c "import json,sys; print(json.load(sys.stdin)['address'])" < "$addr_file" 2>/dev/null || \
               grep -o '"address":"[^"]*"' "$addr_file" | cut -d'"' -f4)
        echo "Node $i validator : $addr"
    fi
done

echo ""
echo "=== Testnet running — press Ctrl-C to stop ==="
echo ""
echo "Useful commands:"
echo "  $WALLET status --node http://127.0.0.1:8545"
echo "  $WALLET balance <address> --node http://127.0.0.1:8545"
echo "  $WALLET validators --node http://127.0.0.1:8545"
echo "  curl http://127.0.0.1:8545/metrics"
echo ""

# Keep running until Ctrl-C
while true; do sleep 5; done
