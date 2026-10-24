#!/usr/bin/env bash
# Checks the packaged artifacts in <dist> for version <version> (default: Cargo.toml's), as CI and release.yml
# build them. With --release, also requires every host library in the natives jar (PR builds have Linux only).
#   marknotes-sync-<v>.tgz           npm package: the files package.json lists, at the right version, nothing else
#   marknotes-sync-kotlin-<v>.jar    generated bindings + the R8 keep rules apps rely on
#   marknotes-sync-android-<v>.aar   jni/{arm64-v8a,x86_64}/libmarknotes_sync.so only, passing check-android-libs.sh
#   marknotes-sync-natives-<v>.jar   host libraries under JNA's resource prefixes
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
release=0
[ "${1:-}" = --release ] && { release=1; shift; }
dist="$(cd "${1:?usage: check-artifacts.sh [--release] <dist> [version]}" && pwd)"
version="${2:-$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' "$root/Cargo.toml")}"
fail() { echo "error: $*" >&2; exit 1; }
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

for f in "marknotes-sync-$version.tgz" "marknotes-sync-kotlin-$version.jar" "marknotes-sync-android-$version.aar" \
  "marknotes-sync-natives-$version.jar"; do
  [ -f "$dist/$f" ] || fail "missing $f"
done

# npm package
tar -tzf "$dist/marknotes-sync-$version.tgz" | sort >"$tmp/npm"
expected="$(cd "$root/bindings/npm" && node -e '
  const p = require("./package.json");
  console.log(["package.json", ...p.files].map((f) => "package/" + f).join("\n"));
' | sort)"
# npm always adds README and LICENSE files when present.
actual="$(grep -Ev '^package/(README|LICENSE|COPYING)' "$tmp/npm" || true)"
[ "$actual" = "$expected" ] || fail "npm package files differ from package.json \"files\":
$(diff <(echo "$expected") <(echo "$actual") || true)"
tar -xzf "$dist/marknotes-sync-$version.tgz" -C "$tmp" package/package.json
[ "$(node -p 'require(process.argv[1]).version' "$tmp/package/package.json")" = "$version" ] \
  || fail "npm package is not version $version"

# Kotlin bindings
unzip -Z1 "$dist/marknotes-sync-kotlin-$version.jar" >"$tmp/kotlin"
grep -qx 'io/github/maxdac/marknotes/sync/SyncScheduler.class' "$tmp/kotlin" || fail "kotlin jar has no bindings"
grep -qx 'META-INF/proguard/marknotes-sync.pro' "$tmp/kotlin" || fail "kotlin jar has no R8 keep rules"
! grep -Eq '\.(so|dll|dylib)$' "$tmp/kotlin" || fail "kotlin jar contains native libraries"

# Android libraries
unzip -Z1 "$dist/marknotes-sync-android-$version.aar" | sort >"$tmp/aar"
printf '%s\n' AndroidManifest.xml classes.jar jni/ jni/arm64-v8a/ jni/arm64-v8a/libmarknotes_sync.so jni/x86_64/ \
  jni/x86_64/libmarknotes_sync.so | sort >"$tmp/aar-expected"
diff <(grep -v '/$' "$tmp/aar-expected") <(grep -v '/$' "$tmp/aar") >/dev/null \
  || fail "unexpected AAR contents: $(diff <(grep -v '/$' "$tmp/aar-expected") <(grep -v '/$' "$tmp/aar") || true)"
unzip -q "$dist/marknotes-sync-android-$version.aar" 'jni/*' -d "$tmp/aar-x"
"$root/scripts/check-android-libs.sh" "$tmp/aar-x/jni"

# Host libraries
unzip -Z1 "$dist/marknotes-sync-natives-$version.jar" >"$tmp/natives"
prefixes=(linux-x86-64/libmarknotes_sync.so)
[ "$release" = 1 ] && prefixes+=(win32-x86-64/marknotes_sync.dll darwin-aarch64/libmarknotes_sync.dylib
  darwin-x86-64/libmarknotes_sync.dylib)
for p in "${prefixes[@]}"; do
  grep -qx "$p" "$tmp/natives" || fail "natives jar has no $p"
done
echo "ok: artifacts for $version in $dist"
