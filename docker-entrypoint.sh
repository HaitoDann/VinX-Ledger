#!/usr/bin/env bash
# VinX node entrypoint — assembles CLI flags from environment variables and
# starts the node.  Every setting can be overridden by passing extra arguments
# after the image name:  docker run vinx-node --block-time 3
set -euo pipefail

BINARY="vinx-node"

# ── Required directories ──────────────────────────────────────────────────────
mkdir -p "${VINX_DATA_DIR:-/data}"

# ── Assemble argument list from env ──────────────────────────────────────────
ARGS=(
  "--data-dir"   "${VINX_DATA_DIR:-/data}"
  "--rpc-listen" "${VINX_RPC_LISTEN:-0.0.0.0:8545}"
)

if [[ -n "${VINX_P2P_LISTEN:-}" ]]; then
  ARGS+=("--p2p-listen" "$VINX_P2P_LISTEN")
fi

if [[ -n "${VINX_BLOCK_TIME:-}" ]]; then
  ARGS+=("--block-time" "$VINX_BLOCK_TIME")
fi

if [[ -n "${VINX_CHAIN_ID:-}" ]]; then
  ARGS+=("--chain-id" "$VINX_CHAIN_ID")
fi

if [[ -n "${VINX_SYNC_PEER:-}" ]]; then
  ARGS+=("--sync-peer" "$VINX_SYNC_PEER")
fi

# VINX_PEERS: space-separated multiaddrs, e.g.
#   "/ip4/1.2.3.4/tcp/9000/p2p/12D3... /ip4/5.6.7.8/tcp/9000/p2p/12D3..."
if [[ -n "${VINX_PEERS:-}" ]]; then
  for peer in $VINX_PEERS; do
    ARGS+=("--peers" "$peer")
  done
fi

# Config file — used when a config.toml is mounted at /data/config.toml.
CONFIG_FILE="${VINX_DATA_DIR:-/data}/config.toml"
if [[ -f "$CONFIG_FILE" ]]; then
  ARGS+=("--config" "$CONFIG_FILE")
fi

# Pass any extra arguments from CMD / docker run ...
ARGS+=("$@")

echo "=== VinX Node ==="
echo "Binary  : $(which $BINARY)"
echo "Data    : ${VINX_DATA_DIR:-/data}"
echo "RPC     : ${VINX_RPC_LISTEN:-0.0.0.0:8545}"
echo "P2P     : ${VINX_P2P_LISTEN:-disabled}"
echo "Genesis : ${VINX_GENESIS_SPEC:-local (no shared spec)}"
echo "Log     : ${RUST_LOG:-info}"
echo ""

exec "$BINARY" "${ARGS[@]}"
