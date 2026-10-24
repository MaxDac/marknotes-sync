#!/usr/bin/env bash
# Checks the Android libraries in <dir>/<abi>/libmarknotes_sync.so (default bindings/kotlin/natives/android) the way
# a device and F-Droid see them. Fails unless, for each of arm64-v8a and x86_64:
#   - the ELF machine matches the ABI and the library is a 64-bit shared object;
#   - every LOAD segment is aligned to at least 16 KB (Android 15+ devices with 16 KB pages);
#   - it needs only Android's system libraries (no libc++_shared.so or other bundled libraries);
#   - it exports the UniFFI entry points the Kotlin bindings call, and nothing else outside UniFFI's namespaces;
#   - no build-machine path (home directories, the checkout, Cargo home) is embedded.
# Uses READELF (default llvm-readelf, then readelf).
set -euo pipefail
cd "$(dirname "$0")/.."
dir="${1:-bindings/kotlin/natives/android}"
readelf="${READELF:-$(command -v llvm-readelf || command -v readelf)}"
fail() { echo "error: $*" >&2; exit 1; }

declare -A machine=([arm64-v8a]=AArch64 [x86_64]="X86-64")
for abi in arm64-v8a x86_64; do
  lib="$dir/$abi/libmarknotes_sync.so"
  [ -f "$lib" ] || fail "missing $lib"
  header="$("$readelf" -hW "$lib")"
  grep -Eq 'Class:\s+ELF64' <<<"$header" || fail "$lib is not ELF64"
  grep -Eq 'Type:\s+DYN' <<<"$header" || fail "$lib is not a shared object"
  grep -Eiq "Machine:\s+.*${machine[$abi]}" <<<"$header" || fail "$lib is not ${machine[$abi]}"

  while read -r align; do
    (( align >= 16384 )) || fail "$lib has a LOAD segment aligned to $align bytes, need 16384"
  done < <("$readelf" -lW "$lib" | awk '$1 == "LOAD" { print $NF }' | while read -r hex; do echo $((hex)); done)

  needed="$("$readelf" -dW "$lib" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p' | sort)"
  for n in $needed; do
    case "$n" in
      libc.so | libm.so | libdl.so | liblog.so) ;;
      *) fail "$lib needs $n; only Android system libraries are allowed" ;;
    esac
  done

  exports="$("$readelf" --dyn-syms -W "$lib" | awk '$5 == "GLOBAL" && $7 != "UND" { print $8 }' | sed 's/@.*//')"
  for symbol in uniffi_marknotes_sync_fn_func_plan uniffi_marknotes_sync_fn_func_reconcile \
    uniffi_marknotes_sync_fn_constructor_syncscheduler_new ffi_marknotes_sync_uniffi_contract_version; do
    grep -qx "$symbol" <<<"$exports" || fail "$lib does not export $symbol"
  done
  stray="$(grep -Ev '^(uniffi_|ffi_marknotes_sync_)' <<<"$exports" || true)"
  [ -z "$stray" ] || fail "$lib exports symbols outside UniFFI: $(head -5 <<<"$stray" | tr '\n' ' ')"

  paths="$(grep -aEo '/(home|Users|root|tmp|runner)/[^[:cntrl:]]{0,80}' "$lib" | head -3 || true)"
  [ -z "$paths" ] || fail "$lib embeds build paths: $paths"
  echo "ok $lib ($(wc -c <"$lib") bytes; needs: $(echo "$needed" | tr '\n' ' '))"
done
