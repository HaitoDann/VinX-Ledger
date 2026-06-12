# ─── Build stage ─────────────────────────────────────────────────────────────
FROM rust:1.78-slim AS builder

WORKDIR /build

# Cache dependencies before copying source
COPY Cargo.toml Cargo.lock ./
COPY crates/vinx-crypto/Cargo.toml crates/vinx-crypto/
COPY crates/vinx-core/Cargo.toml crates/vinx-core/
COPY crates/vinx-state/Cargo.toml crates/vinx-state/
COPY crates/vinx-node/Cargo.toml crates/vinx-node/
COPY crates/vinx-wallet/Cargo.toml crates/vinx-wallet/

# Create stub libs so cargo can resolve the dependency graph
RUN for d in vinx-crypto vinx-core vinx-state vinx-node; do \
      mkdir -p crates/$d/src && echo "pub fn _stub() {}" > crates/$d/src/lib.rs; \
    done && \
    mkdir -p crates/vinx-node/src && echo "fn main() {}" > crates/vinx-node/src/main.rs && \
    mkdir -p crates/vinx-wallet/src && echo "fn main() {}" > crates/vinx-wallet/src/main.rs

RUN cargo build --release -p vinx-node -p vinx-wallet 2>/dev/null || true

# Now copy the real source and rebuild
COPY crates/ crates/
RUN touch crates/*/src/*.rs crates/*/src/**/*.rs 2>/dev/null || true
RUN cargo build --release -p vinx-node -p vinx-wallet

# ─── Runtime stage ───────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/vinx-node /usr/local/bin/
COPY --from=builder /build/target/release/vinx-wallet /usr/local/bin/

WORKDIR /data

EXPOSE 8545 9000

ENTRYPOINT ["vinx-node"]
CMD ["--data-dir", "/data", "--rpc-listen", "0.0.0.0:8545"]
