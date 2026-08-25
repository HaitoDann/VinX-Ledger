# ──────────────────────────────────────────────────────────────────────────────
# Stage 1 — Build
# ──────────────────────────────────────────────────────────────────────────────
FROM rust:1.81-bookworm AS builder

WORKDIR /src

# Cache dependencies separately from source so incremental rebuilds are fast.
COPY Cargo.toml Cargo.lock ./
COPY crates/vinx-core/Cargo.toml      crates/vinx-core/Cargo.toml
COPY crates/vinx-crypto/Cargo.toml    crates/vinx-crypto/Cargo.toml
COPY crates/vinx-state/Cargo.toml     crates/vinx-state/Cargo.toml
COPY crates/vinx-node/Cargo.toml      crates/vinx-node/Cargo.toml
COPY crates/vinx-wallet/Cargo.toml    crates/vinx-wallet/Cargo.toml
COPY crates/vinx-desktop-core/Cargo.toml crates/vinx-desktop-core/Cargo.toml

# Dummy source so cargo can resolve & fetch deps without the real source.
RUN for crate in vinx-core vinx-crypto vinx-state vinx-node vinx-wallet vinx-desktop-core; do \
      mkdir -p crates/$crate/src && echo "fn main(){}" > crates/$crate/src/main.rs && \
      echo "" > crates/$crate/src/lib.rs; \
    done && \
    cargo fetch

# Now copy the real source.
COPY crates/ crates/

# Build the node binary in release mode.
RUN cargo build --release -p vinx-node

# ──────────────────────────────────────────────────────────────────────────────
# Stage 2 — Runtime
# ──────────────────────────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /src/target/release/vinx-node /usr/local/bin/vinx-node

# ── Default environment ───────────────────────────────────────────────────────
# Override these at runtime with -e or in docker-compose.yml.
ENV VINX_DATA_DIR=/data \
    VINX_RPC_LISTEN=0.0.0.0:8545 \
    VINX_P2P_LISTEN=/ip4/0.0.0.0/tcp/9000 \
    RUST_LOG=info

# Persistent storage and key files live here.
VOLUME ["/data"]

# RPC HTTP | P2P libp2p/TCP
EXPOSE 8545 9000

COPY docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod +x /usr/local/bin/docker-entrypoint.sh

ENTRYPOINT ["docker-entrypoint.sh"]
