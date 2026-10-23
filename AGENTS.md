# AGENTS.md

Canonical instructions for coding agents (Claude Code, GitHub Copilot, others) working on **marknotes-sync**, the
shared sync core of the MarkNotes apps. `CLAUDE.md` and `.github/copilot-instructions.md` point here.

## What this is

One Rust crate (`crates/marknotes-sync`) holding the sync algorithm all three apps run: the sync plan, conflict
reconciliation (three-way merge, then newest wins with a conflicted copy), the `.marknotes/manifest.json` format,
the several-drives scheduler, and the path, front-matter and hashing helpers sync needs. It is **sans-IO**: no files,
network, clock or randomness. Apps list their trees, call `plan`, run the actions with their own stores, call
`reconcile` for notes changed on both sides and `buildManifest` at the end (see the README, "Running a plan").

| Consumer | Binding | Artifact |
| --- | --- | --- |
| [MarkNotes](https://github.com/MaxDac/MarkNotes) (Android) | UniFFI Kotlin + JNA | `marknotes-sync-kotlin-<v>.jar`, `marknotes-sync-android-<v>.aar` (+ `marknotes-sync-natives-<v>.jar` for JVM tests) |
| [marknotes-electron](https://github.com/MaxDac/marknotes-electron) | wasm-bindgen, npm | `marknotes-sync-<v>.tgz` |
| [marknotes-web](https://github.com/MaxDac/marknotes-web) | wasm-bindgen, npm (runs in the browser) | `marknotes-sync-<v>.tgz` |

The behaviour is specified in [marknotes-spec](https://github.com/MaxDac/marknotes-spec) (`note-format.md`, "Sync"),
checked out as the `spec/` submodule (`git submodule update --init`).

## Layout

| Path | Contents |
| --- | --- |
| `crates/marknotes-sync/src/` | `plan.rs`, `reconcile.rs`, `merge.rs`, `manifest.rs`, `scheduler.rs`, `paths.rs`, `front_matter.rs`, `hashing.rs`, `model.rs`; `ffi.rs` (UniFFI, feature `uniffi`) and `wasm.rs` (wasm-bindgen, feature `wasm`) |
| `crates/marknotes-sync/tests/spec_fixtures.rs` | Runs every fixture under `spec/fixtures/` this crate covers (`MARKNOTES_SPEC_DIR` overrides the path) |
| `uniffi-bindgen/` | The UniFFI bindgen binary, pinned to the crate's UniFFI version |
| `bindings/npm/` | npm package `@marknotes/sync`: `build.mjs` (wasm inlined as base64), hand-written `index.d.ts`, Node fixture tests |
| `bindings/kotlin/` | Gradle project: tests the generated Kotlin bindings on the JVM and packs the jar, natives jar and AAR |
| `scripts/` | `kotlin-bindings.sh` (host library + Kotlin bindings), `android-libs.sh` (cargo-ndk, arm64-v8a and x86_64) |

## Build and test

These mirror `.github/workflows/ci.yml`:

| Check | Command |
| --- | --- |
| Rust | `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`, `cargo test --workspace --locked` |
| npm | `cd bindings/npm && npm run build && npm test` (needs the `wasm32-unknown-unknown` target and `wasm-bindgen-cli` at the version pinned in `crates/marknotes-sync/Cargo.toml`) |
| Kotlin | `scripts/kotlin-bindings.sh && (cd bindings/kotlin && ./gradlew test)` (JDK 17+) |
| Android libraries | `scripts/android-libs.sh` (needs cargo-ndk and an NDK) |

On Windows run the scripts from Git Bash, and use `.\gradlew.bat`.

## Rules

1. **Spec first.** A behaviour change lands in marknotes-spec (`note-format.md`, fixtures, `features.md`) first;
   then implement it here and bump `spec/` in the same PR. Never weaken an expectation to make a fixture pass.
2. **Keep the three APIs in step.** A change to a public type or function updates `ffi.rs`, `wasm.rs`,
   `bindings/npm/index.d.ts` and the Node and Kotlin tests. JSON field names are camelCase and actions are tagged
   with `kind`, matching the spec fixtures.
3. **No I/O in the core.** Anything that needs time, ids or files is passed in (`nowMillis`, `newId`,
   `existingPaths`) or done by the apps.
4. `wasm-bindgen` is pinned exactly; bump it together with `WASM_BINDGEN_VERSION` in the workflows.
5. Breaking API changes bump the minor version (0.x) in `Cargo.toml` and `bindings/npm/package.json` together.

## Release

Push a tag `vX.Y.Z` matching `Cargo.toml` and `bindings/npm/package.json`. `release.yml` reruns CI, builds the
host libraries (Linux, Windows, macOS arm64 and x86_64) and the Android libraries, and publishes the npm tgz, the
Kotlin jar, the natives jar, the AAR and `SHA256SUMS` as a GitHub release. Then bump the version in the apps.
