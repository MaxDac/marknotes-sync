package io.github.maxdac.marknotes.sync

import kotlinx.serialization.json.*

internal fun parseCopyIntent(value: JsonElement?): ConflictCopyIntent? {
    val o = value as? JsonObject ?: return null
    fun str(key: String) = o.getValue(key).jsonPrimitive.content
    fun time(key: String) = o.getValue(key).jsonPrimitive.long
    return ConflictCopyIntent(
        str("path"), str("localHash"), str("remoteHash"), str("remoteId"), str("remoteVersion"),
        time("localModifiedMillis"), time("remoteModifiedMillis"),
        (o["baseHash"] as? JsonPrimitive)?.contentOrNull, str("newId"), str("copyPath"), time("copyModifiedMillis"),
    )
}

internal fun parseExistingCopy(value: JsonElement?): ExistingCopy? {
    val o = value as? JsonObject ?: return null
    return ExistingCopy(
        o.getValue("path").jsonPrimitive.content, o.getValue("text").jsonPrimitive.content,
        o.getValue("modifiedMillis").jsonPrimitive.long,
    )
}

internal fun copyIntentJson(i: ConflictCopyIntent) = buildJsonObject {
    put("path", i.path); put("localHash", i.localHash); put("remoteHash", i.remoteHash)
    put("remoteId", i.remoteId); put("remoteVersion", i.remoteVersion)
    put("localModifiedMillis", i.localModifiedMillis); put("remoteModifiedMillis", i.remoteModifiedMillis)
    put("baseHash", i.baseHash?.let(::JsonPrimitive) ?: JsonNull)
    put("newId", i.newId); put("copyPath", i.copyPath); put("copyModifiedMillis", i.copyModifiedMillis)
}
