# Sourced by the build scripts. Maps the build-specific paths rustc embeds (panic locations) to fixed ones, so the
# libraries are byte-for-byte the same whatever the checkout directory, Cargo home, or whether the dependencies come
# from the registry or from `cargo vendor --versioned-dirs` (the F-Droid build). See docs/fdroid.md.
# rustc applies the last matching --remap-path-prefix, so the general prefixes come first.
# The flags go in CARGO_ENCODED_RUSTFLAGS (separated by 0x1f) so paths with spaces survive; any RUSTFLAGS already set
# are folded in, since Cargo ignores RUSTFLAGS once the encoded variable is set. Add more with `repro_rustflag`.
# shellcheck shell=bash
repro_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
repro_cargo_home="${CARGO_HOME:-$HOME/.cargo}"
# Git Bash on Windows: rustc sees Windows paths, not /c/...
repro_native() { if command -v cygpath >/dev/null 2>&1; then cygpath -w "$1"; else printf '%s' "$1"; fi; }

repro_rustflag() {
  CARGO_ENCODED_RUSTFLAGS="${CARGO_ENCODED_RUSTFLAGS:+$CARGO_ENCODED_RUSTFLAGS$'\x1f'}$1"
  export CARGO_ENCODED_RUSTFLAGS
}

if [ -z "${CARGO_ENCODED_RUSTFLAGS:-}" ] && [ -n "${RUSTFLAGS:-}" ]; then
  for repro_flag in $RUSTFLAGS; do repro_rustflag "$repro_flag"; done
fi
# The registry sources must exist before their paths can be listed.
(cd "$repro_root" && cargo fetch --locked --quiet)
repro_rustflag "--remap-path-prefix=$(repro_native "$repro_root")=/marknotes-sync"
repro_rustflag "--remap-path-prefix=$(repro_native "$repro_cargo_home")=/cargo"
repro_rustflag "--remap-path-prefix=$(repro_native "$repro_root/vendor")=/deps"
for repro_src in "$repro_cargo_home"/registry/src/*/; do
  [ -d "$repro_src" ] && repro_rustflag "--remap-path-prefix=$(repro_native "${repro_src%/}")=/deps"
done
unset repro_root repro_cargo_home repro_flag repro_src