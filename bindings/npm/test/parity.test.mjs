// Parity corpus: the Rust core generated seeded, randomized inputs and recorded its own outputs; this test replays
// every case through the WebAssembly package and requires exactly the same outputs, so the JS boundary
// (serialization, numbers, strings with non-BMP characters, byte order marks and control characters, null and
// undefined, keys such as __proto__) neither adds to nor loses anything from what the core computes. The Kotlin
// bindings replay the same corpus (ParityTest.kt).
//
// The corpus is target/parity/<op>.json at the repository root (MARKNOTES_PARITY_DIR overrides the folder). Generate
// it with `cargo test -p marknotes-sync --test parity_corpus --locked -- --ignored` (CI does before running the
// tests); see crates/marknotes-sync/tests/parity_corpus.rs.
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import * as sync from "../index.js";

const parityDir = process.env.MARKNOTES_PARITY_DIR ?? fileURLToPath(new URL("../../../target/parity", import.meta.url));
const generate = "cargo test -p marknotes-sync --test parity_corpus --locked -- --ignored";

function read(file) {
  const path = join(parityDir, file);
  assert.ok(existsSync(path), `parity corpus missing: ${path}; generate it with \`${generate}\` at the repository root`);
  return JSON.parse(readFileSync(path, "utf8"));
}

const runners = {
  recreatedFolders: (i) => ({ paths: sync.recreatedFolders(i.local, i.records) }),
  plan: (i) => sync.plan(i),
  reconcile: (i) => sync.reconcile(i),
  merge3: (i) => {
    const result = sync.merge3(i.base, i.local, i.remote);
    const text = i.choices ? sync.mergeText(result.chunks, i.choices) : result.text;
    return { text, conflicts: result.conflicts, hasConflicts: result.hasConflicts, chunks: result.chunks };
  },
  parseManifest: (i) => ({ manifest: sync.parseManifest(i.json) }),
  serializeManifest: (i) => ({ json: sync.serializeManifest(i) }),
  buildManifest: (i) => ({ json: sync.buildManifest(i.records, i.deviceId, i.nowMillis, i.previous) ?? null }),
  conflictCopyName: (i) => ({ path: sync.conflictCopyName(i.path, i.existingPaths) }),
  isSyncablePath: (i) => ({ syncable: sync.isSyncablePath(i.path) }),
  isSyncedEntry: (i) => ({ synced: sync.isSyncedEntry(i.path, i.isFolder) }),
  isAttachmentPath: (i) => ({ isAttachment: sync.isAttachmentPath(i.path) }),
  readId: (i) => ({ id: sync.readId(i.text) ?? null }),
  withId: (i) => ({ text: sync.withId(i.text, i.id) }),
  sha256Hex: (i) => ({ sha256: sync.sha256Hex(Uint8Array.from(i.bytes)) }),
  syncScheduler: (i) => {
    const scheduler = new sync.SyncScheduler(i.driveIds, i.maxPasses);
    try {
      const order = [];
      for (let id = scheduler.next(); id !== undefined; id = scheduler.next()) {
        const r = i.reports[order.length];
        scheduler.report(id, r.localChanges, r.failed);
        order.push(id);
      }
      return { order };
    } finally {
      scheduler.free();
    }
  },
};

for (const [op, run] of Object.entries(runners)) {
  test(`parity: ${op}`, () => {
    const { cases } = read(`${op}.json`);
    assert.ok(cases.length > 0, `parity/${op}.json has no cases`);
    cases.forEach((c, index) => {
      let actual;
      try {
        actual = run(c.input);
      } catch (e) {
        assert.fail(`parity/${op}.json case ${index} (${c.name}) threw: ${e}\n  input ${JSON.stringify(c.input)}`);
      }
      assert.deepEqual(actual, c.expected, `parity/${op}.json case ${index} (${c.name})`);
    });
  });
}

// D15: results were built by assignment, so a manifest path "__proto__" set the object's prototype instead of adding
// an own key, and the entry disappeared from Object.keys, deepEqual and serializeManifest. Results are now built by
// JSON.parse; the core and the Kotlin bindings always kept it.
test("parity: parseManifest keeps a file path named __proto__ (D15)", () => {
  const entry = { sha256: "h", version: "v", syncedAt: 2 };
  const text = JSON.stringify({ format: 1, deviceId: "d", updatedAt: 1, files: { ["__proto__"]: entry } });
  const { files } = sync.parseManifest(text);
  assert.ok(Object.hasOwn(files, "__proto__"), "__proto__ is not an own key of files");
  assert.deepEqual(files, JSON.parse(`{"__proto__":${JSON.stringify(entry)}}`));
});

test("parity: every operation of the corpus has a runner", () => {
  const { ops } = read("index.json");
  for (const op of Object.keys(ops)) assert.ok(op in runners, `parity/${op}.json has no runner`);
  for (const op of Object.keys(runners)) assert.ok(op in ops, `${op} is not in the corpus`);
});
