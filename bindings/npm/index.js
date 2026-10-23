import { initSync } from "./generated/marknotes_sync.js";
import wasmBase64 from "./generated/wasm-base64.js";

function decode(base64) {
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

initSync({ module: decode(wasmBase64) });

export {
  plan,
  reconcile,
  merge3,
  mergeText,
  parseManifest,
  serializeManifest,
  buildManifest,
  sha256Hex,
  conflictCopyName,
  isSyncablePath,
  isSyncedEntry,
  isAttachmentPath,
  readId,
  withId,
  SyncScheduler,
} from "./generated/marknotes_sync.js";

export const MANIFEST_PATH = ".marknotes/manifest.json";
export const DEFAULT_MAX_PASSES = 3;
