package io.github.maxdac.marknotes.sync

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.boolean
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.long
import kotlinx.serialization.json.put
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue
import kotlin.test.fail

/**
 * Parity corpus: the Rust core generated seeded, randomized inputs and recorded its own outputs; this test replays
 * every case through the Kotlin bindings (UniFFI + JNA + the native library) and requires exactly the same outputs,
 * so the FFI layer (serialization, numbers, strings with non-BMP characters, byte order marks and control characters,
 * null) neither adds to nor loses anything from what the core computes. The npm package replays the same corpus
 * (bindings/npm/test/parity.test.mjs).
 *
 * The corpus is read as `parity/<op>.json` through [FixtureSource]: `target/parity/` at the repository root on the
 * JVM, test assets on Android. Generate it with
 * `cargo test -p marknotes-sync --test parity_corpus --locked -- --ignored` (CI does before running the tests); see
 * crates/marknotes-sync/tests/parity_corpus.rs.
 */
class ParityTest {
    private fun read(file: String): JsonObject {
        val path = "parity/$file"
        assertTrue(
            FixtureSource.exists(path),
            "parity corpus missing: $path; generate it with " +
                "`cargo test -p marknotes-sync --test parity_corpus --locked -- --ignored` at the repository root",
        )
        return Json.parseToJsonElement(FixtureSource.read(path)).jsonObject
    }

    private fun parity(op: String, run: (JsonObject) -> JsonElement) {
        val cases = read("$op.json").getValue("cases").jsonArray
        assertTrue(cases.isNotEmpty(), "parity/$op.json has no cases")
        cases.forEachIndexed { index, case ->
            val c = case.jsonObject
            val what = "parity/$op.json case $index (${c["name"]?.jsonPrimitive?.content})"
            val actual = try {
                run(c.getValue("input").jsonObject)
            } catch (e: Exception) {
                throw AssertionError("$what threw $e\n  input ${c["input"]}", e)
            }
            assertEquals(c.getValue("expected"), actual, what)
        }
    }

    private fun JsonObject.str(key: String) = getValue(key).jsonPrimitive.content
    private fun JsonObject.strOrNull(key: String) = (get(key) as? JsonPrimitive)?.takeIf { it.isString }?.content
    private fun JsonObject.long(key: String) = getValue(key).jsonPrimitive.long
    private fun JsonObject.bool(key: String) = getValue(key).jsonPrimitive.boolean
    private fun JsonObject.strings(key: String) = getValue(key).jsonArray.map { it.jsonPrimitive.content }
    private fun s(v: String?): JsonElement = if (v == null) JsonNull else JsonPrimitive(v)
    private fun strings(v: List<String>) = JsonArray(v.map(::JsonPrimitive))

    private fun remote(o: JsonObject) =
        RemoteEntry(o.str("path"), o.str("id"), o.bool("isFolder"), o.str("version"), o.long("modifiedMillis"))

    private fun record(o: JsonObject) = SyncRecord(
        o.str("path"), o.bool("isFolder"), o.str("remoteId"), o.str("remoteVersion"), o.str("localHash"),
        o.long("syncedAt"),
    )

    private fun json(r: RemoteEntry) = buildJsonObject {
        put("path", r.path)
        put("id", r.id)
        put("isFolder", r.isFolder)
        put("version", r.version)
        put("modifiedMillis", r.modifiedMillis)
    }

    private fun json(a: Action): JsonObject = buildJsonObject {
        when (a) {
            is Action.Error -> {
                put("kind", "error"); put("path", a.path); put("message", a.message)
            }
            is Action.RecordFolder -> {
                put("kind", "recordFolder"); put("path", a.path); put("remoteId", a.remoteId)
            }
            is Action.CreateRemoteFolder -> {
                put("kind", "createRemoteFolder"); put("path", a.path)
            }
            is Action.CreateLocalFolder -> {
                put("kind", "createLocalFolder"); put("path", a.path); put("remoteId", a.remoteId)
            }
            is Action.Forget -> {
                put("kind", "forget"); put("path", a.path)
            }
            is Action.UploadNew -> {
                put("kind", "uploadNew"); put("path", a.path)
            }
            is Action.UploadRevision -> {
                put("kind", "uploadRevision"); put("path", a.path); put("remote", json(a.remote))
            }
            is Action.Download -> {
                put("kind", "download"); put("path", a.path); put("remote", json(a.remote))
            }
            is Action.Record -> {
                put("kind", "record"); put("path", a.path); put("remote", json(a.remote))
                put("sha256", a.sha256); put("updateBase", a.updateBase)
            }
            is Action.DeleteLocal -> {
                put("kind", "deleteLocal"); put("path", a.path)
            }
            is Action.DeleteRemote -> {
                put("kind", "deleteRemote"); put("path", a.path); put("remote", json(a.remote))
            }
            is Action.DeleteLocalFolder -> {
                put("kind", "deleteLocalFolder"); put("path", a.path); put("forget", strings(a.forget))
            }
            is Action.DeleteRemoteFolder -> {
                put("kind", "deleteRemoteFolder"); put("path", a.path); put("remote", json(a.remote))
                put("forget", strings(a.forget))
            }
            is Action.Reconcile -> {
                put("kind", "reconcile"); put("path", a.path); put("remote", json(a.remote))
                put("hasRecord", a.hasRecord)
            }
        }
    }

    private fun json(c: MergeChunk): JsonObject = buildJsonObject {
        when (c) {
            is MergeChunk.Resolved -> {
                put("kind", "resolved"); put("lines", strings(c.lines)); put("changed", c.changed)
            }
            is MergeChunk.Conflict -> {
                put("kind", "conflict"); put("base", strings(c.base)); put("local", strings(c.local))
                put("remote", strings(c.remote))
            }
        }
    }

    private fun json(m: Manifest?): JsonElement = if (m == null) {
        JsonNull
    } else {
        buildJsonObject {
            put("deviceId", m.deviceId)
            put("updatedAt", m.updatedAt)
            val files = m.files.mapValues { (_, e) ->
                buildJsonObject {
                    put("sha256", e.sha256)
                    put("version", e.version)
                    put("syncedAt", e.syncedAt)
                }
            }
            put("files", JsonObject(files))
        }
    }

    private val runners: Map<String, (JsonObject) -> JsonElement> = mapOf(
        "plan" to { i ->
            val input = SyncInput(
                local = i.getValue("local").jsonArray.map {
                    val o = it.jsonObject
                    LocalEntry(o.str("path"), o.bool("isFolder"), o.long("modifiedMillis"), o.strOrNull("sha256"))
                },
                remote = i.getValue("remote").jsonArray.map { remote(it.jsonObject) },
                records = i.getValue("records").jsonArray.map { record(it.jsonObject) },
                manifest = i.strOrNull("manifest"),
            )
            buildJsonObject { put("actions", JsonArray(plan(input).actions.map { json(it) })) }
        },
        "reconcile" to { i ->
            val outcome = reconcile(
                ReconcileInput(
                    i.str("path"), i.str("local"), i.long("localModifiedMillis"), i.str("remote"),
                    remote(i.getValue("remoteEntry").jsonObject), i.strOrNull("base"), i.strings("existingPaths"),
                    i.str("newId"),
                ),
            )
            when (outcome) {
                ReconcileOutcome.Same -> buildJsonObject { put("kind", "same") }
                is ReconcileOutcome.Merged -> buildJsonObject {
                    put("kind", "merged")
                    put("text", outcome.text)
                }
                is ReconcileOutcome.NewestWins -> buildJsonObject {
                    put("kind", "newestWins")
                    put("winner", if (outcome.winner == Side.LOCAL) "local" else "remote")
                    put("copyPath", outcome.copyPath)
                    put("copyText", outcome.copyText)
                }
            }
        },
        "merge3" to { i ->
            val result = merge3(i.strOrNull("base"), i.str("local"), i.str("remote"))
            val choices = (i["choices"] as? JsonObject)?.entries?.associate { (k, v) ->
                k.toInt() to ConflictChoice.valueOf(v.jsonPrimitive.content.uppercase())
            }
            val text = if (choices == null) result.text else mergeText(result.chunks, choices)
            buildJsonObject {
                put("text", text)
                put("conflicts", result.conflicts)
                put("hasConflicts", result.hasConflicts)
                put("chunks", JsonArray(result.chunks.map { json(it) }))
            }
        },
        "parseManifest" to { i -> buildJsonObject { put("manifest", json(parseManifest(i.str("json")))) } },
        "serializeManifest" to { i ->
            val files = i.getValue("files").jsonObject.mapValues { (_, e) ->
                val o = e.jsonObject
                ManifestEntry(o.str("sha256"), o.str("version"), o.long("syncedAt"))
            }
            buildJsonObject { put("json", serializeManifest(Manifest(i.str("deviceId"), i.long("updatedAt"), files))) }
        },
        "buildManifest" to { i ->
            val json = buildManifest(
                i.getValue("records").jsonArray.map { record(it.jsonObject) }, i.str("deviceId"), i.long("nowMillis"),
                i.strOrNull("previous"),
            )
            buildJsonObject { put("json", s(json)) }
        },
        "conflictCopyName" to { i ->
            buildJsonObject { put("path", conflictCopyName(i.str("path"), i.strings("existingPaths"))) }
        },
        "isSyncablePath" to { i -> buildJsonObject { put("syncable", isSyncablePath(i.str("path"))) } },
        "isSyncedEntry" to { i -> buildJsonObject { put("synced", isSyncedEntry(i.str("path"), i.bool("isFolder"))) } },
        "isAttachmentPath" to { i -> buildJsonObject { put("isAttachment", isAttachmentPath(i.str("path"))) } },
        "readId" to { i -> buildJsonObject { put("id", s(readId(i.str("text")))) } },
        "withId" to { i -> buildJsonObject { put("text", withId(i.str("text"), i.str("id"))) } },
        "sha256Hex" to { i ->
            val bytes = i.getValue("bytes").jsonArray.map { it.jsonPrimitive.int.toByte() }.toByteArray()
            buildJsonObject { put("sha256", sha256Hex(bytes)) }
        },
        "syncScheduler" to { i ->
            val reports = i.getValue("reports").jsonArray
            val order = mutableListOf<String>()
            SyncScheduler(i.strings("driveIds"), i.getValue("maxPasses").jsonPrimitive.int).use { scheduler ->
                while (true) {
                    val id = scheduler.next() ?: break
                    val r = reports[order.size].jsonObject
                    scheduler.report(id, r.getValue("localChanges").jsonPrimitive.int, r.bool("failed"))
                    order += id
                }
            }
            buildJsonObject { put("order", strings(order)) }
        },
    )

    private fun run(op: String) = parity(op, runners[op] ?: fail("no runner for $op"))

    @Test fun plan() = run("plan")

    @Test fun reconcile() = run("reconcile")

    @Test fun merge3() = run("merge3")

    @Test fun parseManifest() = run("parseManifest")

    @Test fun serializeManifest() = run("serializeManifest")

    @Test fun buildManifest() = run("buildManifest")

    @Test fun conflictCopyName() = run("conflictCopyName")

    @Test fun isSyncablePath() = run("isSyncablePath")

    @Test fun isSyncedEntry() = run("isSyncedEntry")

    @Test fun isAttachmentPath() = run("isAttachmentPath")

    @Test fun readId() = run("readId")

    @Test fun withId() = run("withId")

    @Test fun sha256Hex() = run("sha256Hex")

    @Test fun syncScheduler() = run("syncScheduler")

    /** Every operation of the corpus has a runner here, and every runner has its file in the corpus. */
    @Test
    fun everyOperationOfTheCorpusHasARunner() {
        val ops = read("index.json").getValue("ops").jsonObject.keys
        assertEquals(runners.keys.sorted(), ops.sorted(), "operations of parity/index.json")
    }
}
