package com.allternit.feature.computer

import android.content.Context
import android.net.Uri
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.allternit.core.api.ApiCall
import com.allternit.core.api.ApiClients
import com.allternit.core.api.ApiConfig
import com.allternit.core.api.ApiJson
import com.allternit.core.api.ApiResult
import com.allternit.core.auth.AllternitAuth
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.arr
import com.allternit.feature.artifacts.bool
import com.allternit.feature.artifacts.child
import com.allternit.feature.artifacts.list
import com.allternit.feature.artifacts.listUnder
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.str
import com.allternit.feature.chat.ChatBackend
import com.allternit.feature.chat.NoComputerException
import com.allternit.feature.chat.RuntimeCall
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.callbackFlow
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.flow.flow
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.put
import com.allternit.core.stream.SseParser
import java.io.IOException
import java.util.concurrent.TimeUnit
import kotlin.coroutines.resume
import okhttp3.Call
import okhttp3.Callback
import okhttp3.HttpUrl.Companion.toHttpUrl
import okhttp3.Response
import org.json.JSONArray
import org.json.JSONObject

/** ACI's four views (iOS ACITab, the web phone's `AciTab`): the drawer's ACI rail picks one. */
enum class AciTab { Agent, Apps, Computers, Extensions }

/** Which ACI view shows, shared by the drawer rail and the ACI screen (iOS ACINavStore). */
object AciNav {
    var tab: AciTab by mutableStateOf(AciTab.Agent)
    /** Teach (iOS ACINavStore.isTeachOpen): a full screen over ACI, opened from the drawer's Teach row or the Teach nudge. */
    var teachOpen: Boolean by mutableStateOf(false)
    fun open(tab: AciTab) {
        this.tab = tab
    }
    fun openTeach() { teachOpen = true }
    /** A computer_alert tap or `allternit://computers/<id>`: the Computers view opens this console row's detail once the list has loaded. */
    var openComputerId: String? by mutableStateOf(null)
    fun openComputer(id: String) {
        openComputerId = id
        tab = AciTab.Computers
    }
}

/** A suggestion shown until it's dismissed once (iOS ACIOnceNudge, the PWA's key names). */
object AciOnceNudge {
    private fun key(name: String) = "allternit-phone:nudge:$name"
    fun seen(context: Context, name: String): Boolean =
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(key(name), null) == "1"
    fun remember(context: Context, name: String) =
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putString(key(name), "1").apply()
}

internal const val PREFS = "allternit_shell"

/** Search box input → a URL (iOS ACIWebInput): a URL, a bare host, else a Google search. */
object AciWebInput {
    fun resolve(raw: String): String {
        val value = raw.trim()
        val lowered = value.lowercase()
        if (lowered.startsWith("http://") || lowered.startsWith("https://")) return value
        if (value.contains(".") && !value.contains(" ")) return "https://$value"
        return "https://www.google.com/search?q=" + Uri.encode(value)
    }
}

data class AciShortcut(val id: String, val label: String, val url: String, val icon: String?)

/** The landing's shortcut tiles (iOS ACIShortcutsStore), kept on the device under the same key. */
class AciShortcutsStore(context: Context) {
    private val prefs = context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
    val items = mutableStateListOf<AciShortcut>()

    init {
        val saved = prefs.getString(KEY, null)?.let { raw ->
            runCatching {
                val a = JSONArray(raw)
                (0 until a.length()).map { i ->
                    val o = a.getJSONObject(i)
                    AciShortcut(o.getString("id"), o.getString("label"), o.getString("url"), o.optString("icon").takeIf { it.isNotEmpty() })
                }
            }.getOrNull()
        }
        items.addAll(saved ?: DEFAULTS)
    }

    fun add(label: String, url: String, icon: String) {
        items.add(AciShortcut("shortcut-${System.currentTimeMillis()}", label, url, icon.ifEmpty { null }))
        save()
    }

    fun remove(id: String) {
        items.removeAll { it.id == id }
        save()
    }

    private fun save() {
        val a = JSONArray()
        items.forEach { s -> a.put(JSONObject().put("id", s.id).put("label", s.label).put("url", s.url).apply { s.icon?.let { put("icon", it) } }) }
        prefs.edit().putString(KEY, a.toString()).apply()
    }

    private companion object {
        const val KEY = "allternit.aci-browser-shortcuts"
        val DEFAULTS = listOf(
            AciShortcut("default-google", "Google", "https://www.google.com", "🔍"),
            AciShortcut("default-github", "GitHub", "https://github.com", "🐙"),
            AciShortcut("default-hn", "Hacker News", "https://news.ycombinator.com", "📰"),
        )
    }
}

/** One linked phone as the cloud relay lists it (iOS ACIComputersClient.CloudPhone). */
data class CloudPhone(
    val serial: String,
    val model: String,
    val name: String,
    val online: Boolean,
    val callBridgeEnabled: Boolean?,
    val callBridgeBot: String?,
    val route: String?,
    val screen: Boolean?,
    val platform: String?,
    val canTap: Boolean?,
    val keyboardFor: String?,
    val healthyTapRoute: Boolean,
) {
    val isKeyboard: Boolean get() = keyboardFor != null
    val keyboardLabel: String get() = keyboardFor?.trim().orEmpty().let { if (it.isEmpty()) "Keyboard for your iPhone" else "Keyboard for $it" }
    val isAndroidApp: Boolean get() = serial.startsWith("app:") && (route == "android-app" || platform == "android")
    val isAppPhone: Boolean get() = serial.startsWith("app:") && !isAndroidApp && (route == null || route == "iphone-app-store")
    val isLive: Boolean get() = if (isAppPhone) (screen ?: online) && online else online
    val isIphone: Boolean get() = !isAndroidApp && (model == "iPhone" || serial.startsWith("app:"))
    val isStaleWireless: Boolean get() = !online && Regex("""^\d{1,3}(\.\d{1,3}){3}:\d{1,5}$""").matches(serial)
}

/** One device row with the fields the Computers list shows (iOS RelayRuntimeDevice). */
data class AciDevice(val id: String, val name: String, val status: String, val lastSeenAt: String?, val hostname: String?) {
    val isOnline: Boolean get() = status == "online" || status == "busy"
    val displayName: String get() = name.ifEmpty { hostname ?: "Computer" }
}

data class WorkflowSpec(val skillId: String, val title: String?, val hasNetworkTrace: Boolean?)

data class CloudComputerSummary(val id: String, val label: String, val status: String)

/** The cloud + relay calls behind ACI (iOS ACIComputersClient + RuntimeRelayStore's list). */
object AciApi {
    private val config = ApiConfig()
    private val okHttp by lazy { ApiClients.okHttp(bearer = AllternitAuth.tokenProvider) }

    sealed interface Result<out T> {
        data class Ok<T>(val value: T) : Result<T>
        data class Failed(val message: String, val gate: Boolean = false) : Result<Nothing>
    }

    private suspend fun cloud(path: String, authCopy: Boolean = false): Result<JsonElement> {
        val url = config.cloudBaseUrl.newBuilder().addPathSegments(path).build()
        return when (val r = ApiCall.execute(okHttp, ApiJson.default, ApiCall.request("GET", url, null), JsonElement.serializer())) {
            is ApiResult.Ok -> Result.Ok(r.value)
            is ApiResult.Decode -> Result.Ok(JsonNull)
            is ApiResult.HttpError -> when {
                authCopy && (r.status == 401 || r.status == 403) ->
                    Result.Failed(if (r.status == 401) "Sign in to Allternit to see your phones." else "This Allternit account can’t manage phones.")
                r.status == 428 || r.code == "compute_required" -> Result.Failed("No computer is connected.", gate = true)
                r.status in 502..504 -> Result.Failed("This computer isn't responding.")
                else -> Result.Failed(r.message?.takeIf { it.isNotBlank() } ?: "Couldn't load this. Try again.")
            }
            is ApiResult.Network -> Result.Failed("You're offline. Check your connection and try again.")
        }
    }

    /** `GET {cloud}/api/v1/runtime-devices` with last-seen and host for the Computers list. */
    suspend fun devices(): Result<List<AciDevice>> = when (val r = cloud("api/v1/runtime-devices")) {
        is Result.Ok -> Result.Ok(r.value.obj.list("runtimes").mapNotNull { o ->
            AciDevice(o.str("id") ?: return@mapNotNull null, o.str("name").orEmpty(), o.str("status") ?: "offline",
                o.str("lastSeenAt") ?: o.str("last_seen_at"), o.str("hostname") ?: o.str("host"))
        })
        is Result.Failed -> r
    }

    suspend fun phones(): Result<List<CloudPhone>> = when (val r = cloud("api/v1/phones", authCopy = true)) {
        is Result.Ok -> Result.Ok(r.value.obj.list("phones").mapNotNull { o ->
            val bridge = o["callBridge"] as? JsonObject
            CloudPhone(
                serial = o.str("serial") ?: return@mapNotNull null,
                model = o.str("model").orEmpty(),
                name = o.str("name") ?: o.str("serial").orEmpty(),
                online = o.bool("online") == true,
                callBridgeEnabled = bridge.bool("enabled"),
                callBridgeBot = bridge.str("botName"),
                route = o.str("route"),
                screen = o.bool("screen"),
                platform = o.str("platform"),
                canTap = o.bool("canTap"),
                keyboardFor = (o["keyboard_for"] as? JsonPrimitive)?.let { if (it is JsonNull) null else it.content },
                healthyTapRoute = o.list("routes").any { it.bool("healthy") == true && it.str("route") != "official" },
            )
        }.filter { !it.isStaleWireless })
        is Result.Failed -> r
    }

    /** `GET /v1/browser-skills` on the picked computer, through the relay. */
    suspend fun workflowSpecs(backend: ChatBackend): Result<List<WorkflowSpec>> = when (val r = backend.runtimeJson("GET", "/v1/browser-skills")) {
        is RuntimeCall.Ok -> Result.Ok(r.body.obj.list("specs").mapNotNull { o ->
            WorkflowSpec(o.str("skill_id") ?: return@mapNotNull null, o.str("title"), o.bool("hasNetworkTrace"))
        })
        RuntimeCall.NoComputer -> Result.Failed("No computer is connected.", gate = true)
        is RuntimeCall.Failed -> Result.Failed(r.message)
        is RuntimeCall.Http -> Result.Failed(r.message?.takeIf { it.isNotBlank() } ?: if (r.status in 502..504) "This computer isn't responding." else "Could not load workflow specs.")
    }

    /**
     * `GET {cloud}/api/v1/computers/console` (Computers console contract). 404/501 means the backend
     * is not live for this account yet: [CONSOLE_UNAVAILABLE].
     */
    internal suspend fun console(): Result<List<ConsoleComputer>> {
        val url = config.cloudBaseUrl.newBuilder().addPathSegments("api/v1/computers/console").build()
        return when (val r = ApiCall.execute(okHttp, ApiJson.default, ApiCall.request("GET", url, null), JsonElement.serializer())) {
            is ApiResult.Ok -> Result.Ok(r.value.listUnder("computers").mapNotNull { ConsoleComputer.parse(it) })
            is ApiResult.Decode -> Result.Failed("Couldn't read the computers list. Try again.")
            is ApiResult.HttpError -> when {
                r.status == 404 || r.status == 501 -> Result.Failed(CONSOLE_UNAVAILABLE)
                r.status == 401 -> Result.Failed("Sign in to Allternit to see your computers.")
                else -> Result.Failed(r.message?.takeIf { it.isNotBlank() } ?: "Couldn't load your computers. Try again.")
            }
            is ApiResult.Network -> Result.Failed("You're offline. Check your connection and try again.")
        }
    }

    /** Account flag `computers_console`: `GET {cloud}/api/v1/me/flags`, on when the key is present and not null (iOS AccountFlagsClient). */
    suspend fun consoleFlag(): Boolean {
        val url = config.cloudBaseUrl.newBuilder().addPathSegments("api/v1/me/flags").build()
        val r = ApiCall.execute(okHttp, ApiJson.default, ApiCall.request("GET", url, null), JsonElement.serializer())
        return ((r as? ApiResult.Ok)?.value as? JsonObject)?.get("computers_console").let { it != null && it !is JsonNull }
    }

    /**
     * One read of `GET {cloud}/api/v1/computers/:id/<tail>` for the console detail (the host routes and metrics).
     * 409 = asleep (the body may carry `lastSample`; the computer is never woken and nothing is retried),
     * 404/501 = not available yet. Mirrors iOS ACIComputersClient.check().
     */
    internal suspend fun consoleRead(id: String, tail: String, query: Map<String, String> = emptyMap()): HostRead<JsonElement> {
        val (status, body) = rawCall(ApiCall.request("GET", consoleUrl(id, tail, query), null))
            ?: return HostRead.Failed("You're offline. Check your connection and try again.")
        return when {
            status in 200..299 -> runCatching { ApiJson.default.parseToJsonElement(body) }
                .fold({ HostRead.Ok(it) }, { HostRead.Failed("Couldn't read this. Try again.") })
            status == 409 && isAsleepBody(body) -> HostRead.Asleep(runCatching { ConsoleSnapshot.parse((ApiJson.default.parseToJsonElement(body) as? JsonObject).child("lastSample")) }.getOrNull())
            status == 404 || status == 405 || status == 501 -> HostRead.Unavailable
            status == 401 -> HostRead.Failed("Sign in to Allternit to see this computer.")
            else -> HostRead.Failed(ApiCall.httpError(status, body).message?.takeIf { it.isNotBlank() } ?: "Couldn't load this. Try again.")
        }
    }

    /**
     * `GET {cloud}/api/v1/computers/snippets`: `{snippets:[{id,name,command,description?}]}` or a bare array (iOS #2770).
     * 404/405/501 = [HostRead.Unavailable]. Not tied to one computer, so there is no asleep case.
     */
    internal suspend fun consoleSnippets(): HostRead<List<ConsoleSnippet>> {
        val url = config.cloudBaseUrl.newBuilder().addPathSegments("api/v1/computers/snippets").build()
        val (status, body) = rawCall(ApiCall.request("GET", url, null)) ?: return HostRead.Failed("You're offline. Check your connection and try again.")
        return when {
            status in 200..299 -> runCatching { ApiJson.default.parseToJsonElement(body) }
                .fold({ HostRead.Ok(it.rows("snippets").mapNotNull(ConsoleSnippet::parse)) }, { HostRead.Failed("Couldn't read the snippets. Try again.") })
            status == 404 || status == 405 || status == 501 -> HostRead.Unavailable
            status == 401 -> HostRead.Failed("Sign in to Allternit to see your snippets.")
            else -> HostRead.Failed(ApiCall.httpError(status, body).message?.takeIf { it.isNotBlank() } ?: "Couldn't load your snippets. Try again.")
        }
    }

    /** A 409 is "asleep" only when the body says error:"asleep" (or says nothing); another reason is a plain error. */
    private fun isAsleepBody(body: String): Boolean {
        val error = runCatching { ((ApiJson.default.parseToJsonElement(body) as? JsonObject)?.get("error") as? JsonPrimitive)?.content }.getOrNull()
        return error == null || error == "asleep"
    }

    private suspend fun rawCall(request: okhttp3.Request): Pair<Int, String>? = suspendCancellableCoroutine { cont ->
        val call = okHttp.newCall(request)
        cont.invokeOnCancellation { call.cancel() }
        call.enqueue(object : Callback {
            override fun onFailure(call: Call, e: IOException) { if (cont.isActive) cont.resume(null) }
            override fun onResponse(call: Call, response: Response) {
                val out = try { response.use { it.code to it.body?.string().orEmpty() } } catch (e: IOException) { null }
                if (cont.isActive) cont.resume(out)
            }
        })
    }

    /** One write to `POST|DELETE api/v1/computers/:id/<tail>`. Never wakes a computer; nothing is retried. */
    internal suspend fun consoleWrite(method: String, id: String, tail: String, body: JsonObject?): ConsoleWrite =
        write(ApiCall.request(method, consoleUrl(id, tail, emptyMap()), writeBody(method, body)))

    /** `PATCH|DELETE api/v1/computers/paired/:id` (rename / unpair). */
    internal suspend fun consolePairedWrite(method: String, id: String, body: JsonObject?): ConsoleWrite =
        write(ApiCall.request(method, config.cloudBaseUrl.newBuilder().addPathSegments("api/v1/computers/paired").addPathSegment(id).build(), writeBody(method, body)))

    private fun writeBody(method: String, body: JsonObject?): String? = body?.toString() ?: if (method == "DELETE" || method == "GET") null else "{}"

    private suspend fun write(request: okhttp3.Request): ConsoleWrite {
        val (status, body) = rawCall(request) ?: return ConsoleWrite.Failed("You're offline. Nothing was changed.")
        val json = runCatching { ApiJson.default.parseToJsonElement(body) }.getOrNull()
        val error = ((json as? JsonObject)?.get("error") as? JsonPrimitive)?.content
        return when {
            status in 200..299 -> ConsoleWrite.Ok(json)
            status == 403 && error == "needs_elevation" -> ConsoleWrite.NeedsElevation((json as? JsonObject).str("message").orEmpty())
            status == 409 && (error == null || error == "asleep") -> ConsoleWrite.Asleep
            // Restore answers with its own plain message (501 while the server gate is off, 404 backup_not_found): show it.
            (status == 404 || status == 501) && request.url.encodedPath.endsWith("/backups/restore") && !(json as? JsonObject).str("message").isNullOrBlank() ->
                ConsoleWrite.Failed((json as JsonObject).str("message").orEmpty())
            status == 404 || status == 405 || status == 501 -> ConsoleWrite.Unavailable
            status == 401 -> ConsoleWrite.Failed("Sign in to Allternit to do this. Nothing was changed.")
            else -> ConsoleWrite.Failed(ApiCall.httpError(status, body).message?.takeIf { it.isNotBlank() } ?: "That didn't work. Nothing was changed.")
        }
    }

    /**
     * Live log lines for one unit over SSE (`GET host/logs/stream?unit=`, one LogLine JSON per `data:` line).
     * The flow ends and the call is cancelled when the collector goes away; lines are never stored here.
     * Failures arrive as [ConsoleStreamException].
     */
    internal fun consoleLogStream(id: String, unit: String): Flow<ConsoleLogLine> = callbackFlow {
        val client = okHttp.newBuilder().readTimeout(0, TimeUnit.MILLISECONDS).callTimeout(0, TimeUnit.MILLISECONDS).build()
        val call = client.newCall(ApiCall.request("GET", consoleUrl(id, "host/logs/stream", mapOf("unit" to unit)), null, accept = "text/event-stream"))
        call.enqueue(object : Callback {
            override fun onFailure(call: Call, e: IOException) {
                close(if (call.isCanceled()) null else ConsoleStreamException(ConsoleStreamException.Kind.Network, "The log stream stopped."))
            }
            override fun onResponse(call: Call, response: Response) {
                try {
                    response.use { r ->
                        if (!r.isSuccessful) {
                            val body = r.body?.string().orEmpty()
                            val kind = when (r.code) {
                                409 -> ConsoleStreamException.Kind.Asleep
                                404, 501 -> ConsoleStreamException.Kind.Unavailable
                                else -> ConsoleStreamException.Kind.Network
                            }
                            close(ConsoleStreamException(kind, ApiCall.httpError(r.code, body).message?.takeIf { it.isNotBlank() } ?: "The log stream stopped."))
                            return
                        }
                        val source = r.body?.source() ?: run { close(); return }
                        val parser = SseParser()
                        val buf = ByteArray(4096)
                        while (true) {
                            val n = source.read(buf)
                            if (n < 0) break
                            for (event in parser.feed(buf.copyOf(n))) ConsoleLogLine.parse(event.data)?.let { trySend(it) }
                        }
                        close()
                    }
                } catch (e: IOException) {
                    close(if (call.isCanceled()) null else ConsoleStreamException(ConsoleStreamException.Kind.Network, "The log stream stopped."))
                }
            }
        })
        awaitClose { call.cancel() }
    }

    private fun consoleUrl(id: String, tail: String, query: Map<String, String>) =
        config.cloudBaseUrl.newBuilder().addPathSegments("api/v1/computers").addPathSegment(id).addPathSegments(tail)
            .apply { query.forEach { (k, v) -> addQueryParameter(k, v) } }.build()

    /** The account's cloud computers (`provisioned-instances`), for Restore points and backups. */
    suspend fun cloudComputers(): Result<List<CloudComputerSummary>> = when (val r = LibraryNet.get("provisioned-instances")) {
        is ApiResult.Ok -> Result.Ok(r.value.listUnder("instances").mapNotNull { o ->
            val status = o.str("status").orEmpty()
            if (status == "deleted") return@mapNotNull null
            CloudComputerSummary(o.str("id") ?: return@mapNotNull null, o.str("name") ?: o.str("label") ?: "Cloud computer", status)
        })
        else -> Result.Failed("Couldn’t load cloud computers")
    }
}

/** One parsed frame of `GET /api/aci/stream/:id` (iOS ACIEvent). */
sealed interface AciEvent {
    data class State(
        val status: String?, val lastMessage: String?, val adapterId: String?, val stepIndex: Int?, val totalSteps: Int?,
        /** Present-but-null clears the action; absent leaves it (the web's `'currentAction' in s`). */
        val hasCurrentAction: Boolean, val currentAction: AciAction?,
    ) : AciEvent
    data class Screenshot(val base64: String) : AciEvent
    data class Trace(val message: String?, val adapterId: String?) : AciEvent
    data object Done : AciEvent

    companion object {
        fun parse(raw: String): AciEvent? {
            val o = runCatching { ApiJson.default.parseToJsonElement(raw) }.getOrNull() as? JsonObject ?: return null
            val data = o["data"] as? JsonObject
            return when (o.str("type")) {
                "state" -> data?.let { d ->
                    val action = d["currentAction"] as? JsonObject
                    State(d.str("status"), d.str("lastMessage"), d.str("adapterId"), d.int("stepIndex"), d.int("totalSteps"),
                        d.containsKey("currentAction"), action?.let { a ->
                            AciAction(a.str("type"), a.str("label"), a.str("selector"), a.dbl("x"), a.dbl("y"), a.int("risk"), a.dbl("width"), a.dbl("height"))
                        })
                }
                "screenshot" -> data.str("screenshot")?.let { Screenshot(it) }
                "trace" -> Trace(data.str("message"), data.str("adapterId"))
                "done" -> Done
                else -> null
            }
        }

        private fun JsonObject.int(k: String) = (this[k] as? JsonPrimitive)?.intOrNull
        private fun JsonObject.dbl(k: String) = (this[k] as? JsonPrimitive)?.doubleOrNull
    }
}

/** The agent's in-flight action; the highlight rect is in screenshot pixels (40×20 when unsized, like the web). */
data class AciAction(val type: String?, val label: String?, val selector: String?, val x: Double?, val y: Double?, val risk: Int?, val width: Double?, val height: Double?)

/** Run lifecycle status (iOS ACIRunStatus). */
enum class AciRunStatus {
    Idle, Running, WaitingApproval, Blocked, Done, Error, Unknown;

    companion object {
        fun of(raw: String?): AciRunStatus = when (raw) {
            "Idle" -> Idle
            "Running" -> Running
            "WaitingApproval" -> WaitingApproval
            "Blocked" -> Blocked
            "Done" -> Done
            "Error" -> Error
            else -> Unknown
        }
    }
}

/**
 * The ACI run protocol on the user's own allternit-api, through the relay (iOS ACIAgentClient):
 * `POST /api/aci/run`, `GET /api/aci/stream/:id` (SSE), `POST /api/aci/stop/:id`, `POST /api/aci/approve/:id`.
 */
class AciAgentClient(private val backend: ChatBackend) {
    data class Run(val sessionId: String, val adapterId: String?)

    class NoComputer : Exception("No computer is connected. Pair one in Computers, or wake your cloud computer.")

    /** Defaults are the web store's: Sonnet 4.6, no allowlist, links stay in the agent viewport, auto-verify on. */
    suspend fun startRun(goal: String): Run {
        val body = buildJsonObject {
            put("goal", goal)
            put("model", "anthropic/claude-sonnet-4.6")
            put("allowedSites", buildJsonArray { })
            put("openLinksInBrowser", false)
            put("autoVerify", true)
            put("sessionPersistence", "dont-keep")
        }.toString()
        return when (val r = backend.runtimeJson("POST", "/api/aci/run", body)) {
            is RuntimeCall.Ok -> {
                val o = r.body.obj
                Run(o.str("sessionId") ?: throw IllegalStateException("This app needs an update to start a run."), o.str("adapterId"))
            }
            RuntimeCall.NoComputer -> throw NoComputer()
            is RuntimeCall.Failed -> throw IllegalStateException(r.message)
            is RuntimeCall.Http -> throw IllegalStateException(r.message?.takeIf { it.isNotBlank() } ?: "The computer couldn't start the run (HTTP ${r.status}).")
        }
    }

    /** The run's events; finishes on `done`. Undecodable frames are skipped like the web parser. */
    fun stream(runId: String): Flow<AciEvent> = flow {
        try {
            backend.runtimeEvents("/api/aci/stream/" + Uri.encode(runId)).collect { e ->
                if (e.data.isEmpty()) return@collect
                val event = AciEvent.parse(e.data) ?: return@collect
                emit(event)
                if (event is AciEvent.Done) throw DoneSignal()
            }
        } catch (_: DoneSignal) {
        } catch (_: NoComputerException) {
            throw NoComputer()
        }
    }

    private class DoneSignal : Exception()

    suspend fun stop(runId: String) {
        backend.runtimeJson("POST", "/api/aci/stop/" + Uri.encode(runId), "{}")
    }

    suspend fun approve(runId: String, deny: Boolean) {
        backend.runtimeJson("POST", "/api/aci/approve/" + Uri.encode(runId) + if (deny) "?deny=true" else "", "{}")
    }
}
