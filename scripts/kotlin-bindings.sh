#!/usr/bin/env bash
# Builds the host library with UniFFI and generates the Kotlin bindings into bindings/kotlin/src/main/kotlin.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --release --locked -p marknotes-sync --features uniffi
case "$(uname -s)" in
  Linux*) lib=target/release/libmarknotes_sync.so ;;
  Darwin*) lib=target/release/libmarknotes_sync.dylib ;;
  *) lib=target/release/marknotes_sync.dll ;;
esac
rm -rf bindings/kotlin/src/main/kotlin
cargo run --release --locked -p uniffi-bindgen -- generate --library "$lib" --language kotlin --no-format \
  --out-dir bindings/kotlin/src/main/kotlin
