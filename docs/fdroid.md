# Building for F-Droid

F-Droid builds every app from source and does not accept prebuilt binaries, so the MarkNotes Android app can't
take `marknotes-sync-android-<v>.aar` from the GitHub release as it is: the build server rebuilds the native
library, `libmarknotes_sync.so`, from this repository. Kotlin and Java code (the jar of generated bindings) is
compiled by the app's Gradle build as usual. This page is what the app's F-Droid recipe relies on, and what CI
guarantees about it.

## What is pinned

| What | Pin | Where |
| --- | --- | --- |
| Rust toolchain | `rust-toolchain.toml` (an exact version, with rustfmt and clippy) | rustup reads it in every build |
| Crate versions and checksums | `Cargo.lock`; every build passes `--locked` | |
| Android NDK | `30.0.16248370` (r30) | `scripts/android-libs.sh` refuses any other; `NDK_VERSION` in `ci.yml` |
| cargo-ndk | `4.1.2` | `CARGO_NDK_VERSION` in `ci.yml` |
| Android API level of the libraries | 29 | `--platform 29` in `scripts/android-libs.sh` |
| ABIs | `arm64-v8a`, `x86_64` | `scripts/android-libs.sh` |

Change a pin in all the places listed, in one PR; the "Reproducible Android libraries" job then proves the new
combination still reproduces.

## Building the libraries like F-Droid

```sh
git submodule update --init      # not needed for the libraries, only for the tests
scripts/fdroid-build.sh          # vendors the crates (network), then builds offline
# or, on a build server where `cargo vendor --versioned-dirs` has already run into vendor/:
scripts/fdroid-build.sh --offline
```

The script writes a temporary `.cargo/config.toml` that replaces crates.io with `vendor/`, sets
`CARGO_NET_OFFLINE=true` and runs `scripts/android-libs.sh`, which:

1. checks that the NDK is exactly the pinned one;
2. remaps every build-specific path rustc embeds (the checkout, Cargo home, the registry or `vendor/`) to fixed
   names (`scripts/lib/reproducible.sh`), so the bytes don't depend on where or by whom the build runs;
3. links with 16 KB page alignment (`-z max-page-size=16384`), which Android 15+ devices with 16 KB pages need;
4. writes `bindings/kotlin/natives/android/<abi>/libmarknotes_sync.so` and checks them with
   `scripts/check-android-libs.sh`: 64-bit ELF shared object of the right machine, every `LOAD` segment aligned to
   16 KB or more, only Android system libraries needed (`libc`, `libm`, `libdl`, `liblog`), the UniFFI entry points
   exported and nothing else, and no build-machine path inside.

## Reproducibility

The libraries built from vendored sources, offline, in another directory and with another Cargo home are
byte-for-byte the ones `scripts/android-libs.sh` builds from crates.io. CI checks this on every PR
("Reproducible Android libraries": builds both ways and compares SHA-256), and the release publishes those same
libraries inside the AAR, with `SHA256SUMS`. F-Droid can therefore verify a rebuilt library against the release
(`unzip marknotes-sync-android-<v>.aar 'jni/*'` and compare).

The jar and AAR archives themselves are also built reproducibly (fixed entry times and order, normalized
permissions; see `bindings/kotlin/build.gradle.kts`), though F-Droid only needs the libraries.

## A recipe for the app

In the app's `metadata/<appid>.yml`, add this repository as a `srclib` at the release tag the app uses, and
before the Gradle build:

```yaml
srclibs:
  - marknotes-sync@vX.Y.Z
prebuild:
  - pushd $$marknotes-sync$$ && cargo vendor --locked --versioned-dirs vendor && popd
build:
  - pushd $$marknotes-sync$$ && scripts/fdroid-build.sh --offline && popd
  # then point the app's Gradle build at $$marknotes-sync$$/bindings/kotlin (jar + natives/android)
ndk: r30
```

`scripts/fdroid-build.sh` then runs `scripts/kotlin-bindings.sh`, which builds the host library only to read the UniFFI metadata from it and generate the
Kotlin sources, which the app then compiles; it ships nothing.

## R8 / ProGuard

The Kotlin jar carries its keep rules at `META-INF/proguard/marknotes-sync.pro` (JNA and the generated bindings
are called through reflection and JNI). R8 picks them up automatically; an app needs no rules of its own. CI runs
the shared Kotlin tests on an emulator against an R8-minified test APK that uses only these rules
("Android emulator (E2E, …)").

## Licenses

All dependencies must be compatible with GPL-3.0-or-later and come from crates.io: `deny.toml`, enforced by the
"Dependencies (cargo-deny)" job.