// Behaviour at the JS boundary that the spec fixtures do not cover: how numbers, null and undefined, and wrong types
// are handled, and recovery from a trap in the WebAssembly instance.
import assert from "node:assert/strict";
import { test } from "node:test";

import * as sync from "../index.js";
import * as glue from "../generated/marknotes_sync.js";
import { guard, isTrap } from "../recover.js";

const remote = (extra = {}) => ({ path: "a.md", id: "r", isFolder: false, version: "v", modifiedMillis: 0, ...extra });
const record = (extra = {}) => ({
  path: "a.md", isFolder: false, remoteId: "r", remoteVersion: "v", localHash: "h", syncedAt: 1, ...extra,
});

function drain(scheduler, changes = 0) {
  const order = [];
  for (let id = scheduler.next(); id !== undefined; id = scheduler.next()) {
    scheduler.report(id, changes, false);
    order.push(id);
  }
  return order;
}

test("SyncScheduler: maxPasses defaults to 3 when undefined or null", () => {
  for (const max of [undefined, null]) {
    const s = new sync.SyncScheduler(["a", "b"], max);
    assert.deepEqual(drain(s, 1), ["a", "b", "a", "b", "a", "b"]);
    s.free();
  }
  assert.equal(sync.DEFAULT_MAX_PASSES, 3);
});

test("SyncScheduler: maxPasses must be a non-negative integer", () => {
  for (const bad of [-1, 1.5, NaN, Infinity, 2 ** 32]) {
    assert.throws(() => new sync.SyncScheduler(["a"], bad), /maxPasses must be a non-negative integer/, String(bad));
  }
  const zero = new sync.SyncScheduler(["a"], 0);
  assert.equal(zero.next(), undefined);
  zero.free();
});

test("SyncScheduler: localChanges must be a non-negative integer, and a rejected report records nothing", () => {
  const s = new sync.SyncScheduler(["a", "b"], 3);
  assert.equal(s.next(), "a");
  for (const bad of [-1, 0.5, NaN, -Infinity, 2 ** 32]) {
    assert.throws(() => s.report("a", bad, false), /localChanges must be a non-negative integer/, String(bad));
  }
  assert.equal(s.next(), "b");
  s.report("b", 0, false);
  assert.equal(s.next(), undefined);
  s.free();
});

test("buildManifest: nowMillis must be a safe integer", () => {
  for (const bad of [1.5, NaN, Infinity, 2 ** 53]) {
    assert.throws(() => sync.buildManifest([record()], "d", bad, null), /nowMillis must be a safe integer/, String(bad));
  }
  for (const ok of [0, -1, Number.MAX_SAFE_INTEGER, -Number.MAX_SAFE_INTEGER]) {
    assert.equal(JSON.parse(sync.buildManifest([record()], "d", ok, null)).updatedAt, ok);
  }
});

test("object fields: integer times must be safe integers, other types are rejected", () => {
  const plan = (r) => sync.plan({ local: [], remote: [r], records: [] });
  assert.deepEqual(plan(remote({ modifiedMillis: Number.MAX_SAFE_INTEGER })).actions[0].remote.modifiedMillis,
    Number.MAX_SAFE_INTEGER);
  for (const bad of [1.5, 2 ** 53, NaN, "1", null]) {
    assert.throws(() => plan(remote({ modifiedMillis: bad })), Error, String(bad));
  }
  assert.throws(() => plan(remote({ isFolder: "false" })), Error);
  assert.throws(() => plan(remote({ path: 1 })), Error);
  assert.throws(() => sync.plan({ local: [], remote: [] }), /records/);
  assert.throws(() => sync.plan(null), Error);
});

test("object fields: optional fields accept undefined and null", () => {
  const out = sync.plan({ local: [{ path: "a.md", isFolder: false, sha256: "h" }], remote: [], records: [], manifest: null });
  assert.deepEqual(out.actions, [{ kind: "uploadNew", path: "a.md" }]);
  const r = sync.reconcile({
    path: "a.md", local: "x\n", localModifiedMillis: 1, remote: "x\n", remoteEntry: remote(), newId: "m4n5p6q7",
  });
  assert.deepEqual(r, { kind: "same" });
});

test("mergeText: conflict indexes must be integers", () => {
  const { chunks } = sync.merge3("a\n", "b\n", "c\n");
  assert.equal(sync.mergeText(chunks, { 0: "local" }), "b\n");
  assert.equal(sync.mergeText(chunks, undefined), sync.merge3("a\n", "b\n", "c\n").text);
  assert.throws(() => sync.mergeText(chunks, { x: "local" }), /invalid conflict index/);
  assert.throws(() => sync.mergeText(chunks, { 0: "mine" }), Error);
});

test("strings: lone surrogates are replaced, not rejected", () => {
  assert.equal(sync.isSyncablePath("a\uD800.md"), true);
  assert.equal(sync.conflictCopyName("a\uD800.md", []), "a\uFFFD (conflicted copy).md");
});

test("guard: a trap resets the instance and is rethrown; other errors are rethrown unchanged", () => {
  let resets = 0;
  const trap = new WebAssembly.RuntimeError("unreachable");
  assert.throws(guard(() => { throw trap; }, () => resets++), (e) => e === trap);
  assert.equal(resets, 1);
  const other = new Error("invalid");
  assert.throws(guard(() => { throw other; }, () => resets++), (e) => e === other);
  assert.equal(resets, 1);
  assert.equal(guard(function (x) { return this.k + x; }, () => {}).call({ k: 1 }, 2), 3);
  assert.equal(isTrap(trap), true);
  assert.equal(isTrap(other), false);
});

test("after a reset the module works and older schedulers refuse to run", () => {
  const before = new sync.SyncScheduler(["a"], 1);
  glue.__wbg_reset_state();
  assert.throws(() => before.next(), /stale object/);
  assert.equal(sync.sha256Hex(new TextEncoder().encode("abc")),
    "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
  const after = new sync.SyncScheduler(["a"], 1);
  assert.equal(after.next(), "a");
  after.free();
});
