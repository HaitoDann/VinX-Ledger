#!/usr/bin/env bash
# VinX Ledger — local 3-validator devnet launcher
# Usage: ./scripts/devnet.sh [--clean]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BIN="$ROOT/target/debug/vinx-node"

if [[ "${1:-}" == "--clean" ]]; then
    echo "Cleaning devnet data..."
    rm -rf "$ROOT/devnet1" "$ROOT/devnet2" "$ROOT/devnet3"
fi

# Build if binary is missing or stale
if [[ ! -f "$BIN" ]]; then
    echo "Building vinx-node..."
    cargo build --manifest-path "$ROOT/Cargo.toml"
fi

mkdir -p "$ROOT/devnet1" "$ROOT/devnet2" "$ROOT/devnet3"

PIDS=()
cleanup() {
    echo ""
    echo "Stopping devnet nodes..."
    for pid in "${PIDS[@]:-}"; do
        kill "$pid" 2>/dev/null || true
    done
}
trap cleanup EXIT INT TERM

echo ""
echo "═══════════════════════════════════════════════════"
echo "  VinX Ledger — 3-validator local devnet"
echo "═══════════════════════════════════════════════════"

# Node 1 — genesis validator
"$BIN" \
    --data-dir="$ROOT/devnet1" \
    --rpc-listen="127.0.0.1:8545" \
    --p2p-listen="/ip4/127.0.0.1/tcp/9001" \
    --block-time=3 \
    2>&1 | sed 's/^/[node1] /' &
PIDS+=($!)
echo "  Node 1  →  http://127.0.0.1:8545  (P2P :9001)"

sleep 2

# Node 2 — syncs from node 1
"$BIN" \
    --data-dir="$ROOT/devnet2" \
    --rpc-listen="127.0.0.1:8546" \
    --p2p-listen="/ip4/127.0.0.1/tcp/9002" \
    --peers="/ip4/127.0.0.1/tcp/9001" \
    --sync-peer="http://127.0.0.1:8545" \
    --block-time=3 \
    2>&1 | sed 's/^/[node2] /' &
PIDS+=($!)
echo "  Node 2  →  http://127.0.0.1:8546  (P2P :9002)"

sleep 1

# Node 3 — syncs from node 1
"$BIN" \
    --data-dir="$ROOT/devnet3" \
    --rpc-listen="127.0.0.1:8547" \
    --p2p-listen="/ip4/127.0.0.1/tcp/9003" \
    --peers="/ip4/127.0.0.1/tcp/9001" \
    --sync-peer="http://127.0.0.1:8545" \
    --block-time=3 \
    2>&1 | sed 's/^/[node3] /' &
PIDS+=($!)
echo "  Node 3  →  http://127.0.0.1:8547  (P2P :9003)"

echo "═══════════════════════════════════════════════════"
echo "  Press Ctrl-C to stop all nodes."
echo "═══════════════════════════════════════════════════"
echo ""

wait
