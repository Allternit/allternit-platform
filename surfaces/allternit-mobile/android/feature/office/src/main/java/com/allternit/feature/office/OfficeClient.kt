package com.allternit.feature.office

import com.allternit.core.api.ApiResult
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.arr
import com.allternit.feature.artifacts.bool
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.child
import com.allternit.feature.artifacts.list
import com.allternit.feature.artifacts.long
import com.allternit.feature.artifacts.num
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.str
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/** One section of an artifact-service document (iOS OfficeArtifactSection). */
data class OfficeSection(val id: String, val heading: String?, val kind: String, val body: String, val position: Int)

/** The office editor family a record belongs to (iOS OfficeFamily). */
enum class OfficeFamily(val raw: String, val displayName: String) {
    Docs("docs", "Document"), Sheets("sheets", "Spreadsheet"), Slides("slides", "Presentation"), Pdf("pdf", "PDF");

    /** Platform editor route for the interactive web view. */
    val editorPath: String get() = "/$raw"

    companion object {
        fun of(sectionKind: String): OfficeFamily? = when {
            sectionKind.startsWith("docs-editor/") -> Docs
            sectionKind.startsWith("sheets-editor/") -> Sheets
            sectionKind.startsWith("slides-editor/") -> Slides
            sectionKind.startsWith("pdf-viewer/") -> Pdf
            else -> null
        }
    }
}

/** The artifact-service document the office viewer renders read-only (iOS OfficeArtifact). */
data class OfficeArtifact(val id: String, val title: String, val sections: List<OfficeSection>) {
    val ordered: List<OfficeSection> get() = sections.sortedBy { it.position }
    val family: OfficeFamily? get() = sections.firstOrNull()?.let { OfficeFamily.of(it.kind) }
}

/** What a load can come back as. [computeRequired] is the 428 / compute_required state (no computer). */
sealed interface OfficeLoad {
    data class Ok(val artifact: OfficeArtifact) : OfficeLoad
    data object SignedOut : OfficeLoad
    data object ComputeRequired : OfficeLoad
    data class Failed(val message: String) : OfficeLoad
}

/** iOS OfficeArtifactClient: `GET /api/v1/artifacts/:id` (`{"artifact": {...}}`). */
object OfficeClient {
    suspend fun getArtifact(id: String): OfficeLoad = when (val r = LibraryNet.get("artifacts/${LibraryNet.escape(id)}")) {
        is ApiResult.Ok -> parse(r.value)?.let { OfficeLoad.Ok(it) } ?: OfficeLoad.Failed("This document couldn't be read.")
        is ApiResult.HttpError -> when {
            r.status == 401 -> OfficeLoad.SignedOut
            r.status == 428 || r.code == "compute_required" -> OfficeLoad.ComputeRequired
            else -> OfficeLoad.Failed(r.friendly("open this document"))
        }
        else -> OfficeLoad.Failed(r.friendly("open this document"))
    }

    private fun parse(e: JsonElement): OfficeArtifact? {
        val o: JsonObject = e.obj?.let { it["artifact"].obj ?: it } ?: return null
        val id = o.str("id") ?: return null
        val sections = o["sections"].arr.orEmpty().mapNotNull { it.obj }.mapIndexedNotNull { i, s ->
            OfficeSection(s.str("id") ?: "s$i", s.str("heading"), s.str("kind") ?: return@mapIndexedNotNull null, s.str("body").orEmpty(), s.num("position")?.toInt() ?: i)
        }
        return OfficeArtifact(id, o.str("title") ?: "Document", sections)
    }
}

/** A pod (project, team or chief group) in `GET /floor/state` v2. */
data class FloorPod(val id: String, val name: String, val color: String, val kind: String)

/** Per bot v2 fields: its pod, counts of what needs the person, and its last result (epoch ms). */
data class FloorBot(val pod: String?, val approval: Int, val question: Int, val suggestion: Int, val resultText: String?, val resultAt: Long)

/** A handoff between bots from `GET /floor/events`, with a short label. */
data class FloorHandoff(val id: String, val from: String, val to: String, val label: String, val at: Long)

/** `GET /api/v1/floor/state` (v:2), the Workspace floor's source of pods, needs and results. */
data class FloorState(val v: Int, val mainBotId: String?, val pods: List<FloorPod>, val bots: Map<String, FloorBot>, val handoffs: List<FloorHandoff> = emptyList())

/** What a floor call can come back as (step 27): the value, signed out (401/403), offline, or a message to show. */
sealed interface FloorCall<out T> {
    data class Ok<T>(val value: T) : FloorCall<T>
    data object SignedOut : FloorCall<Nothing>
    data object Offline : FloorCall<Nothing>
    data class Failed(val message: String) : FloorCall<Nothing>
}

internal val AUTOPILOT_SCOPES = listOf("all", "bot", "pod")
internal val AUTOPILOT_NEEDS = listOf("suggestion", "permission", "question")
internal val AUTOPILOT_LEVELS = listOf("draft", "ask", "tell", "limits")

/** The server's default level: questions start (and for now stay) at Draft only, the rest at Ask first. */
internal fun defaultAutopilotLevel(need: String) = if (need == "question") "draft" else "ask"

/**
 * One Autopilot rule (`/floor/autopilot` rules[]): which bots ([scopeType] all | bot | pod, with [scopeId]), which
 * need, and the level (draft | ask | tell | limits). [maxPerDay] and [allowedActions] apply at `limits` only.
 */
data class AutopilotRule(
    val id: String?,
    val scopeType: String,
    val scopeId: String?,
    val need: String,
    val level: String,
    val maxPerDay: Int? = null,
    val allowedActions: List<String> = emptyList(),
    val note: String? = null,
) {
    internal fun toJson(): JsonObject = buildJsonObject {
        id?.let { put("id", it) }
        put("scope", buildJsonObject { put("type", scopeType); if (scopeType != "all") scopeId?.let { put("id", it) } })
        put("need", need)
        // Questions are Draft only until the server can wake the main bot's session (FACTS step 5).
        put("level", if (need == "question") "draft" else level)
        if (level == "limits" && need != "question") put("limits", buildJsonObject {
            maxPerDay?.let { put("maxPerDay", it) }
            put("allowedActions", buildJsonArray { allowedActions.forEach { add(JsonPrimitive(it)) } })
        })
        note?.takeIf { it.isNotBlank() }?.let { put("note", it) }
    }
}

/** `GET|PUT /floor/autopilot`. */
data class AutopilotState(val enabled: Boolean, val mainBotId: String?, val rules: List<AutopilotRule>, val updatedAt: String? = null)

/** One "While you were away" line (`/floor/digest` items); answered items carry the rule that answered. */
data class FloorDigestItem(val botId: String, val threadId: String?, val text: String, val at: Long, val ruleId: String? = null)

/** `GET /floor/digest?since=`. */
data class FloorDigest(
    val since: Long,
    val until: Long,
    val answered: List<FloorDigestItem>,
    val waiting: List<FloorDigestItem>,
    val finished: List<FloorDigestItem>,
    val suggestions: List<FloorDigestItem>,
) {
    val isEmpty: Boolean get() = answered.isEmpty() && waiting.isEmpty() && finished.isEmpty() && suggestions.isEmpty()
}

/** `POST /floor/seen`: the last-seen time before this visit (null on a first visit) and now, epoch ms. */
data class FloorSeen(val previous: Long?, val now: Long)

/** Plain copy for a `PUT /floor/autopilot` 400 (`{error, rule}`). */
internal fun autopilotErrorMessage(code: String?, message: String?): String = when (code) {
    "duplicate_rule" -> "Two rules cover the same bots and the same kind of need. Remove one, then save."
    "duplicate_rule_id" -> "Two rules have the same id. Remove one, then save."
    "unknown_main_bot" -> "Your main bot isn't one of your active bots. Pick your main bot again."
    "too_many_rules" -> "Autopilot can keep up to 50 rules."
    "bad_scope", "scope_id_required", "bad_scope_id" -> "Pick a bot or a pod for each rule that isn't for all bots."
    "bad_need", "bad_level", "bad_rule_id" -> "One rule has a setting this app doesn't know. Update the app, then try again."
    "max_per_day_out_of_range" -> "Answers per day must be between 1 and 200."
    "too_many_actions" -> "A rule can allow up to 20 actions."
    "bad_action" -> "Each allowed action needs a name of 1 to 40 characters."
    "note_too_long" -> "Notes can be up to 200 characters."
    else -> message?.takeIf { it.isNotBlank() } ?: "Autopilot couldn't save these rules."
}

object FloorClient {
    private suspend fun getOk(path: String): JsonElement? = when (val r = LibraryNet.get(path)) { is ApiResult.Ok -> r.value; else -> null }

    /** Null on any failure (signed out, offline, old server); the floor then shows the v1 roster only. */
    suspend fun state(): FloorState? {
        val o = getOk("floor/state").obj ?: return null
        val v = o.num("v")?.toInt() ?: 1
        if (v < 2) return null
        val pods = o.list("pods").mapNotNull { p ->
            FloorPod(p.str("id") ?: return@mapNotNull null, p.str("name").orEmpty().take(40), p.str("color") ?: return@mapNotNull null, p.str("kind") ?: "project")
        }
        val bots = HashMap<String, FloorBot>()
        o.list("bots").forEach { b ->
            val id = b.str("id") ?: b.str("botId") ?: return@forEach
            val n = b.child("needs")
            val r = b.child("lastResult")
            bots[id] = FloorBot(
                b.str("pod"), n.num("approval")?.toInt() ?: 0, n.num("question")?.toInt() ?: 0, n.num("suggestion")?.toInt() ?: 0,
                r.str("text"), r.long("at") ?: 0L,
            )
        }
        return FloorState(v, o.str("mainBotId"), pods, bots, handoffs())
    }

    /** `GET /floor/autopilot` (step 27). */
    suspend fun autopilot(): FloorCall<AutopilotState> = when (val r = LibraryNet.get("floor/autopilot")) {
        is ApiResult.Ok -> r.value.obj?.let { FloorCall.Ok(parseAutopilot(it)) } ?: FloorCall.Failed("This app needs an update to read Autopilot.")
        else -> r.floorFailure("load Autopilot")
    }

    /** `PUT /floor/autopilot` with the whole rule set; a 400 comes back as a plain message for its error code. */
    suspend fun saveAutopilot(state: AutopilotState): FloorCall<AutopilotState> {
        val body = buildJsonObject {
            put("enabled", state.enabled)
            state.mainBotId?.let { put("mainBotId", it) }
            put("rules", buildJsonArray { state.rules.forEach { add(it.toJson()) } })
        }
        return when (val r = LibraryNet.send("PUT", "floor/autopilot", body)) {
            is ApiResult.Ok -> r.value.obj?.let { FloorCall.Ok(parseAutopilot(it)) } ?: FloorCall.Ok(state)
            is ApiResult.HttpError -> if (r.status == 400) FloorCall.Failed(autopilotErrorMessage(r.code, r.message)) else r.floorFailure("save Autopilot")
            else -> r.floorFailure("save Autopilot")
        }
    }

    /**
     * `POST /floor/seen`: stores now as the last-seen time and returns the one before it. Null on any failure (signed
     * out, offline, old server); the digest is then skipped.
     */
    suspend fun seen(): FloorSeen? {
        val o = (LibraryNet.send("POST", "floor/seen") as? ApiResult.Ok)?.value?.obj ?: return null
        val now = o.long("now") ?: return null
        return FloorSeen(o.long("previous"), now)
    }

    /**
     * `GET /floor/digest` (epoch ms). With no [since] the server uses the stored last-seen time, else the last 24 h.
     * Null on any failure.
     */
    suspend fun digest(since: Long? = null): FloorDigest? {
        val o = (LibraryNet.get(if (since != null) "floor/digest?since=$since" else "floor/digest") as? ApiResult.Ok)?.value?.obj ?: return null
        fun items(k: String) = o.list(k).mapNotNull { e ->
            val bot = e.str("botId") ?: return@mapNotNull null
            val text = e.str("text")?.trim()?.takeIf { it.isNotEmpty() } ?: return@mapNotNull null
            FloorDigestItem(bot, e.str("threadId"), text, e.long("at") ?: 0L, e.str("ruleId"))
        }
        return FloorDigest(o.long("since") ?: since ?: 0L, o.long("until") ?: 0L, items("answered"), items("waiting"), items("finished"), items("suggestions"))
    }

    private fun parseAutopilot(o: JsonObject): AutopilotState = AutopilotState(
        enabled = o.bool("enabled") == true,
        mainBotId = o.str("mainBotId"),
        rules = o.list("rules").mapNotNull { r ->
            val scope = r.child("scope")
            val need = r.str("need")?.takeIf { it in AUTOPILOT_NEEDS } ?: return@mapNotNull null
            val limits = r.child("limits")
            AutopilotRule(
                id = r.str("id"),
                scopeType = scope.str("type")?.takeIf { it in AUTOPILOT_SCOPES } ?: "all",
                scopeId = scope.str("id"),
                need = need,
                level = r.str("level")?.takeIf { it in AUTOPILOT_LEVELS } ?: defaultAutopilotLevel(need),
                maxPerDay = limits.num("maxPerDay")?.toInt(),
                allowedActions = (limits?.get("allowedActions") as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.takeIf { p -> p.isString }?.content }.orEmpty(),
                note = r.str("note"),
            )
        },
        updatedAt = o.str("updatedAt"),
    )

    private fun ApiResult<*>.floorFailure(doing: String): FloorCall<Nothing> = when {
        this is ApiResult.HttpError && (status == 401 || status == 403) -> FloorCall.SignedOut
        this is ApiResult.Network -> FloorCall.Offline
        else -> FloorCall.Failed("Couldn't $doing. ${friendly(doing)}")
    }

    /** Recent handoffs (last 3 minutes); empty when events can't be read. */
    private suspend fun handoffs(): List<FloorHandoff> {
        val o = getOk("floor/events") ?: return emptyList()
        val cutoff = System.currentTimeMillis() - 3 * 60_000L
        val items = o.obj?.list("events") ?: o.arr.orEmpty().mapNotNull { it.obj }
        return items.mapNotNull { e ->
            if (e.str("kind") != "handoff") return@mapNotNull null
            val at = e.long("at") ?: 0L
            val from = e.str("botId") ?: e.str("from") ?: return@mapNotNull null
            val to = e.str("toBotId") ?: e.str("to") ?: return@mapNotNull null
            if (at < cutoff) return@mapNotNull null
            FloorHandoff(e.str("id") ?: "$from-$to-$at", from, to, (e.str("label") ?: e.str("text")).orEmpty().take(60), at)
        }
    }
}
