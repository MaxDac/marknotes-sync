package io.github.maxdac.marknotes.sync

import java.io.File
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

/**
 * Runs the marknotes-spec sync fixtures through the Kotlin bindings (JNA + the host library), checking the
 * generated types and the FFI layer as well as the Rust code.
 */
class FixturesTest {
    private val specDir = File(System.getProperty("marknotes.specDir"))

    private fun suite(file: String, run: (JsonObject) -> JsonElement) {
        val body = Json.parseToJsonElement(File(specDir, "fixtures/$file").readText()).jsonObject
        for (case in body.getValue("cases").jsonArray) {
            val c = case.jsonObject
            assertEquals(c.getValue("expected"), run(c.getValue("input").jsonObject), "$file / ${c["name"]}")
        }
    }

    private fun JsonObject.str(key: String) = getValue(key).jsonPrimitive.content
    private fun JsonObject.strOrNull(key: String) = (get(key) as? JsonPrimitive)?.takeIf { it.isString }?.content
    private fun JsonObject.long(key: String) = get(key)?.jsonPrimitive?.long ?: 0L
    private fun JsonObject.bool(key: String) = getValue(key).jsonPrimitive.boolean
    private fun JsonObject.strings(key: String) = get(key)?.jsonArray?.map { it.jsonPrimitive.content }.orEmpty()
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

    @Test
    fun plan() = suite("sync/plan.json") { i ->
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
    }

    @Test
    fun reconcile() = suite("sync/reconcile.json") { i ->
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
    }

    @Test
    fun multiDrive() = suite("sync/multi-drive.json") { i ->
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
    }

    @Test
    fun manifestParse() = suite("manifest/parse.json") { i ->
        val m = parseManifest(i.str("json"))
        val manifest = if (m == null) {
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
        buildJsonObject { put("manifest", manifest) }
    }

    @Test
    fun manifestBuild() = suite("manifest/build.json") { i ->
        val json = buildManifest(
            i.getValue("records").jsonArray.map { record(it.jsonObject) }, i.str("deviceId"), i.long("nowMillis"),
            i.strOrNull("previous"),
        )
        buildJsonObject { put("json", s(json)) }
    }

    @Test
    fun merge3() = suite("merge/merge3.json") { i ->
        val result = merge3(i.strOrNull("base"), i.str("local"), i.str("remote"))
        val choices = (i["choices"] as? JsonObject)?.entries?.associate { (k, v) ->
            k.toInt() to ConflictChoice.valueOf(v.jsonPrimitive.content.uppercase())
        }
        val text = if (choices == null) result.text else mergeText(result.chunks, choices)
        buildJsonObject {
            put("text", text)
            put("conflicts", result.conflicts)
        }
    }

    @Test
    fun conflictCopyName() = suite("merge/conflict-copy-name.json") { i ->
        buildJsonObject { put("path", conflictCopyName(i.str("path"), i.strings("existingPaths"))) }
    }

    @Test
    fun withId() = suite("front-matter/with-id.json") { i ->
        buildJsonObject { put("text", withId(i.str("text"), i.str("id"))) }
    }

    @Test
    fun readId() = suite("front-matter/read-id.json") { i ->
        buildJsonObject { put("id", s(readId(i.str("text")))) }
    }

    @Test
    fun manifestSerialize() = suite("manifest/serialize.json") { i ->
        val files = i.getValue("files").jsonObject.mapValues { (_, e) ->
            val o = e.jsonObject
            ManifestEntry(o.str("sha256"), o.str("version"), o.long("syncedAt"))
        }
        buildJsonObject { put("json", serializeManifest(Manifest(i.str("deviceId"), i.long("updatedAt"), files))) }
    }

    @Test
    fun isAttachmentPath() = suite("attachments/is-attachment-path.json") { i ->
        buildJsonObject { put("isAttachment", isAttachmentPath(i.str("path"))) }
    }

    /** Like the Rust runner: every fixture file of the folders the core owns must have a suite here. */
    @Test
    fun everyOwnedFixtureFileIsMapped() {
        val mapped = setOf(
            "sync/plan.json", "sync/reconcile.json", "sync/multi-drive.json", "manifest/parse.json",
            "manifest/serialize.json", "manifest/build.json", "merge/merge3.json", "merge/conflict-copy-name.json",
        )
        for (folder in listOf("sync", "manifest", "merge")) {
            for (f in File(specDir, "fixtures/$folder").listFiles { f -> f.name.endsWith(".json") }!!) {
                assert("$folder/${f.name}" in mapped) { "$folder/${f.name} has no suite" }
            }
        }
    }

    @Test
    fun sha256() {
        assertEquals(
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            sha256Hex("abc".toByteArray()),
        )
    }
}
