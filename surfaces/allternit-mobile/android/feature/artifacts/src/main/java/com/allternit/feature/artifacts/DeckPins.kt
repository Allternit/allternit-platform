package com.allternit.feature.artifacts

import android.content.Context
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.allternit.core.api.ApiResult
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/** One artifact pinned to the composer deck's Artifacts menu (web DeckPin: {id, kind, title}). */
data class DeckPin(val id: String, val kind: String, val title: String)

/**
 * Pinned deck artifacts (Artifacts redesign 6.4; web `src/lib/artifacts/deck-pins.ts`). Kept per user on the
 * runtime (GET / PUT `deck-pins`, newest first, max [MAX]). An older runtime without the route (404), a
 * signed-out user or a refused call (401/403), or no network falls back to this phone's local copy
 * (SharedPreferences). Plans are never pinned. The card menu (6.5) calls [pin] / [unpin].
 */
object DeckPins {
    const val MAX = 8
    private const val PREFS = "allternit_shell"
    private const val KEY = "deck-pins"
    /** The pre-6.4 local list: artifact ids only. Read once as pins so nothing a person pinned is lost. */
    private const val LEGACY_KEY = "deck-pinned-artifacts"

    private enum class Mode { Runtime, Local }

    /** The current pins; Compose reads it. */
    var pins by mutableStateOf<List<DeckPin>>(emptyList()); private set
    private var mode = Mode.Local

    fun isPinned(id: String): Boolean = pins.any { it.id == id }

    fun normalize(raw: List<DeckPin>): List<DeckPin> {
        val seen = HashSet<String>()
        val out = ArrayList<DeckPin>()
        for (p in raw) {
            if (p.id.isBlank() || p.kind == "plan" || !seen.add(p.id)) continue
            out += DeckPin(p.id, p.kind.ifBlank { "doc" }, p.title.ifBlank { "Untitled" })
            if (out.size >= MAX) break
        }
        return out
    }

    /** Pins (or re-pins) to the front; the oldest drops off past the cap. */
    fun withPin(list: List<DeckPin>, pin: DeckPin): List<DeckPin> =
        if (pin.kind == "plan") list else normalize(listOf(pin) + list.filter { it.id != pin.id })

    /** Loads the pins: the runtime's copy when it keeps one, else this phone's. */
    suspend fun load(context: Context) {
        when (val r = LibraryNet.get("deck-pins")) {
            is ApiResult.Ok -> { mode = Mode.Runtime; pins = normalize(parse(r.value.pinsArray())) }
            else -> { mode = Mode.Local; pins = readLocal(context) }
        }
    }

    suspend fun pin(context: Context, pin: DeckPin): Result<Unit> = save(context, withPin(pins, pin))

    suspend fun unpin(context: Context, id: String): Result<Unit> = save(context, pins.filter { it.id != id })

    suspend fun toggle(context: Context, pin: DeckPin): Result<Unit> = if (isPinned(pin.id)) unpin(context, pin.id) else pin(context, pin)

    private suspend fun save(context: Context, next: List<DeckPin>): Result<Unit> {
        val prev = pins
        pins = next
        if (mode == Mode.Local) { writeLocal(context, next); return Result.success(Unit) }
        val body = buildJsonObject { put("pins", buildJsonArray { next.forEach { add(toJson(it)) } }) }
        return when (val r = LibraryNet.send("PUT", "deck-pins", body)) {
            is ApiResult.Ok -> {
                (r.value as? JsonObject)?.get("pins")?.let { pins = normalize(parse(r.value.pinsArray())) }
                Result.success(Unit)
            }
            is ApiResult.HttpError -> if (r.status == 404 || r.status == 401 || r.status == 403) {
                mode = Mode.Local; writeLocal(context, next); Result.success(Unit)
            } else { pins = prev; Result.failure(IllegalStateException("Pins could not be saved. Try again.")) }
            else -> { pins = prev; Result.failure(IllegalStateException("Pins could not be saved. Try again.")) }
        }
    }

    private fun toJson(p: DeckPin): JsonObject = buildJsonObject { put("id", p.id); put("kind", p.kind); put("title", p.title) }

    private fun JsonElement.pinsArray(): JsonArray? = (this as? JsonObject)?.get("pins") as? JsonArray ?: this as? JsonArray

    private fun parse(arr: JsonArray?): List<DeckPin> = arr.orEmpty().mapNotNull { e ->
        val o = e as? JsonObject ?: return@mapNotNull null
        val id = (o["id"] as? JsonPrimitive)?.takeIf { it.isString }?.content ?: return@mapNotNull null
        DeckPin(id, (o["kind"] as? JsonPrimitive)?.content.orEmpty(), (o["title"] as? JsonPrimitive)?.content.orEmpty())
    }

    private fun readLocal(context: Context): List<DeckPin> {
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val raw = prefs.getString(KEY, null)
        if (raw != null) {
            return runCatching { normalize(parse(kotlinx.serialization.json.Json.parseToJsonElement(raw) as? JsonArray)) }.getOrDefault(emptyList())
        }
        return normalize(prefs.getString(LEGACY_KEY, "").orEmpty().split(',').filter { it.isNotEmpty() }.map { DeckPin(it, "doc", "Untitled") })
    }

    private fun writeLocal(context: Context, list: List<DeckPin>) {
        val arr = buildJsonArray { list.forEach { add(toJson(it)) } }
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putString(KEY, arr.toString()).remove(LEGACY_KEY).apply()
    }
}
