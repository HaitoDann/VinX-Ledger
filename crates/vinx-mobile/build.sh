#!/bin/sh
# Compile le cœur du portefeuille mobile en WebAssembly dans www/pkg.
# Prérequis : rustup target add wasm32-unknown-unknown
#             cargo install wasm-bindgen-cli --version <celle de Cargo.lock>
set -eu
cd "$(dirname "$0")/../.."
cargo build --release --locked --target wasm32-unknown-unknown -p vinx-mobile
wasm-bindgen --target web --no-typescript \
  --out-dir crates/vinx-mobile/www/pkg \
  target/wasm32-unknown-unknown/release/vinx_mobile.wasm
echo "prêt : servir crates/vinx-mobile/www (par exemple : python3 -m http.server -d crates/vinx-mobile/www)"
