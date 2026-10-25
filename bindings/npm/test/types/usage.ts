// Compiled with `npm run typecheck` (tsc --noEmit, strict): the declarations in index.d.ts must type real usage,
// and reject what the runtime rejects. Not run.
import {
  type Action, type ConflictChoice, type ReconcileOutcome, type SyncInput, DEFAULT_MAX_PASSES, MANIFEST_PATH,
  SyncScheduler, buildManifest, conflictCopyName, isAttachmentPath, isSyncablePath, isSyncedEntry, merge3, mergeText,
  foldCase, localDeletions, recreatedFolders, parseManifest, plan, readId, reconcile, serializeManifest, sha256Hex, withId,
} from "@marknotes/sync";

const input: SyncInput = {
  local: [{ path: "a.md", isFolder: false, modifiedMillis: 1, sha256: sha256Hex(new Uint8Array([1])) }],
  remote: [{ path: "a.md", id: "r", isFolder: false, version: "v", modifiedMillis: 2 }],
  records: [],
  manifest: null,
  remoteFolderDeletion: "unsupported",
};

// Every action kind is handled; adding one to the union without handling it here fails to compile.
function describe(action: Action): string {
  switch (action.kind) {
    case "error": return action.message;
    case "renameLocal": case "renameRemote": return `${action.path}:${action.toPath}:${action.sha256 ?? ""}`;
    case "recordFolder": case "createLocalFolder": return action.remoteId;
    case "createRemoteFolder": case "forget": case "uploadNew": case "deleteLocal": return action.path;
    case "uploadRevision": case "download": case "deleteRemote": return action.remote.version;
    case "record": return `${action.sha256}${action.updateBase}`;
    case "deleteLocalFolder": return action.forget.join();
    case "deleteRemoteFolder": return action.forget.join() + action.remote.id;
    case "preserveRemoteFolder": return action.remote.id;
    case "reconcile": return `${action.hasRecord}`;
    default: { const unreachable: never = action; return unreachable; }
  }
}
plan(input).actions.map(describe);
const deleted: string[] = localDeletions(input.local, input.records);
const recreated: string[] = recreatedFolders(input.local, input.records);
const folded: string = foldCase("Note.md");
void [deleted, recreated, folded];

const outcome: ReconcileOutcome = reconcile({
  path: "a.md", local: "x", localModifiedMillis: 1, remote: "y", remoteEntry: input.remote[0],
  base: null, existingPaths: ["a.md"], newId: "id",
});
if (outcome.kind === "newestWins") {
  const side: "local" | "remote" = outcome.winner;
  const modified: number = outcome.copyModifiedMillis;
  const replay = reconcile({
    path: "a.md", local: "x", localModifiedMillis: 1, remote: "y", remoteEntry: input.remote[0],
    existingPaths: [outcome.copyPath], newId: "m4n5p6q7", conflictCopyIntent: outcome.conflictCopyIntent,
    existingCopy: { path: outcome.copyPath, text: outcome.copyText, modifiedMillis: modified },
  });
  void [side, replay, outcome.reuseCopy];
}
if (outcome.kind === "merged") { const text: string = outcome.text; void text; }

const merged = merge3(undefined, "a", "b");
const choice: ConflictChoice = "both";
mergeText(merged.chunks, { 0: choice });
mergeText(merged.chunks);
const manifest = parseManifest("{}");
if (manifest !== null) serializeManifest(manifest);
const next: string | null = buildManifest([], "device", Date.now(), null);
const copy: string = conflictCopyName("a.md", []);
const flags: boolean[] = [isSyncablePath("a.md"), isSyncedEntry("a", true), isAttachmentPath("a.png")];
const id: string | undefined = readId(withId("text", "id"));
const path: ".marknotes/manifest.json" = MANIFEST_PATH;
const passes: 3 = DEFAULT_MAX_PASSES;

const scheduler = new SyncScheduler(["a", "b"]);
for (let drive = scheduler.next(); drive !== undefined; drive = scheduler.next()) scheduler.report(drive, 0, false);
scheduler.free();
void [next, copy, flags, id, path, passes];

// What the runtime rejects is rejected at compile time too.
// @ts-expect-error a file needs isFolder
plan({ local: [{ path: "a.md" }], remote: [], records: [] });
// @ts-expect-error times are numbers, not bigints
buildManifest([], "device", 1n);
// @ts-expect-error no such conflict choice
mergeText(merged.chunks, { 0: "mine" });
// @ts-expect-error sha256Hex takes bytes, not text
sha256Hex("text");
// @ts-expect-error no recursive deletion capability exists
plan({ ...input, remoteFolderDeletion: "recursive" });
// @ts-expect-error markers are booleans
input.records.push({ path: "F", isFolder: true, remoteId: "r", remoteVersion: "", localHash: "", syncedAt: 0, remoteFolderPreserved: "true" });
