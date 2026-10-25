# Testing marknotes-sync

marknotes-sync decides what every MarkNotes app writes, uploads and deletes, so a bug here can lose notes on every
device at once. The tests are layered so that each layer catches what the one before it can't, and every
behaviour they check is traced back to [marknotes-spec](https://github.com/MaxDac/marknotes-spec) (`spec/`).

| Layer | Where | What it proves | Runs |
| --- | --- | --- | --- |
| Spec fixtures | `crates/marknotes-sync/tests/spec_fixtures.rs`, `bindings/npm/test/fixtures.test.mjs`, `bindings/kotlin/src/sharedTest/.../FixturesTest.kt` | The shared spec's examples, in Rust, Node/wasm, the JVM and Android | CI |
| Unit tables | `tests/unit_*.rs` | Every rule of the spec as a table of cases, one row per rule or edge | CI |
| Regressions | `tests/regressions.rs` | One test per defect in the register below | CI |
| Properties | `tests/properties.rs`, `tests/support/gen.rs` | Invariants (below) over generated inputs, 256 cases each with a fixed seed | CI; 4096 cases with a new seed nightly |
| Simulation | `tests/simulation.rs`, `tests/sim/` | The core driven by a reference executor over several devices and drives, with failures, crashes, mid-sync edits and concurrent devices, checked by an independent no-loss oracle | CI; 5000 histories with a new seed nightly |
| Binding parity | `tests/parity_corpus.rs`, `bindings/npm/test/parity.test.mjs`, `bindings/kotlin/src/sharedTest/.../ParityTest.kt` | The same inputs give the same JSON in Rust, wasm and JNA | CI |
| Binding boundaries | `bindings/npm/test/boundary.test.mjs`, `BindingsTest.kt`, `bindings/npm/test/types/` | Numbers, nulls, invalid input, errors that leave the library usable, and the TypeScript declarations | CI |
| Package smoke | `bindings/npm/test/smoke.mjs`, `scripts/check-artifacts.sh` | The packed tgz installs and exports what `index.d.ts` declares; the jar, natives jar and AAR hold what the apps load | CI, release |
| Android E2E | `bindings/android-test/` | The AAR on emulators (API 29, 35 with 16 KB pages, 36), debug and R8-minified | CI |
| Native libraries | `scripts/check-android-libs.sh` | ELF: 16 KB page alignment, the exported symbols, no build paths, stripped | CI, release |
| Reproducibility | CI job "Reproducible Android libraries", `scripts/fdroid-build.sh` | Two builds in different directories and Cargo homes give identical `.so` files (what F-Droid checks) | CI |
| Dependencies | `deny.toml` | Licences compatible with GPL-3.0, crates.io only, no duplicate majors; advisories nightly | CI, nightly |
| Fuzzing | `fuzz/` (checks in `fuzz/src/lib.rs`) | No panic and the invariants hold on arbitrary bytes, for every entry point | CI replays the checks on the fixtures, `fuzz/regressions/` and seeded mutations; nightly libFuzzer, 15 minutes per target |
| Mutation testing | nightly job | Changes to the core's code that no test notices | Nightly (report) |
| Coverage | nightly job | Line and region coverage of the core | Nightly (report) |

## Running the tests

```sh
git submodule update --init                       # the spec fixtures
cargo test --workspace --all-features             # all Rust layers, as CI runs them
cargo test -p marknotes-sync --test simulation    # one layer
cargo test -p marknotes-sync --test parity_corpus -- --ignored   # the parity corpus the binding tests need
cd bindings/npm && npm ci && npm run build && npm test && npm run typecheck && npm pack && npm run smoke -- marknotes-sync-*.tgz
scripts/kotlin-bindings.sh && (cd bindings/kotlin && ./gradlew test)
(cd fuzz && cargo test --release --lib)          # the fuzz checks, replayed without libFuzzer
cargo +nightly fuzz run plan -- -max_total_time=60   # libFuzzer, from the repository root (Linux, macOS)
```

The binding parity tests read the corpus that the Rust side writes:
`cargo test -p marknotes-sync --test parity_corpus -- --ignored` writes `target/parity/<op>.json` (inputs and
Rust's outputs); the npm and Kotlin tests compare their results with it, and fail when it is missing.
The same command executes all `sync/executor-safety.json` scenarios and writes their observed store results to
`target/parity/executor-safety.json`. Binding fixture runners compare that evidence with the canonical execution
fixtures; their plan/reconcile/local-deletions fixtures separately exercise the actual binding APIs. Android test
assets include the generated evidence. Adapter races still require each app's own tests.

Deferred-folder fixtures cover durable marker/notice creation, acknowledgement and restart; new, changed and
ignored descendants; all-drive local recreation; identity replacement (including a failed child download and
restart); and capability upgrades. The reference executor also tests lost responses, incomplete listings, parent
case-rename notice migration and crashes after replacement-folder recording. `recreatedFolders` is included in
the generated Rust/WASM/Kotlin parity corpus. These tests never replace provider/store tests in the apps.

Android E2E (needs an emulator or a device, `ANDROID_HOME`, and the libraries from `scripts/android-libs.sh`):

```sh
scripts/android-libs.sh && scripts/kotlin-bindings.sh
bindings/kotlin/gradlew -p bindings/android-test connectedDebugAndroidTest
bindings/kotlin/gradlew -p bindings/android-test -PtestBuildType=minified connectedMinifiedAndroidTest   # with R8
```

`rust-toolchain.toml` pins the Rust version, so local results match CI. The crate's `rust-version` (MSRV) applies to
the library; the tests need the newer Rust that proptest needs.

## Seeds and reproducing a failure

All randomness is seeded, and the default seeds are fixed, so a plain `cargo test` is reproducible.

| Variable | Used by | Default |
| --- | --- | --- |
| `MARKNOTES_PROPTEST_SEED` | `properties.rs` (decimal, or hex with `0x`) | fixed in `tests/support/gen.rs` |
| `PROPTEST_CASES` | `properties.rs` | 256 |
| `MARKNOTES_SIM_SEED`, `MARKNOTES_SIM_RUNS` | `simulation.rs`, `randomized` | fixed, 200 runs |
| `MARKNOTES_PARITY_SEED`, `MARKNOTES_PARITY_CASES` | `parity_corpus.rs` | fixed |
| `MARKNOTES_SPEC_DIR` | `spec_fixtures.rs` and the binding fixture tests | `spec/` |

### Reproducing a nightly failure

- **Properties or simulation**: the log prints the exact command, for example
  `MARKNOTES_PROPTEST_SEED=123 MARKNOTES_SIM_SEED=123 PROPTEST_CASES=4096 MARKNOTES_SIM_RUNS=5000 cargo test --release -p marknotes-sync --test properties --test simulation`.
  A failing simulation history prints its own seed, to rerun with `MARKNOTES_SIM_RUNS=1`. proptest shrinks a
  failure and prints the minimal input.
- **Fuzzing**: download the `fuzz-crash-<target>` artifact and run `cargo +nightly fuzz run <target> <file>` (Linux or
  macOS; libFuzzer doesn't link with MSVC). Once fixed, add the file to `fuzz/regressions/<target>/`, where
  `cargo test --lib` in `fuzz/` (and CI) replays it on every platform.
- Turn the minimal input into a test in `regressions.rs` (with a new defect ID) or into a spec fixture, then fix it.

### Policy

- A failing test is never weakened to make it pass. If the spec allows the behaviour, the test becomes a
  characterisation (it says so) and the behaviour goes to the spec as a question.
- A behaviour change starts in marknotes-spec (see AGENTS.md).
- A surviving mutant either gets a test or a note in the nightly summary saying why it is equivalent.

## Invariants

The IDs appear in test names and in [`docs/executor-contract.md`](docs/executor-contract.md).

| ID | Invariant | Tests |
| --- | --- | --- |
| `PLAN-TOTAL` | `plan` never panics, on any input | properties, fuzz `plan` |
| `PLAN-DET` | The plan doesn't depend on the order of the listings or records | properties, fuzz `plan` |
| `PLAN-ERR` | An errored path gets no other action and isn't forgotten; nothing under a case collision or duplicate gets an action | unit_plan_table, properties, regressions D2–D4 |
| `PLAN-ORDER` | Errors first; parents created before children; individually guarded files deleted before deepest-first empty folders | unit_plan_table, properties |
| `PLAN-ONE` | At most one change per path (a manifest `record` may precede an `uploadRevision`) | properties |
| `PLAN-IGNORED` | Entries that don't sync get no action but keep their folder | unit_plan_table, properties |
| `PLAN-NOOP` | Identical sides with matching records give an empty plan | properties |
| `PLAN-CHANGE-WINS` | A change on one side is never turned into a deletion by the other side's deletion | unit_plan_table, properties, simulation |
| `MERGE-IDENT` | `merge3(b,x,x)=x`, `merge3(b,b,r)=r`, `merge3(b,l,b)=l`, with no conflicts | unit_merge_reconcile, properties |
| `MERGE-PRESERVE` | Every line added on either side is in the result or in a conflict, also beyond the edit-distance bound | properties, regressions D10, fuzz `merge` |
| `MERGE-LINES` | `lines` splits losslessly; matches are a common subsequence | properties |
| `MERGE-RENDER` | `mergeText` renders exactly the chunks and choices, with markers only for unresolved conflicts | unit_merge_reconcile, properties, fuzz `merge` |
| `MERGE-LIMIT` | Defined results at the edit-distance bound and for long inputs | unit_merge_reconcile, regressions D10 |
| `RECON-NOLOSS` | When the newest wins, the other text is kept byte for byte (apart from its new id) in a copy whose name is free | properties, simulation, fuzz `reconcile` |
| `MAN-ROUNDTRIP` | A manifest parses back to itself, for any paths and timestamps | properties, fuzz `manifest` |
| `MAN-CANON` | Keys and paths are written in the spec's order | unit_manifest_scheduler |
| `MAN-REJECT` | Each of the spec's rejection rules; never a panic | unit_manifest_scheduler, fuzz `manifest` |
| `MAN-SKIP` | `buildManifest` skips the write only when the previous manifest parsed and holds the same files | unit_manifest_scheduler, properties |
| `FM-ID` | `readId(withId(t, id)) = id`; the body and the rest of the block are kept byte for byte; idempotent | unit_paths_fm_hash, properties, regressions D5 D14, fuzz `front_matter` |
| `PATH-NORM` | `normalize` and `foldCase` are idempotent; a syncable path is normalised | unit_paths_fm_hash, properties, fuzz `paths` |
| `PATH-COPY` | A conflicted copy name is free under case folding and in the same folder | properties, regressions D1, fuzz `paths` |
| `SCHED-TERM` | The scheduler terminates, each drive syncs at least once and at most `maxPasses` times | unit_manifest_scheduler, properties |
| `HASH-VEC` | SHA-256 matches the NIST vectors | unit_paths_fm_hash |
| `SIM-*` | No loss, order of conflict steps, failures and restarts, mid-sync edits, conditional writes, modification times, manifest re-uploads, convergence, idempotence, several drives | simulation (`sim_*`, `story_*`, `randomized`); defined in `tests/sim/mod.rs` |
| `BIND-PARITY` | Rust, wasm and JNA return the same JSON for the same input | parity tests |
| `BIND-BOUNDARY` | Numbers (2^53, `i64` limits, fractions, NaN), null and omitted arguments, NUL and unpaired surrogates | boundary.test.mjs, BindingsTest.kt |
| `BIND-ERR` | Invalid input is a catchable error, and the next call works | boundary.test.mjs, BindingsTest.kt |
| `BIND-API` | Constants, defaults and every export match `index.d.ts` | types/usage.ts, smoke.mjs |

## Defect register

Found while hardening the tests for the F-Droid release. Each has a test in `regressions.rs` (or in the bindings'
tests) that fails without the fix.

| ID | Defect | Resolution |
| --- | --- | --- |
| D1 | Conflicted-copy names compared with `to_lowercase`, not simple case folding: a copy could overwrite an earlier one | Fixed: `fold_case` |
| D2 | A file/folder clash: what the spec does with the clashing path's contents | Characterised: only the clashing path is an error |
| D3 | A path listed twice: the last entry silently won | Fixed: an error, whatever the order |
| D4 | A folder a listing omitted although it listed entries inside it was deleted | Fixed: an implied folder is never deleted |
| D5 | A BOM before the front matter made `withId` add a second block | Fixed with MaxDac/marknotes-spec#33 |
| D6 | Names with control characters were syncable | Fixed with MaxDac/marknotes-spec#34 |
| D7 | Kotlin turned negative scheduler counts into 0; wasm defaulted silently | Fixed: rejected in both bindings (0.2.0) |
| D8 | wasm cast NaN, infinities, fractions and unsafe integers to `i64` | Fixed: rejected with an error (0.2.0) |
| D9 | A wasm panic left the instance unusable | Fixed: the module is reset after a trap; fuzzing for panics |
| D10 | Myers' diff uses memory quadratic in the edit distance (about 32 MB at the bound) | Characterised; bounded by `MAX_EDIT_DISTANCE` |
| D11 | The merge without a base depends on which side is first | Specified by MaxDac/marknotes-spec#35 |
| D12 | Fractional and huge manifest timestamps | Characterised: truncated toward zero, saturated |
| D13 | The README gave another JNA version than the build | Fixed |
| D14 | `withId` rewrote the whole front-matter block, changing line endings and stray carriage returns | Fixed: only the `id` line changes |
| D15 | npm results were built by assignment, so `parseManifest` lost a file named `__proto__` | Fixed: results go through `JSON.parse`; integers that aren't safe still throw (found by the parity corpus) |
| D16 | Case folding lowercased characters outside a hand-written list, missing `S` mappings such as U+FB05 → U+FB06 and U+1FD3 → U+0390 | Fixed: a table generated from Unicode 17.0.0 CaseFolding.txt (`scripts/gen-case-folding.py`) |
| D17 | npm silently defaulted a missing `version`, `modifiedMillis`, `remoteVersion`, `localHash` or `syncedAt` to `""` or 0 | Fixed: missing fields are an error, as `index.d.ts` and Kotlin already required |
| D18 | npm `free()` on a scheduler made before a trap reset freed whatever the new instance had at that address | Fixed: a stale scheduler is only detached |
| D19 | npm `plan` threw on `remoteFolderDeletion: undefined` (#18); every defaulted field rejected `undefined` and `null` | Fixed: they mean omitted (0.4.1); a wrong type still throws |
| D20 | Nightly (seed 1): `preserveRemoteFolder` after a local folder deletion failed the ordering property | Property was stale: preservation replaces `deleteRemoteFolder` in the deepest-first folder deletions (0.4.1) |
| D21 | Nightly (seed 1): a folder case rename was inferred while a descendant was recorded under the other spelling | Fixed: every case-folded descendant must map one-to-one, else the case errors stand (0.4.1, MaxDac/marknotes-spec#59) |
| D22 | Nightly (seed 1): a case rename came before its parent's `recordFolder` | Property was stale: renames follow the errors and may precede an ancestor's `recordFolder` (MaxDac/marknotes-spec#59) |
| D23 | Nightly fuzz (`reconcile`): a losing text that starts with a BOM and has no front matter failed the copy-body check | Check was stale: the BOM moves before the copy's new block (MaxDac/marknotes-spec#33); `fuzz/regressions/reconcile/` |
| D24 | Nightly fuzz (`reconcile`): a `newId` containing a line break and `---` ended the copy's block early | Check was too strong: `newId` must be a fresh note id (8 characters of the id alphabet), so the copy-body check runs only for valid ids; `fuzz/regressions/reconcile/` |

## Safety regressions resolved in 0.3

The historical `gap_*` names remain stable, but now assert the corrected canonical behavior.

| Test | Finding | Spec issue |
| --- | --- | --- |
| `gap_folder_delete_races_concurrent_upload` | Concurrent unseen children survive atomic empty-only folder removal | MaxDac/marknotes-spec#38 |
| `gap_restored_note_deleted_through_stale_record` | Per-drive deletion epochs prevent stale deletions removing a restored note | MaxDac/marknotes-spec#37 |
| `gap_conflicted_copy_mtime` | Copies retain the losing edit time, including replay and upload | MaxDac/marknotes-spec#40 |
| `gap_crash_after_copy_duplicates_it` | Durable pre-write intent replays one verified copy with the same id | MaxDac/marknotes-spec#42 |
| `gap_case_rename_missed_by_another_device` | Record-proven unchanged stale aliases converge | MaxDac/marknotes-spec#39 |

The canonical spec and [executor contract](docs/executor-contract.md) require create-only files, copies and folders
(MaxDac/marknotes-spec#41); `sync_executor_safety` checks the after-listing races and all six conflict crash points.