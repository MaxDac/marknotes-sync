#!/usr/bin/env bash
# Builds libmarknotes_sync.so for the Android ABIs MarkNotes ships (arm64-v8a, x86_64) into
# bindings/kotlin/natives/android/<abi>/. Needs cargo-ndk and an Android NDK (ANDROID_NDK_HOME or the SDK's ndk/).
set -euo pipefail
cd "$(dirname "$0")/.."

cargo ndk --target arm64-v8a --target x86_64 --platform 29 --output-dir bindings/kotlin/natives/android \
  build --release --locked -p marknotes-sync --features uniffi
# Keep only the core library (cargo-ndk may also copy libc++_shared.so).
find bindings/kotlin/natives/android -type f ! -name libmarknotes_sync.so -delete
ls -lR bindings/kotlin/natives/android
