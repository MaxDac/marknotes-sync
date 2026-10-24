# marknotes-sync

The shared sync core of the MarkNotes apps ([Android](https://github.com/MaxDac/MarkNotes),
[desktop](https://github.com/MaxDac/marknotes-electron), [web](https://github.com/MaxDac/marknotes-web)). One Rust
implementation of the sync plan, conflict reconciliation, three-way merge and `.marknotes/manifest.json`, exposed to
Kotlin through [UniFFI](https://mozilla.github.io/uniffi-rs/) and to TypeScript through WebAssembly.

The behaviour is specified in [marknotes-spec](https://github.com/MaxDac/marknotes-spec) (`note-format.md`, "Sync")
and tested against its fixtures (the `spec/` submodule).

## Running a plan

The core does no I/O. A sync pass in an app is:

1. List the local tree and the drive (`LocalEntry`, `RemoteEntry`: path, folder flag, mtime, local SHA-256 or remote id and version) and load the
   records of the previous sync (`SyncRecord`). Include the entries that are not synced (other files, hidden files
   and folders): they keep their folder from being deleted.
2. `plan(input)` returns ordered actions (`kind`: `createRemoteFolder`, `createLocalFolder`, `uploadNew`,
   `uploadRevision`, `download`, `deleteLocal`, `deleteRemote`, `reconcile`, `record`, `forget`, `error`, ...). Run each one with the app's stores; a
   failing action adds `"<path>: <message>"` to the pass's errors and the pass goes on.
3. For `reconcile` (a note changed on both sides), download the remote text, read the base copy if the record has
   one and call `reconcile({ path, base, local, localModifiedMillis, remote, remoteEntry, existingPaths, newId })`:
   - `same`: record the path and update the base copy;
   - `merged`: write the merged text locally, upload it, record it;
   - `newestWins`: keep `winner` on both sides, write `copyText` (the loser, with the fresh id) to `copyPath`,
     upload it, record both and show the conflict. The merge-back UI calls `merge3(base, note, copy)` and `mergeText(chunks, choices)`.
4. `buildManifest(records, deviceId, nowMillis, previousManifestText)` returns the new manifest text, or nothing when
   it hasn't changed; upload it to `.marknotes/manifest.json`.

With several drives, `SyncScheduler(driveIds, maxPasses)` says which drive to sync next: call `next()`, sync that
drive, `report(id, localChanges, ok)`, until `next()` returns nothing.

The apps must run the actions as [docs/executor-contract.md](docs/executor-contract.md) says (conditional uploads and
deletions, what to do after a failure or a crash, when to save the records); the simulation tests check the core
against exactly that executor.

Invalid arguments are errors, not silent defaults (since 0.2.0): Kotlin throws `SyncException.InvalidArgument` (a
negative `maxPasses` or `localChanges`), and JavaScript throws an `Error` for a number that isn't a safe integer, a
negative count or a missing field. Either way the next call works. If the WebAssembly ever traps, the call throws
and the module is instantiated again.

## Using it

Releases publish, for tag `vX.Y.Z`:

| Asset | For |
| --- | --- |
| `marknotes-sync-X.Y.Z.tgz` | npm package `@marknotes/sync` (desktop and web) |
| `marknotes-sync-kotlin-X.Y.Z.jar` | Kotlin bindings, package `io.github.maxdac.marknotes.sync` (needs JNA) |
| `marknotes-sync-android-X.Y.Z.aar` | `libmarknotes_sync.so` for `arm64-v8a` and `x86_64` |
| `marknotes-sync-natives-X.Y.Z.jar` | host libraries (Linux x86-64, Windows x86-64, macOS arm64 and x86-64) for JVM unit tests |
| `SHA256SUMS` | checksums |

npm (Electron, Phoenix assets):

```sh
npm install https://github.com/MaxDac/marknotes-sync/releases/download/v0.2.0/marknotes-sync-0.2.0.tgz
```

```ts
import { plan, reconcile, buildManifest } from "@marknotes/sync";
```

The WebAssembly is inlined and instantiated synchronously on import, so it works in Node, Electron and browsers
without bundler configuration.

Gradle (Android):

```kotlin
// settings.gradle.kts
repositories {
    ivy("https://github.com/MaxDac/marknotes-sync/releases/download") {
        patternLayout { artifact("v[revision]/[module]-[revision].[ext]") }
        metadataSources { artifact() }
        content { includeGroup("io.github.maxdac.marknotes") }
    }
}

// :core (pure JVM)
implementation("io.github.maxdac.marknotes:marknotes-sync-kotlin:0.2.0")
implementation("net.java.dev.jna:jna:5.19.1")
testRuntimeOnly("io.github.maxdac.marknotes:marknotes-sync-natives:0.2.0")

// :app
implementation("io.github.maxdac.marknotes:marknotes-sync-android:0.2.0@aar")
implementation("net.java.dev.jna:jna:5.19.1@aar")
```

The jar carries the R8/ProGuard rules JNA needs (`META-INF/proguard/marknotes-sync.pro`), so minified release builds
work without extra rules; the Android E2E tests run against an R8-minified build.

## Building

Requirements: Rust (the version in `rust-toolchain.toml`, which rustup installs), plus for the bindings the
`wasm32-unknown-unknown` target, `wasm-bindgen-cli` at the version pinned in `crates/marknotes-sync/Cargo.toml`, Node 18+, JDK 17+, and for Android `cargo-ndk` with an NDK.

```sh
git submodule update --init
cargo test --workspace --all-features                      # core, spec fixtures, properties, simulation
(cd bindings/npm && npm run build && npm test)             # npm package
scripts/kotlin-bindings.sh && (cd bindings/kotlin && ./gradlew test)   # Kotlin bindings on the JVM
scripts/android-libs.sh                                    # Android .so files
```

[TESTING.md](TESTING.md) describes every test layer, the invariants and how to reproduce a failure;
[docs/fdroid.md](docs/fdroid.md) how to build the Android libraries reproducibly from source, as F-Droid does. See
[AGENTS.md](AGENTS.md) for the layout, rules and release process.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
