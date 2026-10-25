// Types of the MarkNotes sync core. Field names match the marknotes-spec fixtures. Times are milliseconds since the
// epoch and must be safe integers; hashes are lowercase hex SHA-256. Functions taking objects throw an Error when a
// field is missing or has the wrong type. If the WebAssembly code ever traps, the call throws a
// WebAssembly.RuntimeError and the instance is replaced (see recover.js); SyncScheduler objects created before then
// throw when used.

/** A file or folder in the local note tree. `sha256` is required for files. */
export interface LocalEntry {
  path: string;
  isFolder: boolean;
  modifiedMillis?: number;
  sha256?: string | null;
}

/** A file or folder on a drive. `version` is `""` for folders. */
export interface RemoteEntry {
  path: string;
  id: string;
  isFolder: boolean;
  version: string;
  modifiedMillis: number;
}

/** What both sides looked like the last time they agreed on a path. */
export interface SyncRecord {
  path: string;
  isFolder: boolean;
  remoteId: string;
  remoteVersion: string;
  localHash: string;
  syncedAt: number;
  localDeleted?: boolean;
  /** Folder-only, bound to remoteId and scoped to this device and drive. */
  remoteFolderPreserved?: boolean;
}

export interface SyncInput {
  local: LocalEntry[];
  remote: RemoteEntry[];
  records: SyncRecord[];
  /** Text of the drive's `.marknotes/manifest.json`, if any. */
  manifest?: string | null;
  /** Omitted means the strict atomic-empty-only contract, never recursive deletion. */
  remoteFolderDeletion?: "atomicEmptyOnly" | "unsupported";
}

/**
 * One step of a sync plan. Run the actions in order; when one fails, record `"<path>: <message>"` and go on.
 * Notes keep a base copy when uploaded, downloaded or recorded with `updateBase`; attachments never do.
 */
export type Action =
  | { kind: "renameLocal"; path: string; toPath: string; isFolder: boolean; remote: RemoteEntry; sha256: string | null }
  | { kind: "renameRemote"; path: string; toPath: string; isFolder: boolean; remote: RemoteEntry; sha256: string | null }
  | { kind: "error"; path: string; message: string }
  | { kind: "recordFolder"; path: string; remoteId: string }
  | { kind: "createRemoteFolder"; path: string }
  | { kind: "createLocalFolder"; path: string; remoteId: string }
  | { kind: "forget"; path: string }
  | { kind: "uploadNew"; path: string }
  | { kind: "uploadRevision"; path: string; remote: RemoteEntry }
  | { kind: "download"; path: string; remote: RemoteEntry }
  | { kind: "record"; path: string; remote: RemoteEntry; sha256: string; updateBase: boolean }
  | { kind: "deleteLocal"; path: string }
  | { kind: "deleteRemote"; path: string; remote: RemoteEntry }
  | { kind: "deleteLocalFolder"; path: string; forget: string[] }
  | { kind: "deleteRemoteFolder"; path: string; remote: RemoteEntry; forget: string[] }
  | { kind: "preserveRemoteFolder"; path: string; remote: RemoteEntry }
  | { kind: "reconcile"; path: string; remote: RemoteEntry; hasRecord: boolean };

export interface SyncPlan {
  actions: Action[];
}

export type Side = "local" | "remote";

export interface ReconcileInput {
  path: string;
  local: string;
  localModifiedMillis: number;
  remote: string;
  remoteEntry: RemoteEntry;
  /** The base copy, or null when there is none. */
  base?: string | null;
  /** Every local path, used to pick a free name for the conflicted copy. */
  existingPaths: string[];
  /** A fresh note id for the conflicted copy. */
  newId: string;
  conflictCopyIntent?: ConflictCopyIntent | null;
  existingCopy?: ExistingCopy | null;
}

/** Persist before creating the copy; scoped to the current drive. */
export interface ConflictCopyIntent {
  path: string;
  localHash: string;
  remoteHash: string;
  remoteId: string;
  remoteVersion: string;
  localModifiedMillis: number;
  remoteModifiedMillis: number;
  baseHash: string | null;
  newId: string;
  copyPath: string;
  copyModifiedMillis: number;
}

export interface ExistingCopy {
  path: string;
  text: string;
  modifiedMillis: number;
}

export type ReconcileOutcome =
  | { kind: "same" }
  | { kind: "merged"; text: string }
  | { kind: "newestWins"; winner: Side; copyPath: string; copyText: string; copyModifiedMillis: number; conflictCopyIntent: ConflictCopyIntent; reuseCopy: boolean };

export type MergeChunk =
  | { kind: "resolved"; lines: string[]; changed: boolean }
  | { kind: "conflict"; base: string[]; local: string[]; remote: string[] };

export type ConflictChoice = "local" | "remote" | "both" | "neither";

export interface MergeResult {
  chunks: MergeChunk[];
  hasConflicts: boolean;
  conflicts: number;
  /** Merged text; unresolved conflicts are written with markers. */
  text: string;
}

export interface ManifestEntry {
  sha256: string;
  version: string;
  syncedAt: number;
}

export interface Manifest {
  deviceId: string;
  updatedAt: number;
  files: Record<string, ManifestEntry>;
}

export const MANIFEST_PATH: ".marknotes/manifest.json";
export const DEFAULT_MAX_PASSES: 3;

/** The ordered actions that bring one drive and the local tree in sync. */
export function plan(input: SyncInput): SyncPlan;
/** File records to flag durably on every linked drive before planning. */
export function localDeletions(local: LocalEntry[], records: SyncRecord[]): string[];
/** Retire these preserved folder records and active notices on ALL drives before planning; keep descendants. */
export function recreatedFolders(local: LocalEntry[], records: SyncRecord[]): string[];
/** Resolves a note that changed on both sides: same, merged, or newest wins with a conflicted copy. */
export function reconcile(input: ReconcileInput): ReconcileOutcome;
/** Line-based three-way merge; `base` is null when there is no common ancestor. */
export function merge3(base: string | null | undefined, local: string, remote: string): MergeResult;
/** Builds the merged text, resolving conflict `i` (0-based) with `choices[i]`. */
export function mergeText(chunks: MergeChunk[], choices?: Record<number, ConflictChoice> | null): string;
/** Parses a manifest; null when it is unreadable or uses a newer format. */
export function parseManifest(text: string): Manifest | null;
export function serializeManifest(manifest: Manifest): string;
/**
 * The manifest to write after a sync, or null when the files are unchanged from `previous`. Throws when
 * `nowMillis` is not a safe integer.
 */
export function buildManifest(
  records: SyncRecord[],
  deviceId: string,
  nowMillis: number,
  previous?: string | null,
): string | null;
export function sha256Hex(bytes: Uint8Array): string;
export function conflictCopyName(path: string, existingPaths: string[]): string;
/** Unicode simple case folding, identical to the planner's path comparison. */
export function foldCase(path: string): string;
export function isSyncablePath(path: string): boolean;
export function isSyncedEntry(path: string, isFolder: boolean): boolean;
export function isAttachmentPath(path: string): boolean;
export function readId(text: string): string | undefined;
export function withId(text: string, id: string): string;

/**
 * Order in which several drives are synced: call `next()` until it returns undefined, and `report()` each result.
 * A drive that brings local changes queues the others again, at most `maxPasses` times each (default 3).
 * The constructor and `report()` throw when `maxPasses` or `localChanges` is not a non-negative integer.
 */
export class SyncScheduler {
  constructor(driveIds: string[], maxPasses?: number | null);
  next(): string | undefined;
  report(driveId: string, localChanges: number, failed: boolean): void;
  free(): void;
}
