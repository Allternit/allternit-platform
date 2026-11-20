package com.allternit.feature.meetings

import android.content.Context
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.allternit.core.api.ApiCall
import com.allternit.core.api.ApiJson
import com.allternit.core.api.ApiResult
import com.allternit.feature.artifacts.LibraryNet
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.Locale
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.put
import okhttp3.HttpUrl.Companion.toHttpUrl
import okhttp3.Request

// ---- JSON helpers (snake_case, like iOS MeetingsClient.decoder) ----
private fun JsonObject.s(key: String): String? = (this[key] as? JsonPrimitive)?.contentOrNull
private fun JsonObject.i(key: String): Int? = (this[key] as? JsonPrimitive)?.intOrNull
private fun JsonObject.b(key: String): Boolean? = (this[key] as? JsonPrimitive)?.booleanOrNull
private fun JsonObject.o(key: String): JsonObject? = this[key] as? JsonObject
private fun JsonObject.a(key: String): List<JsonElement> = (this[key] as? JsonArray).orEmpty()

internal fun parseDate(s: String?): Instant? = s?.takeIf { it.isNotEmpty() }?.let { runCatching { Instant.parse(it) }.getOrNull() }

/** iOS GizziStatus. */
enum class GizziStatus(val raw: String) {
    Scheduled("scheduled"), Off("off"), Launching("launching"), Joining("joining"), WaitingAdmit("waiting_admit"),
    Live("live"), Paused("paused"), Ended("ended"), NotAdmitted("not_admitted");

    val isActive: Boolean get() = this == Launching || this == Joining || this == WaitingAdmit || this == Live || this == Paused

    companion object { fun of(raw: String?): GizziStatus = entries.firstOrNull { it.raw == raw } ?: Off }
}

data class MeetingActionItem(val text: String, val owner: String?, val due: String?)

data class Meeting(
    val id: String, val platform: String, val platformLabel: String?, val title: String, val joinUrl: String,
    val startsAt: String?, val isOneOnOne: Boolean?, val autoJoin: Boolean?, val gizzi: String, val peopleLabel: String?,
    val latestSessionId: String?, val liveAt: String?, val latestSessionLiveAt: String?, val latestSessionIdNested: String?,
) {
    val status: GizziStatus get() = GizziStatus.of(gizzi)
    val startDate: Instant? get() = parseDate(startsAt)
    val liveDate: Instant? get() = parseDate(liveAt ?: latestSessionLiveAt)
    val sessionId: String? get() = latestSessionIdNested ?: latestSessionId
    val displayTitle: String get() = title.trim().ifEmpty { "$platformName call" }
    val platformName: String
        get() = platformLabel?.takeIf { it.isNotEmpty() } ?: when (platform) {
            "meet" -> "Google Meet"; "zoom" -> "Zoom"; "teams" -> "Microsoft Teams"; else -> "Call"
        }
    val gizziOn: Boolean
        get() = when (status) {
            GizziStatus.Off, GizziStatus.NotAdmitted -> false
            GizziStatus.Ended -> autoJoin ?: false
            else -> true
        }

    companion object {
        fun from(j: JsonObject): Meeting {
            val latest = j.o("latest_session")
            return Meeting(
                id = j.s("id").orEmpty(), platform = j.s("platform").orEmpty(), platformLabel = j.s("platform_label"), title = j.s("title").orEmpty(),
                joinUrl = j.s("join_url").orEmpty(), startsAt = j.s("starts_at"), isOneOnOne = j.b("is_one_on_one"), autoJoin = j.b("auto_join"),
                gizzi = j.s("gizzi") ?: "off", peopleLabel = j.s("people_label"), latestSessionId = j.s("latest_session_id"),
                liveAt = j.s("live_at"), latestSessionLiveAt = latest?.s("live_at"), latestSessionIdNested = latest?.s("id"),
            )
        }
    }
}

data class MeetingSession(
    val id: String, val meetingId: String, val state: String, val gizzi: String, val endReason: String?, val platform: String?,
    val speakReplies: Boolean, val listening: Boolean, val liveAt: String?, val endedAt: String?, val summary: String?,
    val decisions: List<String>?, val actionItems: List<MeetingActionItem>?,
) {
    val status: GizziStatus get() = GizziStatus.of(gizzi)
    val liveDate: Instant? get() = parseDate(liveAt)
    val endedDate: Instant? get() = parseDate(endedAt)

    companion object {
        fun from(j: JsonObject) = MeetingSession(
            id = j.s("id").orEmpty(), meetingId = j.s("meeting_id").orEmpty(), state = j.s("state").orEmpty(), gizzi = j.s("gizzi") ?: "off",
            endReason = j.s("end_reason"), platform = j.s("platform"), speakReplies = j.b("speak_replies") ?: false, listening = j.b("listening") ?: true,
            liveAt = j.s("live_at"), endedAt = j.s("ended_at"), summary = j.s("summary"),
            decisions = (j["decisions"] as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.contentOrNull },
            actionItems = (j["action_items"] as? JsonArray)?.mapNotNull { (it as? JsonObject)?.let { o -> MeetingActionItem(o.s("text").orEmpty(), o.s("owner"), o.s("due")) } },
        )
    }
}

/** iOS MeetingRunner: which of the person's own computers runs the meeting bot. */
data class MeetingRunner(val state: String, val runtimeName: String?) {
    val isNone: Boolean get() = state != "ready" && state != "asleep"
    val text: String
        get() {
            val name = runtimeName?.trim().orEmpty()
            return when {
                isNone -> NONE_TEXT
                state == "asleep" -> if (name.isEmpty()) "Your cloud computer is asleep. It wakes when Gizzi joins." else "$name is asleep. It wakes when Gizzi joins."
                else -> if (name.isEmpty()) "Gizzi joins from your computer." else "Gizzi joins from $name."
            }
        }

    companion object {
        const val NONE_TEXT = "No computer is online. Gizzi joins calls from your own computer, not from Allternit’s servers. Open Allternit on a computer that has Google Chrome, or add a cloud computer."
        const val NONE_ACTION = "Get a cloud computer"
    }
}

data class TranscriptSegment(val id: String, val seq: Int, val speaker: String?, val startMs: Int, val text: String)
data class MeetingArtifact(val id: String, val kind: String, val title: String, val url: String, val version: Int?)
data class ToolApproval(val id: String, val tool: String, val argsSummary: String, val askedBy: String?, val status: String)

private fun seg(o: JsonObject) = TranscriptSegment(o.s("id") ?: o.i("seq").toString(), o.i("seq") ?: 0, o.s("speaker"), o.i("start_ms") ?: 0, o.s("text").orEmpty())
private fun art(o: JsonObject) = MeetingArtifact(o.s("id").orEmpty(), o.s("kind").orEmpty(), o.s("title").orEmpty(), o.s("url").orEmpty(), o.i("version"))
private fun appr(o: JsonObject) = ToolApproval(o.s("id").orEmpty(), o.s("tool").orEmpty(), o.s("args_summary").orEmpty(), o.s("asked_by"), o.s("status").orEmpty())

/** iOS MeetingsError, with the same copy. */
sealed class MeetingsError(val message: String) {
    data object SignedOut : MeetingsError("Sign in to use Meetings.")
    data object PaidPlan : MeetingsError("Meetings is part of the Plus, Super and Ultra plans.")
    class Forbidden(m: String) : MeetingsError(m)
    data object NoRuntime : MeetingsError(MeetingRunner.NONE_TEXT)
    class Other(m: String) : MeetingsError(m)
}

class MeetingsException(val error: MeetingsError) : Exception(error.message)

/** iOS MeetingsClient: cloud-api `api/v1/meetings…` and `meeting-sessions…` with the account token. */
object MeetingsClient {
    private fun url(path: String, query: Map<String, String> = emptyMap()) =
        (LibraryNet.config.cloudBaseUrl.toString().trimEnd('/') + "/api/v1/" + path).toHttpUrl().newBuilder().apply {
            query.forEach { (k, v) -> addQueryParameter(k, v) }
        }.build()

    private fun error(status: Int, code: String?, message: String?): MeetingsError = when {
        status == 401 -> MeetingsError.SignedOut
        status == 402 || code == "meetings_requires_paid_plan" -> MeetingsError.PaidPlan
        status == 403 -> MeetingsError.Forbidden(message ?: "Your organization has turned Meetings off.")
        code == "unsupported_link" -> MeetingsError.Other("That isn't a Google Meet, Zoom or Microsoft Teams link.")
        code == "zoom_not_connected" -> MeetingsError.Other("Connect your Zoom account first. Zoom only lets Gizzi in through the account of someone in the call.")
        status == 429 -> MeetingsError.Other("Gizzi is in too many calls right now. Try again in a minute.")
        status == 428 || code == "compute_required" -> MeetingsError.NoRuntime
        else -> MeetingsError.Other(message ?: "Meetings answered $status. Try again.")
    }

    private suspend fun send(method: String, path: String, body: JsonElement? = null, query: Map<String, String> = emptyMap()): JsonObject {
        val request = ApiCall.request(method, url(path, query), body?.toString() ?: if (method == "GET") null else "{}")
        return when (val r = ApiCall.execute(LibraryNet.okHttp, ApiJson.default, request, JsonElement.serializer())) {
            is ApiResult.Ok -> (r.value as? JsonObject) ?: JsonObject(emptyMap())
            is ApiResult.HttpError -> throw MeetingsException(error(r.status, r.code, r.message))
            is ApiResult.Network -> throw MeetingsException(MeetingsError.Other("You're offline. Check your connection and try again."))
            else -> throw MeetingsException(MeetingsError.Other("Meetings sent something this app can't read yet. Update the app."))
        }
    }

    suspend fun list(range: String, limit: Int = 50) = send("GET", "meetings", query = mapOf("range" to range, "limit" to "$limit")).a("meetings").mapNotNull { (it as? JsonObject)?.let(Meeting::from) }
    suspend fun meeting(id: String) = Meeting.from(send("GET", "meetings/$id"))
    suspend fun runner(): MeetingRunner = send("GET", "meetings/runner").let { MeetingRunner(it.s("state").orEmpty(), it.o("runtime")?.s("name")) }
    suspend fun create(link: String) = Meeting.from(send("POST", "meetings", buildJsonObject { put("url", link) }))
    suspend fun setAutoJoin(id: String, on: Boolean) = Meeting.from(send("PATCH", "meetings/$id", buildJsonObject { put("auto_join", on) }))
    suspend fun startSession(id: String, speakReplies: Boolean?) = MeetingSession.from(send("POST", "meetings/$id/sessions", buildJsonObject {
        put("when", "now"); if (speakReplies != null) put("speak_replies", speakReplies)
    }))
    suspend fun session(sid: String): Triple<MeetingSession, Meeting, Pair<Int, List<ToolApproval>>> {
        val j = send("GET", "meeting-sessions/$sid")
        val approvals = j.a("pending_approvals").mapNotNull { (it as? JsonObject)?.let(::appr) }.filter { it.status == "pending" }
        return Triple(MeetingSession.from(j.o("session") ?: JsonObject(emptyMap())), Meeting.from(j.o("meeting") ?: JsonObject(emptyMap())), j.a("participants").size to approvals)
    }
    suspend fun transcript(sid: String, limit: Int = 500) = send("GET", "meeting-sessions/$sid/transcript", query = mapOf("limit" to "$limit")).a("segments").mapNotNull { (it as? JsonObject)?.let(::seg) }
    suspend fun artifacts(sid: String) = send("GET", "meeting-sessions/$sid/artifacts").a("artifacts").mapNotNull { (it as? JsonObject)?.let(::art) }
    suspend fun control(sid: String, action: String) = MeetingSession.from(send("POST", "meeting-sessions/$sid/control", buildJsonObject { put("action", action) }))
    suspend fun setSpeakReplies(sid: String, on: Boolean) = MeetingSession.from(send("PATCH", "meeting-sessions/$sid", buildJsonObject { put("speak_replies", on) }))
    suspend fun decide(sid: String, approvalId: String, approve: Boolean) { send("POST", "meeting-sessions/$sid/approvals/$approvalId", buildJsonObject { put("decision", if (approve) "approve" else "deny") }) }
    suspend fun settingsReplyMode(): String? = runCatching { send("GET", "me/meeting-settings").s("default_reply_mode") }.getOrNull()
    /** iOS AccountFlagsClient.string("meetings"): `GET /me/flags`. */
    suspend fun meetingsFlag(): Boolean? = runCatching { send("GET", "me/flags").let { it["meetings"] != null && it["meetings"] !is JsonNull } }.getOrNull()

    /**
     * iOS MeetingsClient.events: the session's SSE stream (`text/event-stream`, Last-Event-ID resume).
     * Calls [onEvent] with (id, type, data) per frame until the stream ends; throws on a failed open.
     */
    suspend fun events(sid: String, lastEventId: String?, onEvent: (String?, String, String) -> Unit) = withContext(Dispatchers.IO) {
        val request = Request.Builder().url(url("meeting-sessions/$sid/events")).header("Accept", "text/event-stream")
            .apply { if (lastEventId != null) header("Last-Event-ID", lastEventId) }.build()
        LibraryNet.okHttp.newCall(request).execute().use { response ->
            if (!response.isSuccessful) throw MeetingsException(error(response.code, null, null))
            val source = response.body?.source() ?: return@use
            var id: String? = null; var type = "message"; val data = StringBuilder()
            while (isActive) {
                val line = source.readUtf8Line() ?: break
                if (line.isEmpty()) {
                    if (data.isNotEmpty()) onEvent(id, type, data.toString())
                    type = "message"; data.clear(); continue
                }
                if (line.startsWith(":")) continue
                val colon = line.indexOf(':')
                val field = if (colon < 0) line else line.substring(0, colon)
                val value = if (colon < 0) "" else line.substring(colon + 1).removePrefix(" ")
                when (field) {
                    "id" -> id = value
                    "event" -> type = value
                    "data" -> { if (data.isNotEmpty()) data.append('\n'); data.append(value) }
                }
            }
        }
    }
}

/** iOS MeetingsStore: the agenda (live, upcoming, past), sessions per meeting, the runner and the flag. */
object MeetingsStore {
    sealed interface Phase {
        data object Idle : Phase; data object Loading : Phase; data object Loaded : Phase
        data class Failed(val message: String) : Phase; data object SignedOut : Phase; data object PaidPlan : Phase
        data class Forbidden(val message: String) : Phase
    }

    var phase by mutableStateOf<Phase>(Phase.Idle); private set
    val live = mutableStateListOf<Meeting>()
    val upcoming = mutableStateListOf<Meeting>()
    val past = mutableStateListOf<Meeting>()
    val sessions = mutableStateMapOf<String, MeetingSession>()
    val pendingSwitches = mutableStateListOf<String>()
    var runner by mutableStateOf<MeetingRunner?>(null); private set
    var replyMode by mutableStateOf<String?>(null); private set
    var actionError by mutableStateOf<String?>(null)
    var isEnabled by mutableStateOf(false); private set
    private var flagLoaded = false

    /** The drawer entry shows only with the account's `meetings` flag (cached on the device, like iOS). */
    suspend fun refreshFlag(context: Context, signedIn: Boolean) {
        val prefs = context.getSharedPreferences("allternit_meetings", Context.MODE_PRIVATE)
        if (!flagLoaded) { isEnabled = prefs.getBoolean("flag", false); flagLoaded = true }
        if (!signedIn) { isEnabled = false; prefs.edit().putBoolean("flag", false).apply(); return }
        MeetingsClient.meetingsFlag()?.let { isEnabled = it; prefs.edit().putBoolean("flag", it).apply() }
    }

    fun session(m: Meeting): MeetingSession? = sessions[m.id]

    suspend fun load(signedIn: Boolean, quiet: Boolean = false) {
        if (!signedIn) { phase = Phase.SignedOut; clear(); return }
        if (!quiet || phase != Phase.Loaded) phase = Phase.Loading
        try {
            coroutineScope {
                val runnerJob = async { runCatching { MeetingsClient.runner() }.getOrNull() }
                val modeJob = async { MeetingsClient.settingsReplyMode() }
                val now = async { MeetingsClient.list("now") }
                val next = async { MeetingsClient.list("upcoming") }
                val done = async { MeetingsClient.list("past", 30) }
                apply(now.await(), next.await(), done.await())
                phase = Phase.Loaded
                runner = runnerJob.await()
                replyMode = modeJob.await()
            }
            loadSessions()
            syncNotices()
        } catch (e: MeetingsException) {
            when (val err = e.error) {
                MeetingsError.SignedOut -> { phase = Phase.SignedOut; clear() }
                MeetingsError.PaidPlan -> { phase = Phase.PaidPlan; clear() }
                is MeetingsError.Forbidden -> { phase = Phase.Forbidden(err.message); clear() }
                else -> if (!(quiet && phase == Phase.Loaded)) phase = Phase.Failed(err.message)
            }
        }
    }

    /** Posts a banner (iOS MeetingNotifier): set by the screen, which owns a Context. */
    var notifier: ((String, Meeting) -> Unit)? = null
    private val seenStates = HashMap<String, String>()

    /** What a notification would say about this meeting now (iOS noticeKey). */
    private fun noticeKey(m: Meeting, session: MeetingSession?): String = when (m.status) {
        GizziStatus.Launching, GizziStatus.Joining -> "joining"
        GizziStatus.WaitingAdmit -> "waiting_admit"
        GizziStatus.Live, GizziStatus.Paused -> "live"
        GizziStatus.NotAdmitted -> "not_admitted"
        GizziStatus.Ended -> if (session?.summary != null) "recap_ready" else "ended"
        GizziStatus.Scheduled, GizziStatus.Off -> m.status.raw
    }

    /** Joining, waiting in the lobby, not let in, and recap ready: one banner per change; a meeting seen for the first time posts none. */
    private fun syncNotices() {
        for (m in live + past.take(12)) {
            val key = noticeKey(m, sessions[m.id])
            val previous = seenStates[m.id]
            if (previous == key) continue
            seenStates[m.id] = key
            if (previous == null) continue
            if (key in setOf("joining", "waiting_admit", "not_admitted", "recap_ready")) notifier?.invoke(key, m)
        }
        val known = (live + upcoming + past).map { it.id }.toSet()
        seenStates.keys.retainAll(known)
    }

    private fun clear() { live.clear(); upcoming.clear(); past.clear(); sessions.clear(); seenStates.clear() }

    private fun apply(now: List<Meeting>, next: List<Meeting>, done: List<Meeting>) {
        val seen = HashSet<String>()
        val all = (now + next).filter { seen.add(it.id) }
        live.clear(); live.addAll(all.filter { it.status.isActive })
        upcoming.clear(); upcoming.addAll(all.filter { !it.status.isActive && it.status != GizziStatus.Ended && it.status != GizziStatus.NotAdmitted }.sortedBy { it.startDate ?: Instant.MAX })
        val endedToday = all.filter { it.status == GizziStatus.Ended || it.status == GizziStatus.NotAdmitted }
        val seenPast = HashSet<String>()
        past.clear(); past.addAll((endedToday + done).filter { seenPast.add(it.id) }.sortedByDescending { it.startDate ?: Instant.MIN })
    }

    private suspend fun loadSessions() {
        val wanted = (live + past.take(12)).mapNotNull { m -> m.sessionId?.let { m.id to it } }
        for ((meetingId, sid) in wanted) {
            val known = sessions[meetingId]
            if (known != null && !known.status.isActive && !(known.status == GizziStatus.Ended && known.summary == null)) continue
            runCatching { MeetingsClient.session(sid).first }.getOrNull()?.let { sessions[meetingId] = it }
        }
    }

    fun didUpdate(session: MeetingSession, meeting: Meeting?) {
        sessions[session.meetingId] = session
        fun patch(rows: MutableList<Meeting>) { rows.indexOfFirst { it.id == session.meetingId }.takeIf { it >= 0 }?.let { rows[it] = rows[it].copy(gizzi = session.gizzi) } }
        patch(live); patch(upcoming); patch(past)
        if (session.status.isActive && live.none { it.id == session.meetingId } && meeting != null) {
            live.add(0, meeting.copy(gizzi = session.gizzi)); upcoming.removeAll { it.id == meeting.id }
        } else if (!session.status.isActive) {
            live.indexOfFirst { it.id == session.meetingId }.takeIf { it >= 0 }?.let { past.add(0, live.removeAt(it)) }
        }
    }

    private suspend fun stillNoRunner(): Boolean {
        if (runner?.isNone != true) return false
        runner = runCatching { MeetingsClient.runner() }.getOrNull()
        return runner?.isNone == true
    }

    suspend fun setGizzi(m: Meeting, on: Boolean) {
        if (m.id in pendingSwitches) return
        pendingSwitches += m.id
        val before = upcoming.toList()
        upcoming.indexOfFirst { it.id == m.id }.takeIf { it >= 0 }?.let { upcoming[it] = upcoming[it].copy(autoJoin = on, gizzi = if (on) "scheduled" else "off") }
        try {
            val updated = MeetingsClient.setAutoJoin(m.id, on)
            upcoming.indexOfFirst { it.id == updated.id }.takeIf { it >= 0 }?.let { upcoming[it] = updated }
        } catch (e: MeetingsException) {
            upcoming.clear(); upcoming.addAll(before); handle(e.error)
        } finally { pendingSwitches -= m.id }
    }

    /** Paste a link: save it, and with [joinNow] send Gizzi now (from one of the person's own computers). */
    suspend fun addLink(link: String, joinNow: Boolean, signedIn: Boolean): Pair<Meeting, MeetingSession?> {
        if (joinNow && stillNoRunner()) throw MeetingsException(MeetingsError.NoRuntime)
        val meeting = MeetingsClient.create(link.trim())
        var session: MeetingSession? = null
        if (joinNow) {
            session = MeetingsClient.startSession(meeting.id, speakReplies = replyMode == "voice")
            didUpdate(session, meeting)
        }
        load(signedIn, quiet = true)
        return meeting to session
    }

    suspend fun sendNow(m: Meeting): MeetingSession? {
        if (stillNoRunner()) { handle(MeetingsError.NoRuntime); return null }
        return try {
            MeetingsClient.startSession(m.id, speakReplies = replyMode == "voice").also { didUpdate(it, m) }
        } catch (e: MeetingsException) { handle(e.error); null }
    }

    fun handle(e: MeetingsError) {
        when (e) {
            MeetingsError.PaidPlan -> phase = Phase.PaidPlan
            MeetingsError.SignedOut -> phase = Phase.SignedOut
            else -> actionError = e.message
        }
    }
}

/** iOS MeetingLiveSession: one session's detail, transcript, artifacts and approvals, kept live by SSE. */
class MeetingLiveSession(val sessionId: String, initial: Meeting?) {
    sealed interface Phase { data object Loading : Phase; data object Ready : Phase; data class Failed(val m: String) : Phase; data object PaidPlan : Phase; data object SignedOut : Phase }

    var phase by mutableStateOf<Phase>(Phase.Loading); private set
    var session by mutableStateOf<MeetingSession?>(null); private set
    var meeting by mutableStateOf(initial); private set
    val transcript = mutableStateListOf<TranscriptSegment>()
    val artifacts = mutableStateListOf<MeetingArtifact>()
    val approvals = mutableStateListOf<ToolApproval>()
    var lastReply by mutableStateOf<String?>(null); private set
    var busyAction by mutableStateOf<String?>(null); private set
    var actionError by mutableStateOf<String?>(null)
    private var lastEventId: String? = null

    val status: GizziStatus get() = session?.status ?: meeting?.status ?: GizziStatus.Joining
    val isEnded: Boolean get() = status == GizziStatus.Ended || status == GizziStatus.NotAdmitted

    suspend fun loadAll() {
        try {
            val (s, m, extra) = MeetingsClient.session(sessionId)
            session = s; meeting = m
            approvals.clear(); approvals.addAll(extra.second)
            MeetingsStore.didUpdate(s, m)
            merge(MeetingsClient.transcript(sessionId))
            val a = MeetingsClient.artifacts(sessionId)
            artifacts.clear(); artifacts.addAll(a)
            phase = Phase.Ready
        } catch (e: MeetingsException) {
            when (e.error) {
                MeetingsError.PaidPlan -> phase = Phase.PaidPlan
                MeetingsError.SignedOut -> phase = Phase.SignedOut
                else -> if (session == null) phase = Phase.Failed(e.error.message)
            }
        }
    }

    /** Load, then follow the event stream with backoff; poll for the recap after the call ends. */
    suspend fun run() {
        loadAll()
        var failures = 0
        while (kotlinx.coroutines.currentCoroutineContext().isActive && !isEnded) {
            if (failures >= 3) { delay(4_000); loadAll(); continue }
            try {
                MeetingsClient.events(sessionId, lastEventId) { id, type, data ->
                    failures = 0
                    if (id != null) lastEventId = id
                    handle(type, data)
                }
            } catch (e: MeetingsException) {
                if (e.error == MeetingsError.PaidPlan) { phase = Phase.PaidPlan; return }
                if (e.error == MeetingsError.SignedOut) { phase = Phase.SignedOut; return }
                failures += 1
            } catch (e: java.io.IOException) { failures += 1 }
            if (isEnded) break
            delay(minOf(15_000L, 1000L shl failures))
            loadAll()
        }
        repeat(12) { if (session?.summary != null) return; delay(10_000); loadAll() }
    }

    private fun handle(type: String, data: String) {
        val j = runCatching { ApiJson.default.parseToJsonElement(data).jsonObject }.getOrNull() ?: return
        // The SSE reader runs on IO; snapshot state writes are thread-safe, list edits go through main.
        android.os.Handler(android.os.Looper.getMainLooper()).post {
            when (type) {
                "meeting.session.state" -> session?.let { s ->
                    val st = j.s("state").orEmpty()
                    val ns = s.copy(state = st, gizzi = j.s("gizzi") ?: s.gizzi, endReason = j.s("end_reason"), listening = st != "paused",
                        liveAt = if (st == "live" && s.liveAt == null) Instant.now().toString() else s.liveAt)
                    session = ns; MeetingsStore.didUpdate(ns, meeting)
                }
                "meeting.transcript.delta" -> merge(j.a("segments").mapNotNull { (it as? JsonObject)?.let(::seg) })
                "meeting.reply" -> lastReply = j.s("text")
                "meeting.approval.requested" -> j.o("approval")?.let(::appr)?.let { a -> approvals.removeAll { it.id == a.id }; approvals.add(a) }
                "meeting.approval.resolved" -> j.o("approval")?.let(::appr)?.let { a -> approvals.removeAll { it.id == a.id } }
                "meeting.artifact.created", "meeting.artifact.updated" -> j.o("artifact")?.let(::art)?.let { a ->
                    val i = artifacts.indexOfFirst { it.id == a.id }; if (i >= 0) artifacts[i] = a else artifacts.add(a)
                }
                "meeting.ended" -> session?.let { s ->
                    val reason = j.s("end_reason").orEmpty()
                    val ns = s.copy(state = "ended", endReason = reason, gizzi = if (reason == "not_admitted" || reason == "guest_blocked") "not_admitted" else "ended", endedAt = Instant.now().toString())
                    session = ns; MeetingsStore.didUpdate(ns, meeting)
                }
                "meeting.summary" -> session?.let { s ->
                    val ns = s.copy(summary = j.s("summary"), decisions = j.a("decisions").mapNotNull { (it as? JsonPrimitive)?.contentOrNull },
                        actionItems = j.a("action_items").mapNotNull { (it as? JsonObject)?.let { o -> MeetingActionItem(o.s("text").orEmpty(), o.s("owner"), o.s("due")) } })
                    session = ns; MeetingsStore.didUpdate(ns, meeting)
                }
            }
        }
    }

    private fun merge(segments: List<TranscriptSegment>) {
        if (segments.isEmpty()) return
        val bySeq = LinkedHashMap<Int, TranscriptSegment>()
        transcript.forEach { bySeq[it.seq] = it }
        segments.forEach { bySeq[it.seq] = it }
        transcript.clear(); transcript.addAll(bySeq.values.sortedBy { it.seq })
    }

    suspend fun toggleListening() { val a = if (status == GizziStatus.Paused) "resume" else "stop_listening"; run(a) { MeetingsClient.control(sessionId, a) } }
    suspend fun setSpeakReplies(on: Boolean) = run("speak") { MeetingsClient.setSpeakReplies(sessionId, on) }
    suspend fun leave() = run("leave") { MeetingsClient.control(sessionId, "leave") }

    private suspend fun run(name: String, body: suspend () -> MeetingSession) {
        if (busyAction != null) return
        busyAction = name
        try { val s = body(); session = s; MeetingsStore.didUpdate(s, meeting) } catch (e: MeetingsException) { actionError = e.error.message } finally { busyAction = null }
    }

    suspend fun decide(a: ToolApproval, approve: Boolean) {
        try { MeetingsClient.decide(sessionId, a.id, approve); approvals.removeAll { it.id == a.id } } catch (e: MeetingsException) { actionError = e.error.message }
    }
}

/** iOS MeetingFormat: the shared labels. */
object MeetingFormat {
    private val zone: ZoneId get() = ZoneId.systemDefault()

    fun stateLabel(status: GizziStatus, session: MeetingSession?, platform: String? = null): String = when {
        status == GizziStatus.WaitingAdmit && (session?.platform ?: platform) == "zoom" -> "Waiting for you to join Zoom"
        else -> when (status) {
            GizziStatus.Scheduled -> "Gizzi joins"; GizziStatus.Off -> "Off"; GizziStatus.Launching, GizziStatus.Joining -> "Joining"
            GizziStatus.WaitingAdmit -> "Waiting to be let in"; GizziStatus.Live -> "Live"; GizziStatus.Paused -> "Paused"
            GizziStatus.NotAdmitted -> "Not let in"; GizziStatus.Ended -> if (session?.summary != null) "Recap ready" else "Ended"
        }
    }

    fun endReason(reason: String?): String = when (reason) {
        "left_by_user" -> "You asked Gizzi to leave."
        "removed" -> "Gizzi was removed from the call."
        "not_admitted" -> "Nobody let Gizzi in."
        "empty_room" -> "Everyone left, so Gizzi did too."
        "max_duration" -> "Gizzi left after 8 hours."
        "other_notetaker" -> "Another notetaker joined, so Gizzi left."
        "join_failed" -> "Gizzi couldn't join the call."
        "guest_blocked" -> "The call doesn't let guests like Gizzi in."
        "no_capacity" -> "Gizzi was in too many calls to join."
        "no_runtime" -> "Gizzi could not join. None of your computers was online. Open Allternit on your computer or start your cloud computer, then send Gizzi again."
        "error" -> "Something went wrong in the call."
        else -> "Gizzi left the call."
    }

    fun recapLine(m: Meeting, s: MeetingSession?): String {
        if (m.status == GizziStatus.NotAdmitted) return "Not let in"
        if (s == null) return if (m.status == GizziStatus.Ended) "Ended" else m.platformName
        if (s.summary != null) {
            val parts = mutableListOf("Recap ready")
            val d = s.decisions?.size ?: 0; val a = s.actionItems?.size ?: 0
            if (d > 0) parts += "$d decision${if (d == 1) "" else "s"}"
            if (a > 0) parts += "$a action item${if (a == 1) "" else "s"}"
            return parts.joinToString(" · ")
        }
        if (s.status == GizziStatus.Ended && s.endReason in listOf("left_by_user", "empty_room", "removed", "max_duration")) return "Writing the recap…"
        return endReason(s.endReason)
    }

    fun minutes(start: Instant, end: Instant): String {
        val m = maxOf(0L, (end.epochSecond - start.epochSecond) / 60)
        return if (m < 60) "$m min" else "${m / 60} h ${m % 60} min"
    }

    fun time(d: Instant?): String = d?.let { DateTimeFormatter.ofPattern("h:mm a", Locale.getDefault()).format(it.atZone(zone)) } ?: "Now"

    fun dayLabel(d: Instant?): String {
        d ?: return "Links you added"
        val day = d.atZone(zone).toLocalDate(); val today = LocalDate.now(zone)
        return when (day) { today -> "Today"; today.plusDays(1) -> "Tomorrow"; else -> DateTimeFormatter.ofPattern("EEEE, MMM d", Locale.getDefault()).format(day) }
    }

    fun shortDay(d: Instant?): String {
        d ?: return ""
        val day = d.atZone(zone).toLocalDate(); val today = LocalDate.now(zone)
        return when (day) { today -> time(d); today.minusDays(1) -> "Yesterday"; else -> DateTimeFormatter.ofPattern("MMM d", Locale.getDefault()).format(day) }
    }

    private val patterns = listOf(
        Regex("^https://meet\\.google\\.com/([a-z]{3}-[a-z]{4}-[a-z]{3}|lookup/\\S+)"),
        Regex("^https://([a-z0-9-]+\\.)?zoom\\.(us|com)/(j|my|wc)/"),
        Regex("^https://teams\\.(microsoft|live)\\.com/(l/meetup-join/|meet/)"),
    )
    fun isCallLink(text: String): Boolean = text.trim().lowercase().let { s -> patterns.any { it.containsMatchIn(s) } }
}
