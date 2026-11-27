package com.allternit.feature.chat

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.allternit.core.api.ApiCall
import com.allternit.core.api.ApiJson
import com.allternit.core.api.ApiResult
import java.util.UUID
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.put

/*
 * Start anywhere (handoffs/start-anywhere-2026-11-27, steps 19–20): start a real session on one of the
 * user's computers from the phone. Contract: docs/contracts/session-start (fixtures in test resources).
 * Device routes ride the runtime relay to the CHOSEN computer (like the Code terminal's `target`), never
 * the auto-picked one. Same shapes as web `src/lib/session-start/types.ts` and iOS SessionStart.swift.
 */

/** The four permission mode ids (the app's ids, PLAN delta 2). Labels match web PERMISSION_SHORT_LABEL. */
enum class StartMode(val id: String, val label: String) {
    Auto("auto", "Auto"),
    Default("default", "Ask"),
    Plan("plan", "Read-only"),
    Bypass("bypass", "Bypass");

    companion object {
        fun of(id: String?): StartMode? = entries.firstOrNull { it.id == id }
    }
}

data class StartGit(val remote: String, val branch: String, val head: String? = null, val dirty: Boolean? = null, val ahead: Int? = null)

data class StartFolder(val path: String, val name: String, val lastUsedAt: String?, val git: StartGit? = null)

data class StartModeSet(val default: StartMode, val supported: List<StartMode>)

data class StartDevice(val runtimeId: String, val name: String, val os: String?, val remoteStart: Boolean)

/** `GET /api/v1/session-start/options` on a computer. */
data class StartOptions(
    val device: StartDevice,
    val folders: List<StartFolder>,
    val modes: Map<String, StartModeSet>,
    val defaultRuntime: String,
)

/** One computer's folder snapshot from cloud-api (`GET /api/v1/start-options`), for asleep offers. */
data class StartSnapshot(
    val runtimeId: String,
    val name: String,
    val kind: String?,
    val remoteStart: Boolean,
    val updatedAt: String?,
    val folders: List<StartFolder>,
    val modes: Map<String, StartModeSet>,
    val defaultRuntime: String?,
)

/** Where the session opens: a path on that computer, or a git repo (a cloud computer clones it). */
sealed interface StartFolderRef {
    data class Path(val path: String) : StartFolderRef
    data class Git(val remote: String, val branch: String, val head: String? = null) : StartFolderRef

    val label: String get() = when (this) {
        is Path -> path.trimEnd('/').substringAfterLast('/').ifEmpty { path }
        is Git -> remote.trimEnd('/').removeSuffix(".git").substringAfterLast('/')
    }

    fun toJson(): JsonObject = when (this) {
        is Path -> buildJsonObject { put("path", path) }
        is Git -> buildJsonObject {
            put("git", buildJsonObject { put("remote", remote); put("branch", branch); head?.let { put("head", it) } })
        }
    }
}

data class StartOrigin(val surface: String, val deviceName: String, val parentSessionId: String? = null, val depth: Int? = null)

/** `POST /api/v1/session-start` body. */
data class StartRequest(
    val folder: StartFolderRef,
    val runtime: String,
    val mode: StartMode? = null,
    val confirmBypass: Boolean? = null,
    val prompt: String? = null,
    val origin: StartOrigin,
) {
    fun toJson(): JsonObject = buildJsonObject {
        put("folder", folder.toJson())
        put("runtime", runtime)
        mode?.let { put("mode", it.id) }
        confirmBypass?.let { put("confirmBypass", it) }
        prompt?.let { put("prompt", it) }
        put("origin", buildJsonObject {
            put("surface", origin.surface)
            put("deviceName", origin.deviceName)
            origin.parentSessionId?.let { put("parentSessionId", it) }
            origin.depth?.let { put("depth", it) }
        })
    }
}

sealed interface StartResponse {
    /** A gizzi engine session: the chat thread drives it (messages, approvals, Stop). */
    data class Engine(val sessionId: String, val directory: String) : StartResponse
    /** A CLI in a pane terminal: the thread hosts the Code terminal composer. */
    data class Terminal(val terminalId: String, val cwd: String, val agent: String) : StartResponse
}

/** A refusal from the contract's error list, worded for the person (message + what to do). */
data class StartError(val status: Int, val error: String, val message: String, val action: String) {
    val sentence: String get() = if (action.isBlank()) message else "$message $action"
}

sealed interface StartResult<out T> {
    data class Ok<T>(val value: T) : StartResult<T>
    data class Refused(val error: StartError) : StartResult<Nothing>
    /** The computer did not answer (asleep, offline) or no computer at all. */
    data class Unreachable(val message: String) : StartResult<Nothing>
}

/** Where a chat thread runs, kept in the chat session metadata under [KEY] (web binding.ts). */
data class StartBinding(
    val runtimeId: String,
    val deviceName: String,
    val folder: String,
    val runtime: String,
    val mode: StartMode,
    val kind: String,
    val sessionId: String? = null,
    val terminalId: String? = null,
    val origin: StartOrigin,
) {
    val isTerminal: Boolean get() = kind == "terminal"

    fun toJson(): JsonObject = buildJsonObject {
        put("runtimeId", runtimeId); put("deviceName", deviceName); put("folder", folder)
        put("runtime", runtime); put("mode", mode.id); put("kind", kind)
        sessionId?.let { put("sessionId", it) }
        terminalId?.let { put("terminalId", it) }
        put("origin", buildJsonObject { put("surface", origin.surface); put("deviceName", origin.deviceName) })
    }

    companion object {
        const val KEY = "startBinding"

        /** The binding in a session's metadata, or null when absent or malformed (web readBinding). */
        fun read(metadata: JsonElement?): StartBinding? {
            val raw = (metadata as? JsonObject)?.get(KEY) as? JsonObject ?: return null
            val origin = raw["origin"] as? JsonObject ?: return null
            val kind = raw.str("kind")?.takeIf { it == "engine" || it == "terminal" } ?: return null
            return StartBinding(
                runtimeId = raw.str("runtimeId") ?: return null,
                deviceName = raw.str("deviceName") ?: return null,
                folder = raw.str("folder") ?: return null,
                runtime = raw.str("runtime") ?: return null,
                mode = StartMode.of(raw.str("mode")) ?: return null,
                kind = kind,
                sessionId = raw.str("sessionId"),
                terminalId = raw.str("terminalId"),
                origin = StartOrigin(origin.str("surface") ?: return null, origin.str("deviceName") ?: return null),
            )
        }
    }
}

/** Decoders for the contract shapes (hand-written, like the rest of the chat's runtime JSON). */
object StartDecode {
    fun git(o: JsonObject?): StartGit? {
        o ?: return null
        return StartGit(
            remote = o.str("remote") ?: return null,
            branch = o.str("branch") ?: return null,
            head = o.str("head"),
            dirty = o.bool("dirty"),
            ahead = (o["ahead"] as? JsonPrimitive)?.intOrNull,
        )
    }

    fun folder(o: JsonObject): StartFolder? {
        val path = o.str("path") ?: return null
        return StartFolder(path, o.str("name") ?: path.trimEnd('/').substringAfterLast('/'), o.str("lastUsedAt"), git(o["git"] as? JsonObject))
    }

    fun modes(o: JsonObject?): Map<String, StartModeSet> = o.orEmptyObject().mapNotNull { (id, el) ->
        val m = el as? JsonObject ?: return@mapNotNull null
        val def = StartMode.of(m.str("default")) ?: return@mapNotNull null
        id to StartModeSet(def, m.arr("supported").mapNotNull { StartMode.of((it as? JsonPrimitive)?.content) })
    }.toMap()

    fun options(o: JsonObject): StartOptions? {
        val d = o["device"] as? JsonObject ?: return null
        return StartOptions(
            device = StartDevice(d.str("runtimeId") ?: return null, d.str("name").orEmpty(), d.str("os"), d.bool("remoteStart") == true),
            folders = o.arr("folders").mapNotNull { (it as? JsonObject)?.let(::folder) },
            modes = modes(o["modes"] as? JsonObject),
            defaultRuntime = o.str("defaultRuntime") ?: "allternit",
        )
    }

    fun snapshots(o: JsonObject): List<StartSnapshot> = o.arr("devices").mapNotNull { el ->
        val d = el as? JsonObject ?: return@mapNotNull null
        StartSnapshot(
            runtimeId = d.str("runtimeId") ?: return@mapNotNull null,
            name = d.str("name").orEmpty(),
            kind = d.str("kind"),
            remoteStart = d.bool("remoteStart") == true,
            updatedAt = d.str("updatedAt"),
            folders = d.arr("folders").mapNotNull { (it as? JsonObject)?.let(::folder) },
            modes = modes(d["modes"] as? JsonObject),
            defaultRuntime = d.str("defaultRuntime"),
        )
    }

    fun response(o: JsonObject): StartResponse? = when (o.str("kind")) {
        "engine" -> StartResponse.Engine(o.str("sessionId") ?: return null, o.str("directory").orEmpty())
        "terminal" -> StartResponse.Terminal(o.str("terminalId") ?: return null, o.str("cwd").orEmpty(), o.str("agent").orEmpty())
        else -> null
    }

    /** A refusal: the body's own words when it has them, else the contract's (errors.json). */
    fun error(status: Int, code: String?, message: String?): StartError {
        val known = ERRORS[code]
        return StartError(
            status = status,
            error = code ?: "unknown",
            message = message?.takeIf { it.isNotBlank() && it != code } ?: known?.first ?: "The computer couldn't start the session.",
            action = known?.second ?: "Try again.",
        )
    }

    /** docs/contracts/session-start/errors.json, same wording. */
    val ERRORS: Map<String, Pair<String, String>> = mapOf(
        "remote_start_off" to ("Remote start is turned off on this computer." to "Turn on Remote start in Settings on that computer, or start the session there."),
        "mode_not_allowed" to ("This permission mode is higher than the one you are allowed to grant." to "Pick a mode at or below your own, or confirm bypass on the computer."),
        "depth_exceeded" to ("Sessions started from sessions can only go three levels deep." to "Start the work from a top-level session instead."),
        "folder_missing" to ("That folder no longer exists on this computer." to "Pick another folder from the list."),
        "runtime_missing" to ("That coding tool is not installed on this computer." to "Pick another tool or install it on that computer."),
        "runtime_signed_out" to ("That coding tool is signed out on this computer." to "Sign in to the tool on that computer, then try again."),
        "rate_limited" to ("This computer has started too many sessions in the last minute." to "Wait a minute, then try again."),
        "clone_failed" to ("The cloud computer could not copy the repository." to "Check that the repository is reachable, then try again."),
    )

    private fun JsonObject?.orEmptyObject(): JsonObject = this ?: JsonObject(emptyMap())
}

/** The session-start calls. Every device call names its computer ([runtimeId]); none uses the auto pick. */
object SessionStartClient {
    suspend fun options(backend: ChatBackend, runtimeId: String): StartResult<StartOptions> =
        when (val r = backend.runtimeJson("GET", "/api/v1/session-start/options", null, runtimeId)) {
            is RuntimeCall.Ok -> (r.body as? JsonObject)?.let(StartDecode::options)?.let { StartResult.Ok(it) }
                ?: StartResult.Unreachable("Couldn't read that computer's folders.")
            is RuntimeCall.Http -> StartResult.Refused(StartDecode.error(r.status, r.code, r.message))
            RuntimeCall.NoComputer -> StartResult.Unreachable(ChatErrors.UNREACHABLE)
            is RuntimeCall.Failed -> StartResult.Unreachable(r.message)
        }

    /**
     * `POST /api/v1/session-start` with an Idempotency-Key (the relay keeps the header, #3181). A retry of
     * the same send passes the same [idempotencyKey], so the computer never starts it twice.
     */
    suspend fun start(backend: ChatBackend, runtimeId: String, request: StartRequest, idempotencyKey: String = UUID.randomUUID().toString()): StartResult<StartResponse> =
        when (val r = backend.runtimeJson("POST", "/api/v1/session-start", request.toJson().toString(), runtimeId, headers = mapOf("Idempotency-Key" to idempotencyKey))) {
            is RuntimeCall.Ok -> (r.body as? JsonObject)?.let(StartDecode::response)?.let { StartResult.Ok(it) }
                ?: StartResult.Unreachable("The computer answered, but not with a session.")
            is RuntimeCall.Http -> StartResult.Refused(StartDecode.error(r.status, r.code, r.message))
            RuntimeCall.NoComputer -> StartResult.Unreachable(ChatErrors.UNREACHABLE)
            is RuntimeCall.Failed -> StartResult.Unreachable(r.message)
        }

    /** cloud-api `GET /api/v1/start-options`: every computer's last folder snapshot (asleep offers). Empty on failure. */
    suspend fun snapshots(backend: ChatBackend): List<StartSnapshot> {
        val url = backend.cloudBaseUrl.newBuilder().addPathSegments("api/v1/start-options").build()
        val r = ApiCall.execute(backend.authedHttp, ApiJson.default, ApiCall.request("GET", url, null), JsonObject.serializer())
        return (r as? ApiResult.Ok)?.value?.let(StartDecode::snapshots).orEmpty()
    }
}

/**
 * The computer, folder and permission picked before a chat exists (web pending.ts): written by the + sheet
 * and the chip's Computer row, read by the first send. Compose state, so the chip and the sheet redraw.
 * [backend] is set by the shell when it makes the chat backend, so the chip (feature/bots) can list devices.
 */
object PendingStart {
    var backend: ChatBackend? = null
    var runtimeId by mutableStateOf<String?>(null)
    var deviceName by mutableStateOf<String?>(null)
    /** True when the picked computer was asleep at pick time ("Pick <name> anyway"). */
    var asleep by mutableStateOf(false)
    var folder by mutableStateOf<StartFolderRef?>(null)
    /** Null = the computer's default for the runtime. Applies to this chat only. */
    var mode by mutableStateOf<StartMode?>(null)
    /** The picked computer's default mode for the current runtime (from its options), for "<mode> · default". */
    var defaultMode by mutableStateOf<StartMode?>(null)
    var options by mutableStateOf<StartOptions?>(null)
    /** The open thread's binding (set by the chat page), so the chip keeps its computer segment there. */
    var bound by mutableStateOf<StartBinding?>(null)

    fun pick(device: RelayDevice?, asleep: Boolean = false) {
        if (device?.id != runtimeId) { folder = null; options = null; defaultMode = null }
        runtimeId = device?.id
        deviceName = device?.name?.ifEmpty { "Computer" }
        this.asleep = asleep
    }

    /** After the first send started a session: the thread is bound, the next new chat starts clean. */
    fun clear() {
        runtimeId = null; deviceName = null; asleep = false; folder = null; mode = null; defaultMode = null; options = null
    }
}
