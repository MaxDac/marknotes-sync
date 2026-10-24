import * as glue from "./generated/marknotes_sync.js";
import wasmBase64 from "./generated/wasm-base64.js";
import { guard, isTrap } from "./recover.js";

function decode(base64) {
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

glue.initSync({ module: decode(wasmBase64) });

const reset = glue.__wbg_reset_state;

// The id the generated glue gives the current instance; a scheduler made before a reset carries an older one.
function currentInstance() {
  const probe = new glue.SyncScheduler([], 1);
  const id = probe.__wbg_inst;
  probe.free();
  return id;
}

export const plan = guard(glue.plan, reset);
export const localDeletions = guard(glue.localDeletions, reset);
export const reconcile = guard(glue.reconcile, reset);
export const merge3 = guard(glue.merge3, reset);
export const mergeText = guard(glue.mergeText, reset);
export const parseManifest = guard(glue.parseManifest, reset);
export const serializeManifest = guard(glue.serializeManifest, reset);
export const buildManifest = guard(glue.buildManifest, reset);
export const sha256Hex = guard(glue.sha256Hex, reset);
export const conflictCopyName = guard(glue.conflictCopyName, reset);
export const isSyncablePath = guard(glue.isSyncablePath, reset);
export const isSyncedEntry = guard(glue.isSyncedEntry, reset);
export const isAttachmentPath = guard(glue.isAttachmentPath, reset);
export const readId = guard(glue.readId, reset);
export const withId = guard(glue.withId, reset);

export class SyncScheduler extends glue.SyncScheduler {
  constructor(driveIds, maxPasses) {
    try {
      super(driveIds, maxPasses);
    } catch (error) {
      if (isTrap(error)) reset();
      throw error;
    }
  }

  // The generated free() does not check the instance: on a stale scheduler it would free whatever the new instance
  // has at the same address. A stale scheduler's memory went with the old instance, so only detach it.
  free() {
    if (this.__wbg_ptr !== 0 && this.__wbg_inst !== currentInstance()) {
      this.__destroy_into_raw();
      return;
    }
    guard(glue.SyncScheduler.prototype.free, reset).call(this);
  }
}
SyncScheduler.prototype.next = guard(glue.SyncScheduler.prototype.next, reset);
SyncScheduler.prototype.report = guard(glue.SyncScheduler.prototype.report, reset);
if (Symbol.dispose) SyncScheduler.prototype[Symbol.dispose] = SyncScheduler.prototype.free;

export const MANIFEST_PATH = ".marknotes/manifest.json";
export const DEFAULT_MAX_PASSES = 3;
