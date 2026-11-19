package com.allternit.feature.shell

import android.content.Context
import androidx.annotation.DrawableRes
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.graphics.Color
import com.allternit.core.api.ApiCall
import com.allternit.core.api.ApiClients
import com.allternit.core.api.ApiJson
import com.allternit.core.api.ApiConfig
import com.allternit.core.api.ApiResult
import com.allternit.core.api.gen.AgentSession
import com.allternit.core.api.gen.AllternitApi
import com.allternit.core.auth.AllternitAuth
import com.allternit.core.design.FriendlyErrorMessage
import com.allternit.core.design.ModeAccents
import com.allternit.core.design.R as DesignR
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.contentOrNull
import java.time.OffsetDateTime

/**
 * The four phone modes (iOS `ShellMode`, Core/AppMode.swift). [raw] is the wire/test id:
 * `modeFanCard<raw>` and the persisted value. Cowork is not a mode (a composer toggle in Home).
 */
internal enum class ShellMode(val raw: String, val label: String, @DrawableRes val icon: Int, val accent: Color) {
    Home("home", "Home", R.drawable.shell_house, ModeAccents.Chat.accent),
    Bots("bots", "Bots", DesignR.drawable.ph_cpu, ModeAccents.Cowork.accent),
    Code("code", "Code", DesignR.drawable.ph_terminal, ModeAccents.Code.accent),
    Aci("aci", "ACI", DesignR.drawable.ph_globe, ModeAccents.Browser.accent);

    /** PhoneDrawer WORDMARK_SUFFIX beside the pixel A://TERNIT (iOS ShellWordmark); Code draws GIZZI CODE. */
    val wordmarkSuffix: String
        get() = when (this) {
            Home, Code -> ""
            Bots -> "BOTS"
            Aci -> "ACI"
        }

    /** The drawer's New pill (HistorySidebarView NewOption title). */
    val newLabel: String
        get() = when (this) {
            Home -> "New chat"
            Bots, Code -> "New thread"
            Aci -> "New session"
        }

    val emptyTitle: String
        get() = when (this) {
            Home -> "No chats yet"
            Bots -> "No threads yet"
            Code -> "No code threads yet"
            Aci -> "No runs yet"
        }

    val emptyMessage: String
        get() = when (this) {
            Home -> "Your chats show up here."
            Bots -> "Your bot threads show up here."
            Code -> "Your code threads show up here."
            Aci -> "Your agent runs show up here."
        }

    companion object {
        /** MenuHold.fanModes order: Home · Bots · Code · ACI. */
        val fan: List<ShellMode> = entries

        fun fromRaw(raw: String?): ShellMode = entries.firstOrNull { it.raw == raw } ?: Home
    }
}

/** Remembers the last mode, like iOS AppModeStore (UserDefaults). */
internal object ShellModePrefs {
    private const val FILE = "allternit_shell"
    private const val KEY = "shell-mode"

    fun load(context: Context): ShellMode =
        ShellMode.fromRaw(context.getSharedPreferences(FILE, Context.MODE_PRIVATE).getString(KEY, null))

    fun save(context: Context, mode: ShellMode) {
        context.getSharedPreferences(FILE, Context.MODE_PRIVATE).edit().putString(KEY, mode.raw).apply()
    }
}

/**
 * A drawer rail row (HistorySidebarView `railRows`). [id] keeps the web's rail-tab ids, so the
 * test id is `drawerRow<id>` (maestro/ids.md).
 */
internal data class RailRow(val id: String, val label: String, @DrawableRes val icon: Int)

internal fun railRows(mode: ShellMode): List<RailRow> {
    val projects = RailRow("projects", "Projects", R.drawable.shell_folder)
    val artifacts = RailRow("artifacts-library", "Artifacts", R.drawable.shell_file_text)
    return when (mode) {
        ShellMode.Home -> listOf(
            projects, artifacts,
            RailRow("automation-tasks", "Automation Tasks", R.drawable.shell_clock),
            RailRow("customize", "Customize", DesignR.drawable.ph_sliders_horizontal),
        )
        ShellMode.Code -> listOf(
            projects, artifacts,
            RailRow("code-automations", "Automation Tasks", R.drawable.shell_clock),
        )
        ShellMode.Aci -> listOf(
            RailRow("mcp-apps", "MCP Apps", DesignR.drawable.ph_plugs_connected),
            RailRow("browser-extensions", "ACI Extensions", DesignR.drawable.ph_puzzle_piece),
            RailRow("computers", "Computers", DesignR.drawable.ph_desktop),
            RailRow("site-apis", "Teach", DesignR.drawable.ph_record_fill),
        )
        // iOS shows the bots rail (BotsDrawerRail) here; Bots mode arrives in a later step.
        ShellMode.Bots -> listOf(RailRow("bots", "Your bots", DesignR.drawable.ph_cpu), projects)
    }
}

/** Drawer rail rows the person hid from More › Show in rail (iOS RailTabsStore), per mode. */
internal object RailTabPrefs {
    private const val FILE = "allternit_shell"

    private fun key(mode: ShellMode) = "rail-hidden-${mode.raw}"

    fun hidden(context: Context, mode: ShellMode): Set<String> =
        context.getSharedPreferences(FILE, Context.MODE_PRIVATE).getStringSet(key(mode), emptySet()).orEmpty().toSet()

    fun toggle(context: Context, mode: ShellMode, id: String): Set<String> {
        val next = hidden(context, mode).let { if (id in it) it - id else it + id }
        context.getSharedPreferences(FILE, Context.MODE_PRIVATE).edit().putStringSet(key(mode), next).apply()
        return next
    }
}

/** Recents status filter (iOS RecentsStatusFilter, PhoneDrawer STATUS_OPTIONS). */
internal enum class RecentsStatusFilter(val label: String) { All("All"), Active("Active"), Completed("Completed"), Archived("Archived") }

/** Recents last-activity filter (iOS RecentsDateFilter, PhoneDrawer DATE_OPTIONS). */
internal enum class RecentsDateFilter(val label: String, val days: Long?) {
    All("Any time", null), Today("Today", 1), Week("Last 7 days", 7), Month("Last 30 days", 30)
}

// MARK: - Session metadata (iOS AgentSession.swift decoding of `metadata`)

private fun AgentSession.meta(key: String): JsonElement? = (metadata as? JsonObject)?.get(key)

private fun AgentSession.metaString(key: String): String? = (meta(key) as? JsonPrimitive)?.contentOrNull

private fun AgentSession.metaBool(key: String): Boolean = (meta(key) as? JsonPrimitive)?.booleanOrNull ?: false

internal val AgentSession.originSurface: String get() = metaString("originSurface").orEmpty()
internal val AgentSession.agentId: String? get() = metaString("agent_id")?.takeIf { it.isNotEmpty() }
/** The cowork project the session belongs to (`metadata.project_id`, iOS AgentSession.projectId). */
internal val AgentSession.projectId: String? get() = metaString("project_id")?.takeIf { it.isNotEmpty() }
internal val AgentSession.archived: Boolean get() = metaBool("archived")
internal val AgentSession.ephemeral: Boolean get() = metaBool("ephemeral")
internal val AgentSession.isTerminal: Boolean
    get() = ((meta("sourceRef") as? JsonObject)?.get("harness") as? JsonPrimitive)?.contentOrNull == "gizzi-tui"
internal val AgentSession.title: String get() = name?.trim()?.takeIf { it.isNotEmpty() } ?: "Untitled"

/** ShellSessionsStore.belongs: which mode's list a session sits in. */
internal fun AgentSession.belongs(mode: ShellMode): Boolean {
    val surface = originSurface
    return when (mode) {
        ShellMode.Bots -> agentId != null
        ShellMode.Home -> agentId == null && (surface == "chat" || surface == "cowork" || surface.isEmpty())
        ShellMode.Code -> agentId == null && surface == "code"
        ShellMode.Aci -> agentId == null && surface == "browser"
    }
}

/** updated_at, else created_at, as epoch millis (RFC 3339 from chrono `to_rfc3339`). 0 when absent. */
internal val AgentSession.timestampMillis: Long
    get() = parseMillis(updatedAt) ?: parseMillis(createdAt) ?: 0L

private fun parseMillis(value: String?): Long? =
    value?.let { runCatching { OffsetDateTime.parse(it).toInstant().toEpochMilli() }.getOrNull() }

/** The rail's relative time (session-summary formatRelativeTime): "12s", "5m", "3h", "2d". */
internal fun relativeTime(millis: Long, now: Long = System.currentTimeMillis()): String {
    if (millis <= 0L) return ""
    val seconds = ((now - millis) / 1000).coerceAtLeast(0)
    return when {
        seconds < 60 -> "${seconds}s"
        seconds < 3600 -> "${seconds / 60}m"
        seconds < 86_400 -> "${seconds / 3600}h"
        else -> "${seconds / 86_400}d"
    }
}

// MARK: - Data

/** One API client for the native shell (one OkHttp client per process). */
internal object ShellApi {
    val okHttp: okhttp3.OkHttpClient by lazy { ApiClients.okHttp(bearer = AllternitAuth.tokenProvider) }
    val api: AllternitApi by lazy { AllternitApi(okHttp, ApiConfig()) }
}

/**
 * The user's sessions for the drawer and the home (iOS ShellSessionsStore, list only).
 * Compose state, so screens recompose on change. Errors keep the last good list.
 */
internal class ShellSessionsStore(private val api: AllternitApi = ShellApi.api) {
    var sessions by mutableStateOf<List<AgentSession>>(emptyList())
        private set
    var isLoading by mutableStateOf(false)
        private set
    var hasLoaded by mutableStateOf(false)
        private set
    var loadError by mutableStateOf<String?>(null)
        private set
    /** The last load was refused by the compute gate: this account has no computer yet. */
    var noComputer by mutableStateOf(false)
        private set
    /** Sessions running on a cloud computer: id → its label (`GET teleports/placements`). Absent = local. */
    var cloudPlaces by mutableStateOf<Map<String, String>>(emptyMap())
        private set

    // MARK: Working / unread (iOS ShellSessionsStore seen, activeId, streaming)

    /** Where "last seen" lives (iOS UserDefaults `allternit-phone-session-seen`); set by the shell. */
    var seenPrefs: android.content.SharedPreferences? = null
        set(value) {
            field = value
            seen.clear()
            value?.getString(SEEN_KEY, null)?.let { raw ->
                runCatching {
                    (kotlinx.serialization.json.Json.parseToJsonElement(raw) as? JsonObject)?.forEach { (k, v) ->
                        (v as? JsonPrimitive)?.contentOrNull?.toLongOrNull()?.let { seen[k] = it }
                    }
                }
            }
        }
    private val seen = androidx.compose.runtime.mutableStateMapOf<String, Long>()
    private var activeId by mutableStateOf<String?>(null)
    private var streamingIds by mutableStateOf<Set<String>>(emptySet())

    fun isStreaming(id: String): Boolean = id in streamingIds

    /** New activity since the session was last seen, not while it streams or is on screen. */
    fun isUnread(session: AgentSession): Boolean {
        if (session.id == activeId || session.id in streamingIds) return false
        val last = seen[session.id] ?: return false
        return session.timestampMillis > last
    }

    /** The chat reports its session and whether a reply is streaming; null = it left the screen. */
    fun setActive(id: String?, streaming: Boolean) {
        val previous = activeId
        if (previous != null && previous != id) {
            markSeen(previous)
            if (id != null) streamingIds = streamingIds - previous
        }
        activeId = id
        if (id == null) return
        streamingIds = if (streaming) streamingIds + id else streamingIds - id
        markSeen(id)
    }

    /** A reply that keeps running (or ends) while its chat is off screen. */
    fun setStreaming(id: String?, streaming: Boolean) {
        if (id == null) return
        streamingIds = if (streaming) streamingIds + id else streamingIds - id
    }

    fun markSeen(id: String) {
        val at = sessions.firstOrNull { it.id == id }?.timestampMillis ?: 0L
        seen[id] = maxOf(at, seen[id] ?: 0L)
        saveSeen()
    }

    private fun saveSeen() {
        val prefs = seenPrefs ?: return
        val body = kotlinx.serialization.json.buildJsonObject { seen.forEach { (k, v) -> put(k, JsonPrimitive(v)) } }
        prefs.edit().putString(SEEN_KEY, body.toString()).apply()
    }

    suspend fun load() {
        if (isLoading) return
        isLoading = true
        try {
            when (val r = api.listAgentSessions()) {
                is ApiResult.Ok -> {
                    sessions = r.value.sessions
                        .filter { !it.ephemeral }
                        .sortedByDescending { it.timestampMillis }
                    loadError = null
                    noComputer = false
                    hasLoaded = true
                    // First sighting is the baseline: only later activity counts as unread.
                    sessions.forEach { s -> if (seen[s.id] == null || s.id == activeId) seen[s.id] = s.timestampMillis }
                    saveSeen()
                    loadPlaces()
                }
                is ApiResult.HttpError -> if (isNoComputer(r)) {
                    // No computer is not an error (iOS ShellSessionsStore.isNoComputer): the list is
                    // just empty, and the composer's computer chip says why.
                    sessions = emptyList()
                    cloudPlaces = emptyMap()
                    noComputer = true
                    loadError = null
                    hasLoaded = true
                } else {
                    loadError = statusSentence(r.status)
                        ?: FriendlyErrorMessage.from(r.message ?: "", "load conversations")
                }
                is ApiResult.Network ->
                    loadError = "Could not connect to the server. Check your connection and try again."
                is ApiResult.Decode ->
                    loadError = "Something went wrong. Check your connection and try again."
            }
        } finally {
            isLoading = false
        }
    }

    // MARK: Overview actions (iOS ShellSessionsStore rename / archive, OverviewBulkActions)

    private fun sessionUrl(id: String) =
        ApiConfig().apiBaseUrl.newBuilder().addPathSegment("agent-sessions").addPathSegment(id).build()

    private suspend fun patch(id: String, body: String): String? = describe(
        ApiCall.executeUnit(ShellApi.okHttp, ApiJson.default, ApiCall.request("PATCH", sessionUrl(id), body)),
    )

    private fun describe(r: ApiResult<*>): String? = when (r) {
        is ApiResult.Ok -> null
        is ApiResult.HttpError -> statusSentence(r.status) ?: FriendlyErrorMessage.from(r.message ?: "", "update the chat")
        is ApiResult.Network -> "Could not connect to the server. Check your connection and try again."
        is ApiResult.Decode -> null
    }

    /** `PATCH agent-sessions/:id { name }`. Optimistic; restored on failure. Returns the error. */
    suspend fun rename(id: String, name: String): String? {
        val before = sessions
        sessions = sessions.map { if (it.id == id) it.copy(name = name) else it }
        val body = kotlinx.serialization.json.buildJsonObject { put("name", JsonPrimitive(name)) }.toString()
        return patch(id, body)?.also { sessions = before }
    }

    /**
     * Close a card (X, swipe up, ⋯ → Archive): `metadata.archived` plus `active: false`, as iOS
     * archive(). Optimistic: the card leaves at once and comes back if the write fails.
     */
    suspend fun setArchived(id: String, archived: Boolean): String? {
        val before = sessions
        if (archived) sessions = sessions.filterNot { it.id == id }
        val body = """{"active":${!archived},"metadata":{"archived":$archived}}"""
        val error = patch(id, body)
        if (error != null) sessions = before else load()
        return error
    }

    /** `DELETE agent-sessions/:id` for good. Optimistic; the list comes back if it fails. */
    suspend fun delete(id: String): String? {
        val before = sessions
        sessions = sessions.filterNot { it.id == id }
        val error = describe(ApiCall.executeUnit(ShellApi.okHttp, ApiJson.default, ApiCall.request("DELETE", sessionUrl(id), null)))
        if (error != null) sessions = before
        return error
    }

    /** `GET teleports/placements` → `{ placements: { id: { targetId, label } } }`. An older backend has none: all local. */
    suspend fun loadPlaces() {
        val url = ApiConfig().apiBaseUrl.newBuilder().addPathSegments("teleports/placements").build()
        val r = ApiCall.execute(ShellApi.okHttp, ApiJson.default, ApiCall.request("GET", url, null), JsonObject.serializer())
        if (r is ApiResult.Ok) {
            cloudPlaces = (r.value["placements"] as? JsonObject).orEmpty().mapNotNull { (id, v) ->
                ((v as? JsonObject)?.get("label") as? JsonPrimitive)?.contentOrNull?.let { id to it }
            }.toMap()
        }
    }

    /** Move a Code session between the Mac and the cloud computer (`POST native-sessions/teleport`). */
    suspend fun teleport(id: String, toCloud: Boolean): String? {
        val url = ApiConfig().apiBaseUrl.newBuilder().addPathSegments("native-sessions/teleport").build()
        val body = """{"sessionId":${JsonPrimitive(id)},"to":"${if (toCloud) "cloud" else "local"}"}"""
        val error = describe(ApiCall.executeUnit(ShellApi.okHttp, ApiJson.default, ApiCall.request("POST", url, body)))
        if (error == null) loadPlaces()
        return error
    }

    /** Archived sessions of [mode] (the overview's Archived view). */
    fun archivedOf(mode: ShellMode): List<AgentSession> = sessions.filter { it.archived && it.belongs(mode) }

    /** Open (not archived) sessions of [mode], newest first. */
    fun of(mode: ShellMode): List<AgentSession> = sessions.filter { !it.archived && it.belongs(mode) }
}

/**
 * The compute gate's account-state refusals (iOS APIError.isComputeGateRefusal): cloud-api's 428
 * ("No data-plane node", pair a device), and 409/410 `compute_required` / `free_computers_retired`.
 */
private const val SEEN_KEY = "allternit-phone-session-seen"

internal fun isNoComputer(r: ApiResult.HttpError): Boolean {
    if (r.status == 428) return true
    if (r.status != 409 && r.status != 410) return false
    val text = "${r.code.orEmpty()} ${r.message.orEmpty()}"
    return Regex("""\b(compute_required|free_computers_retired)\b""", RegexOption.IGNORE_CASE).containsMatchIn(text) ||
        text.contains("data-plane node", ignoreCase = true)
}

/** The sentence for a status with no usable message (iOS statusSentence, web statusSentence). */
internal fun statusSentence(status: Int): String? = when (status) {
    401 -> "Sign in again to continue."
    403 -> "You don't have access to that."
    404 -> "That isn't there anymore."
    408, 504 -> "That took too long. Try again."
    429 -> "Too many requests. Wait a moment and try again."
    in 500..599 -> "Something went wrong on our side. Try again."
    else -> null
}

/** iOS CalmCopy: the no-computer chip and empty states. */
internal object NoComputerCopy {
    const val TITLE = "No computer connected"
    const val DETAIL = "Connect a computer or start Allternit Cloud to use this."
}

/** Pinned recents, local to this phone (iOS RailPinsStore, key `allternit:rail:pinned`, ids only). */
internal object RailPinPrefs {
    private const val FILE = "allternit_shell"
    private const val KEY = "allternit:rail:pinned"

    fun pinned(context: Context): Set<String> =
        context.getSharedPreferences(FILE, Context.MODE_PRIVATE).getStringSet(KEY, emptySet()).orEmpty().toSet()

    fun set(context: Context, ids: Set<String>) {
        context.getSharedPreferences(FILE, Context.MODE_PRIVATE).edit().putStringSet(KEY, ids).apply()
    }
}
