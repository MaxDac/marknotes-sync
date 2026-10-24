// Installs the packed tgz into an empty project, as an app would, and checks that it loads, works, and exports
// exactly what index.d.ts declares. Usage: npm run smoke -- <marknotes-sync-X.Y.Z.tgz>
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const tgz = resolve(process.argv[2] ?? assert.fail("usage: smoke.mjs <tgz>"));
const dir = mkdtempSync(join(tmpdir(), "marknotes-sync-smoke-"));
try {
  writeFileSync(join(dir, "package.json"), JSON.stringify({ name: "smoke", private: true, type: "module" }));
  execFileSync("npm", ["install", "--no-audit", "--no-fund", "--ignore-scripts", tgz], {
    cwd: dir, stdio: "inherit", shell: process.platform === "win32",
  });
  const pkg = join(dir, "node_modules", "@marknotes", "sync");
  const sync = await import(pathToFileURL(join(dir, "node_modules", "@marknotes", "sync", "index.js")).href);

  const declared = [...readFileSync(join(pkg, "index.d.ts"), "utf8")
    .matchAll(/^export (?:declare )?(?:function|class|const) (\w+)/gm)].map((m) => m[1]).sort();
  assert.deepEqual(Object.keys(sync).sort(), [...new Set(declared)].sort(), "runtime exports differ from index.d.ts");

  const actions = sync.plan({
    local: [{ path: "a.md", isFolder: false, modifiedMillis: 1, sha256: sync.sha256Hex(new TextEncoder().encode("a")) }],
    remote: [],
    records: [],
  }).actions;
  assert.deepEqual(actions, [{ kind: "uploadNew", path: "a.md" }]);
  const scheduler = new sync.SyncScheduler(["x"]);
  assert.equal(scheduler.next(), "x");
  scheduler.free();
  console.log(`ok: ${tgz} installs and works`);
} finally {
  rmSync(dir, { recursive: true, force: true });
}
