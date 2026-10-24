// Builds the npm package: compiles the core to WebAssembly, runs wasm-bindgen and inlines the .wasm (base64) so that
// the package works unchanged in Node, Electron and browser bundlers. Needs cargo with the wasm32-unknown-unknown
// target and the wasm-bindgen CLI at the version pinned in crates/marknotes-sync/Cargo.toml.
import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..", "..");
const out = join(here, "generated");
const run = (cmd, args) => execFileSync(cmd, args, { cwd: root, stdio: "inherit" });

run("cargo", [
  "build", "--release", "--locked", "-p", "marknotes-sync",
  "--features", "wasm", "--target", "wasm32-unknown-unknown",
]);
rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });
run("wasm-bindgen", [
  "--target", "web", "--no-typescript", "--experimental-reset-state-function", "--out-dir", out, "--out-name", "marknotes_sync",
  join(root, "target", "wasm32-unknown-unknown", "release", "marknotes_sync.wasm"),
]);

// The default (async) init fetches the .wasm next to the module; the package only uses initSync, so drop that
// URL to keep bundlers from looking for a file that is not shipped.
const gluePath = join(out, "marknotes_sync.js");
const glue = readFileSync(gluePath, "utf8");
const urlPattern = /new URL\('marknotes_sync_bg\.wasm', import\.meta\.url\)/g;
if (!urlPattern.test(glue)) throw new Error("wasm-bindgen output changed: no default .wasm URL found");
if (!/export function __wbg_reset_state\b/.test(glue) || !glue.includes("wasm.__wbindgen_start()")) throw new Error("wasm-bindgen output changed: no __wbg_reset_state");
writeFileSync(gluePath, glue.replace(urlPattern, "undefined"));

const wasm = readFileSync(join(out, "marknotes_sync_bg.wasm"));
rmSync(join(out, "marknotes_sync_bg.wasm"));
writeFileSync(join(out, "wasm-base64.js"), `export default "${wasm.toString("base64")}";\n`);

const cargo = readFileSync(join(root, "Cargo.toml"), "utf8");
const version = /\[workspace\.package\][^[]*?version\s*=\s*"([^"]+)"/.exec(cargo)[1];
const pkg = JSON.parse(readFileSync(join(here, "package.json"), "utf8"));
if (pkg.version !== version) throw new Error(`package.json version ${pkg.version} != Cargo.toml ${version}`);
console.log(`built @marknotes/sync ${version} (${wasm.length} bytes of wasm)`);
