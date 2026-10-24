#!/usr/bin/env bash
# Builds libmarknotes_sync.so for the Android ABIs MarkNotes ships (arm64-v8a, x86_64) into
# bindings/kotlin/natives/android/<abi>/, then checks them (scripts/check-android-libs.sh).
# Needs cargo-ndk and the Android NDK pinned below (ANDROID_NDK_HOME, or ndk/<version> in ANDROID_HOME).
# MARKNOTES_ALLOW_ANY_NDK=1 accepts another NDK, for local builds only: the release must use the pinned one.
set -euo pipefail
cd "$(dirname "$0")/.."

# Pinned for reproducible builds (docs/fdroid.md); keep in step with .github/workflows/*.yml.
NDK_VERSION=30.0.16248370
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}/ndk/$NDK_VERSION}"
ndk_version="$(sed -n 's/^Pkg\.Revision *= *//p' "$ANDROID_NDK_HOME/source.properties" 2>/dev/null || true)"
if [ "$ndk_version" != "$NDK_VERSION" ]; then
  if [ "${MARKNOTES_ALLOW_ANY_NDK:-}" = 1 ]; then
    echo "warning: NDK ${ndk_version:-not found} at $ANDROID_NDK_HOME, not the pinned $NDK_VERSION" >&2
  else
    echo "error: need NDK $NDK_VERSION, found ${ndk_version:-none} at $ANDROID_NDK_HOME" >&2
    exit 1
  fi
fi

# shellcheck source=lib/reproducible.sh
source scripts/lib/reproducible.sh
# 16 KB ELF segment alignment, required by Android 15+ devices with 16 KB pages (the NDK default since r28; explicit
# so another NDK can't silently change it).
repro_rustflag "-Clink-arg=-Wl,-z,max-page-size=16384"

rm -rf bindings/kotlin/natives/android
cargo ndk --target arm64-v8a --target x86_64 --platform 29 --output-dir bindings/kotlin/natives/android \
  build --release --locked -p marknotes-sync --features uniffi
# Keep only the core library (cargo-ndk may also copy libc++_shared.so).
find bindings/kotlin/natives/android -type f ! -name libmarknotes_sync.so -delete
scripts/check-android-libs.sh bindings/kotlin/natives/android
