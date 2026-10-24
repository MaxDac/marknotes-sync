#!/usr/bin/env bash
# Builds the Android libraries the way an F-Droid build server does: from vendored sources (cargo vendor), with the
# network off. The result must be byte-for-byte the libraries in the released AAR, which CI checks ("Reproducible
# Android libraries"). It then generates the Kotlin bindings. See docs/fdroid.md for the F-Droid recipe that calls it.
#   scripts/fdroid-build.sh            vendor (needs the network once), then build offline
#   scripts/fdroid-build.sh --offline  build from an existing vendor/ directory (what the build server runs)
# Same requirements as scripts/android-libs.sh (cargo-ndk, the pinned NDK), plus the toolchain in rust-toolchain.toml.
set -euo pipefail
cd "$(dirname "$0")/.."

if [ "${1:-}" != --offline ]; then
  rm -rf vendor
  cargo vendor --locked --versioned-dirs --quiet vendor >/dev/null
fi
[ -d vendor ] || { echo "error: no vendor/ directory; run without --offline first" >&2; exit 1; }

# Source replacement, as `cargo vendor` prints it. A temporary config file: the repository has none of its own.
[ ! -e .cargo/config.toml ] || { echo "error: .cargo/config.toml exists; remove it first" >&2; exit 1; }
mkdir -p .cargo
trap 'rm -f .cargo/config.toml; rmdir .cargo 2>/dev/null || true' EXIT
cat >.cargo/config.toml <<'EOF'
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
EOF

export CARGO_NET_OFFLINE=true
scripts/android-libs.sh
# The Kotlin sources the app compiles, generated from the host library's UniFFI metadata.
scripts/kotlin-bindings.sh