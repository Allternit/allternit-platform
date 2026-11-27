package com.allternit.feature.code

import com.allternit.core.design.FriendlyErrorMessage
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.bool
import com.allternit.feature.artifacts.child
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.list
import com.allternit.feature.artifacts.long
import com.allternit.feature.artifacts.str
import com.allternit.feature.chat.ChatBackend
import com.allternit.feature.chat.RuntimeCall
import com.allternit.core.api.ApiResult
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/*
 * Code mode data (iOS CodeClients, FileClient, SessionDiffClient, DevPreviewClient, PtyClient and
 * the Factory pane client). The routes live on the user's computer (gizzi-code `v1/...`, Factory
 * `api/factory/...`) and ride the relay through ChatBackend.runtimeJson, like iOS APIClient with a
 * relay placeholder base. A phone with no computer gets [CodeResult.NoComputer], never a raw error.
 */

sealed interface CodeResult<out T> {
    data class Ok<T>(val value: T) : CodeResult<T>
    /** No computer on the account (428 / compute_required): an empty state, not an error. */
    data object NoComputer : CodeResult<Nothing>
    data class Failed(val message: String, val status: Int? = null) : CodeResult<Nothing>
}

internal inline fun <T, R> CodeResult<T>.map(f: (T) -> R): CodeResult<R> = when (this) {
    is CodeResult.Ok -> CodeResult.Ok(f(value))
    CodeResult.NoComputer -> CodeResult.NoComputer
    is CodeResult.Failed -> this
}

/** One runtime call as a [CodeResult]; [doing] words the failure ("load the files"). */
internal suspend fun ChatBackend.codeCall(method: String, path: String, body: String? = null, doing: String, target: String? = null): CodeResult<JsonElement> =
    when (val r = runtimeJson(method, path, body, target)) {
        is RuntimeCall.Ok -> CodeResult.Ok(r.body)
        RuntimeCall.NoComputer -> CodeResult.NoComputer
        is RuntimeCall.Http -> CodeResult.Failed(FriendlyErrorMessage.from(r.message ?: r.code ?: "HTTP ${r.status}", doing), r.status)
        is RuntimeCall.Failed -> CodeResult.Failed(r.message)
    }

internal fun encode(segment: String): String = LibraryNet.escape(segment)

// MARK: - Records

/** A row of `GET v1/file/tree` (iOS FileNode). */
data class FileNode(val name: String, val path: String, val absolute: String, val isDirectory: Boolean, val ignored: Boolean)

/** `GET v1/file/read` (iOS FileReadResult): binary files carry base64 in [content]. */
data class FileRead(val binary: Boolean, val content: String, val diff: String?, val encoding: String?, val mimeType: String?)

/** One file's accumulated change in a session (iOS FileDiff): full before/after text. */
data class FileDiff(val file: String, val before: String, val after: String, val additions: Int, val deletions: Int, val status: String?)

data class DevPort(val port: Int, val command: String?)

data class CodeFolder(val directory: String, val branch: String?) {
    /** iOS CodeFolderInfo.name: the last path component. */
    val name: String get() = directory.trimEnd('/').substringAfterLast('/').ifEmpty { directory }
}

/** A Terminal bot from `GET /api/factory/agents` (iOS TerminalBot). */
data class TerminalBot(
    val id: String,
    val slug: String,
    val name: String,
    val address: String,
    val state: String,
    val remoteMachine: String?,
) {
    val initial: String get() = (name.ifEmpty { slug }).trim().take(1).uppercase().ifEmpty { "?" }
    val stateLabel: String
        get() = when (state) {
            "idle" -> "Idle"
            "working" -> "Working"
            "blocked" -> "Blocked"
            "needs_you" -> "Needs you"
            "done" -> "Done"
            "failed" -> "Failed"
            "offline" -> "Offline"
            else -> state
        }
}

data class TerminalBots(val bots: List<TerminalBot>, val engineRunning: Boolean, val engineError: String?)

/** iOS CodePermissionMode: the session's permission rules. */
enum class CodePermissionMode(val raw: String, val label: String, val blurb: String) {
    Manual("DEFAULT", "Manual", "Ask before running commands and edits"),
    Auto("AUTO", "Auto-accept edits", "Run commands and apply file edits without asking"),
    Plan("PLAN", "Plan mode", "Explore the codebase and propose a plan before changes"),
    Safe("SAFE", "Read only", "Inspect files without changing anything");

    fun rules(): JsonArray {
        val read = listOf("read", "glob", "grep", "list", "websearch", "codesearch", "question", "todoread", "lsp")
        fun rule(permission: String, action: String) = buildJsonObject {
            put("permission", permission); put("pattern", "*"); put("action", action)
        }
        return buildJsonArray {
            when (this@CodePermissionMode) {
                Manual -> Unit
                Auto -> {
                    read.forEach { add(rule(it, "allow")) }
                    listOf("edit", "write", "patch", "multiedit").forEach { add(rule(it, "allow")) }
                }
                Plan, Safe -> {
                    add(rule("*", "deny"))
                    read.forEach { add(rule(it, "allow")) }
                }
            }
        }
    }
}

// MARK: - Clients

internal object CodeApi {
    /** iOS CodeFolderClient.load: `v1/path` (+ `v1/vcs` for the branch). */
    suspend fun folder(b: ChatBackend): CodeResult<CodeFolder> {
        val path = b.codeCall("GET", "/v1/path", doing = "read the working folder")
        val dir = when (path) {
            is CodeResult.Ok -> (path.value as? JsonObject).str("directory") ?: return CodeResult.Failed("Your computer didn't say which folder it is working in.")
            CodeResult.NoComputer -> return CodeResult.NoComputer
            is CodeResult.Failed -> return path
        }
        val branch = ((b.codeCall("GET", "/v1/vcs", doing = "read the branch") as? CodeResult.Ok<JsonElement>)?.value as? JsonObject).str("branch")
        return CodeResult.Ok(CodeFolder(dir, branch))
    }

    suspend fun tree(b: ChatBackend, path: String?): CodeResult<List<FileNode>> {
        val route = "/v1/file/tree" + if (path.isNullOrEmpty()) "" else "?path=${encode(path)}"
        return b.codeCall("GET", route, doing = "load the files").map { body ->
            ((body as? JsonArray) ?: JsonArray(emptyList())).mapNotNull { it as? JsonObject }.map { o ->
                FileNode(o.str("name").orEmpty(), o.str("path").orEmpty(), o.str("absolute").orEmpty(), o.str("type") == "directory", o.bool("ignored") == true)
            }
        }
    }

    suspend fun read(b: ChatBackend, path: String): CodeResult<FileRead> =
        b.codeCall("GET", "/v1/file/read?path=${encode(path)}", doing = "open the file").map { body ->
            val o = body as? JsonObject
            FileRead(o.str("type") == "binary", o.str("content").orEmpty(), o.str("diff"), o.str("encoding"), o.str("mimeType"))
        }

    /** Save from the editor: `POST /api/v1/files/write` (the same write the workspace file editor uses). */
    suspend fun write(b: ChatBackend, absolutePath: String, content: String): CodeResult<Unit> {
        val body = buildJsonObject { put("path", absolutePath); put("content", content) }.toString()
        return b.codeCall("POST", "/api/v1/files/write", body, "save the file").map { }
    }

    suspend fun sessionDiff(b: ChatBackend, sessionId: String): CodeResult<List<FileDiff>> =
        b.codeCall("GET", "/v1/session/${encode(sessionId)}/diff", doing = "load the changes").map { body ->
            ((body as? JsonArray) ?: JsonArray(emptyList())).mapNotNull { it as? JsonObject }.map { o ->
                FileDiff(
                    o.str("file").orEmpty(), o.str("before").orEmpty(), o.str("after").orEmpty(),
                    (o.long("additions") ?: 0L).toInt(), (o.long("deletions") ?: 0L).toInt(), o.str("status"),
                )
            }
        }

    suspend fun ports(b: ChatBackend, ptyId: String): CodeResult<List<DevPort>> =
        b.codeCall("GET", "/v1/pty/${encode(ptyId)}/ports", doing = "load the ports").map { body ->
            ((body as? JsonArray) ?: JsonArray(emptyList())).mapNotNull { it as? JsonObject }
                .mapNotNull { o -> o.long("port")?.let { DevPort(it.toInt(), o.str("command")) } }
        }

    suspend fun mintPreviewToken(b: ChatBackend, ptyId: String, port: Int): CodeResult<String> {
        val body = buildJsonObject { put("port", port) }.toString()
        return b.codeCall("POST", "/v1/pty/${encode(ptyId)}/preview/token", body, "open the preview").map { (it as? JsonObject).str("token").orEmpty() }
    }

    // Terminal service (cmd/allternit-api terminal_routes.rs; web lib/terminal-api.ts, iOS TerminalServiceClient):
    // the pane-engine shells the Code terminal's tiles run in, as on Desktop, the web and iOS. The relay reaches
    // these on the computer's runtime; gizzi-code's `/v1/pty` is not served there (only ports and the dev
    // preview below still use it, on a gizzi-code pty).

    /**
     * `POST /terminal/create {shell, cols, rows, cwd?}` → the terminal id (`data.session_id`). zsh is only the
     * default: where the computer has none, an empty shell runs its own default (iOS create's retry).
     */
    suspend fun createTerminal(b: ChatBackend, cols: Int, rows: Int, cwd: String? = null, options: TerminalCreateOptions = TerminalCreateOptions()): CodeResult<String> {
        var r = createTerminal(b, "/bin/zsh", cols, rows, cwd, options)
        if (r is CodeResult.Failed && MISSING_SHELL.containsMatchIn(r.message)) r = createTerminal(b, "", cols, rows, cwd, options)
        // The computer's own words (status set) become a friendly sentence; relay failures already are one.
        return if (r is CodeResult.Failed && r.status != null) CodeResult.Failed(FriendlyErrorMessage.from(r.message, "start a shell"), r.status) else r
    }

    private val MISSING_SHELL = Regex("ENOENT|doesn't exist|No such file")

    private suspend fun createTerminal(b: ChatBackend, shell: String, cols: Int, rows: Int, cwd: String?, options: TerminalCreateOptions): CodeResult<String> {
        val body = buildJsonObject {
            put("shell", shell); put("cols", cols); put("rows", rows)
            if (cwd != null) put("cwd", cwd)
            options.command?.let { put("command", it) }
            if (options.args.isNotEmpty()) put("args", buildJsonArray { options.args.forEach { add(JsonPrimitive(it)) } })
            if (options.env.isNotEmpty()) put("env", buildJsonObject { options.env.forEach { (k, v) -> put(k, v) } })
            options.title?.let { put("title", it) }
        }.toString()
        return when (val r = b.runtimeJson("POST", "/terminal/create", body)) {
            is RuntimeCall.Ok -> {
                val o = r.body as? JsonObject
                val id = o.str("sessionId") ?: o.str("session_id") ?: o.child("data").str("session_id")
                when {
                    o.bool("success") == false -> CodeResult.Failed(o.str("message") ?: "The shell didn't start.", 500)
                    id.isNullOrEmpty() -> CodeResult.Failed("Your computer didn't return a terminal.")
                    else -> CodeResult.Ok(id)
                }
            }
            RuntimeCall.NoComputer -> CodeResult.NoComputer
            // The raw message is kept so the missing-shell retry can read it.
            is RuntimeCall.Http -> CodeResult.Failed(r.message ?: r.code ?: "HTTP ${r.status}", r.status)
            is RuntimeCall.Failed -> CodeResult.Failed(r.message)
        }
    }

    /**
     * `GET /terminal/list?preview=N` (platform #2795): every terminal on the computer, each with its last [preview]
     * screen rows. [LiveCall.Unsupported] on a runtime without the route.
     */
    suspend fun listTerminals(b: ChatBackend, preview: Int): LiveCall<List<TerminalListItem>> =
        liveCall(b, "GET", "/terminal/list?preview=$preview") { LiveWire.terminals(it) }

    // Live sessions (platform #2799, #2800; web live-sessions-api.ts, iOS LiveSessionsClient): coding-agent CLI
    // sessions on the computer, and Move / Open a copy / New session here. The error code is kept, since every
    // adopt refusal has its own sentence (OnYourComputer.moveErrorText).

    /** `GET /api/v1/live-sessions`. */
    suspend fun liveSessions(b: ChatBackend): LiveCall<List<LiveSession>> =
        liveCall(b, "GET", "/api/v1/live-sessions") { LiveWire.sessions(it) }

    /**
     * `POST /api/v1/live-sessions/adopt {sessionId?|address?, harness, mode, cols, rows}`. mode: move | copy | new |
     * cancel. A move answers 202 with a job to follow; copy and new answer 200 with the new terminal.
     */
    suspend fun adopt(b: ChatBackend, session: LiveSession, mode: String, cols: Int, rows: Int): LiveCall<AdoptStart> {
        val body = buildJsonObject {
            if (session.sessionId.isNotEmpty()) put("sessionId", session.sessionId)
            if (session.address.isNotEmpty()) put("address", session.address)
            put("harness", session.harness); put("mode", mode); put("cols", cols); put("rows", rows)
        }.toString()
        return when (val r = liveCall(b, "POST", "/api/v1/live-sessions/adopt", body) { it as? JsonObject }) {
            is LiveCall.Ok -> LiveWire.adoptReply(mode, r.value)
            LiveCall.Unsupported -> LiveCall.Unsupported
            LiveCall.NoComputer -> LiveCall.NoComputer
            is LiveCall.Failed -> r
        }
    }

    /** `GET /api/v1/live-sessions/adopt/:jobId`. */
    suspend fun adoptJob(b: ChatBackend, jobId: String): LiveCall<AdoptJob> =
        when (val r = liveCall(b, "GET", "/api/v1/live-sessions/adopt/${encode(jobId)}") { LiveWire.job(it as? JsonObject) }) {
            is LiveCall.Ok -> r.value?.let { LiveCall.Ok(it) } ?: LiveCall.Failed("bad_response", "Your computer didn't return the move.")
            LiveCall.Unsupported -> LiveCall.Unsupported
            LiveCall.NoComputer -> LiveCall.NoComputer
            is LiveCall.Failed -> r
        }

    /**
     * One relayed call that keeps the runtime's error code. A 404 with no code of its own is an older runtime
     * without the route ([LiveCall.Unsupported]); a 404 that carries a code (an adopt refusal) is a real answer.
     */
    private suspend fun <T> liveCall(b: ChatBackend, method: String, path: String, body: String? = null, parse: (JsonElement) -> T): LiveCall<T> =
        when (val r = b.runtimeJson(method, path, body)) {
            is RuntimeCall.Ok -> LiveCall.Ok(parse(r.body))
            RuntimeCall.NoComputer -> LiveCall.NoComputer
            is RuntimeCall.Http ->
                if (r.status == 404 && (r.code == null || r.code == "not_found")) LiveCall.Unsupported
                else LiveCall.Failed(r.code ?: "unknown", r.message?.trim()?.takeIf { it.isNotEmpty() } ?: "Your computer answered ${r.status}.", r.status)
            is RuntimeCall.Failed -> LiveCall.Failed("unreachable", r.message)
        }

    /** `POST /terminal/:id/input {content}`: raw bytes for the pty (a real pty reads '\r' as Enter). 404 = the terminal is gone. */
    suspend fun terminalInput(b: ChatBackend, id: String, content: String, target: String? = null): CodeResult<Unit> {
        val body = buildJsonObject { put("session_id", id); put("content", content) }.toString()
        return b.codeCall("POST", "/terminal/${encode(id)}/input", body, "send the keys", target).map { }
    }

    /** `POST /terminal/:id/resize {cols, rows}`. 404 = the terminal is gone. */
    suspend fun resizeTerminal(b: ChatBackend, id: String, cols: Int, rows: Int, target: String? = null): CodeResult<Unit> {
        val body = buildJsonObject { put("session_id", id); put("cols", cols); put("rows", rows) }.toString()
        return b.codeCall("POST", "/terminal/${encode(id)}/resize", body, "resize the shell", target).map { }
    }

    /** `POST /terminal/:id/close`: ends the shell on the computer (an unknown id is fine). */
    suspend fun closeTerminal(b: ChatBackend, id: String, target: String? = null) {
        val body = buildJsonObject { put("session_id", id) }.toString()
        b.codeCall("POST", "/terminal/${encode(id)}/close", body, "close the shell", target)
    }

    /** `GET /terminal/:id/stream`: the SSE path (scrollback replay, then live output, then `exit`). */
    fun terminalStreamPath(id: String): String = "/terminal/${encode(id)}/stream"

    // gizzi-code ptys (`/v1/pty`): the Computers console terminal (feature/computer) still opens these.
    suspend fun createPty(b: ChatBackend, title: String?, target: String? = null): CodeResult<String> {
        val body = buildJsonObject { if (title != null) put("title", title) }.toString()
        return b.codeCall("POST", "/v1/pty/create", body, "start a shell", target).map { (it as? JsonObject).str("id").orEmpty() }
    }

    suspend fun resizePty(b: ChatBackend, id: String, cols: Int, rows: Int, target: String? = null) {
        val body = buildJsonObject { put("size", buildJsonObject { put("cols", cols); put("rows", rows) }) }.toString()
        b.codeCall("PUT", "/v1/pty/${encode(id)}", body, "resize the shell", target)
    }

    suspend fun deletePty(b: ChatBackend, id: String, target: String? = null) {
        b.codeCall("DELETE", "/v1/pty/${encode(id)}", doing = "close the shell", target = target)
    }

    suspend fun terminalBots(b: ChatBackend): CodeResult<TerminalBots> =
        b.codeCall("GET", "/api/factory/agents", doing = "load Terminal bots").map { body ->
            val o = body as? JsonObject
            val engine = o.child("engine")
            val bots = o.list("agents").filter { a ->
                a.child("binding").str("type") == "terminal" && a.child("pane") != null
            }.map { a ->
                val machine = a.child("machine")
                TerminalBot(
                    a.str("id").orEmpty(), a.str("slug").orEmpty(), a.str("name").orEmpty(), a.str("address").orEmpty(), a.str("state").orEmpty(),
                    machine?.takeIf { it.str("id") != "local" }?.let { it.str("name") ?: it.str("id") },
                )
            }
            TerminalBots(bots, engine.bool("running") ?: true, engine.str("error"))
        }

    suspend fun sendPaneInput(b: ChatBackend, agentId: String, text: String? = null, keys: List<String>? = null) {
        val body = buildJsonObject {
            if (text != null) put("text", text)
            if (keys != null) put("keys", buildJsonArray { keys.forEach { add(JsonPrimitive(it)) } })
        }.toString()
        b.codeCall("POST", "/api/factory/agents/${encode(agentId)}/input", body, "send the keys")
    }

    // Sessions (iOS CodeSessionsClient): PATCH /api/v1/agent-sessions/:id on the cloud.
    private suspend fun patch(sessionId: String, body: JsonObject): String? =
        when (val r = LibraryNet.send("PATCH", "agent-sessions/${encode(sessionId)}", body)) {
            is ApiResult.Ok -> null
            is ApiResult.Decode -> null
            else -> r.friendly("update the thread")
        }

    suspend fun rename(sessionId: String, name: String) = patch(sessionId, buildJsonObject { put("name", name) })
    suspend fun archive(sessionId: String) = patch(sessionId, buildJsonObject { put("active", false) })
    suspend fun markReviewed(sessionId: String) = patch(sessionId, buildJsonObject { put("markReviewed", true) })
    suspend fun applyPermission(sessionId: String, mode: CodePermissionMode) =
        patch(sessionId, buildJsonObject { put("metadata", buildJsonObject { put("permission", mode.rules()) }) })

    /** iOS updateSessionMetadata(isolation: "worktree"): the launch's Worktree pick isolates the new thread. */
    suspend fun applyIsolation(sessionId: String) =
        patch(sessionId, buildJsonObject { put("metadata", buildJsonObject { put("isolation", "worktree") }) })

    /** Pending permission asks for a session, raw (the changeset sheet needs `metadata.diff`). */
    suspend fun pendingPermissions(sessionId: String): List<ChangeRequest> {
        val r = LibraryNet.get("permissions?sessionId=${encode(sessionId)}") as? ApiResult.Ok<JsonElement> ?: return emptyList()
        val rows = ((r.value as? JsonObject)?.get("permissions") as? JsonArray) ?: (r.value as? JsonArray) ?: return emptyList()
        return rows.mapNotNull { it as? JsonObject }.mapNotNull { o ->
            val id = o.str("id") ?: return@mapNotNull null
            val meta = o.child("metadata")
            val counts = meta.child("filediff")
            ChangeRequest(
                id = id,
                permission = o.str("permission").orEmpty(),
                patterns = (o["patterns"] as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.content }.orEmpty(),
                path = meta.str("filepath") ?: meta.str("path"),
                diff = meta.str("diff"),
                additions = (counts.long("additions") ?: 0L).toInt(),
                deletions = (counts.long("deletions") ?: 0L).toInt(),
            )
        }.sortedBy { it.id }
    }

    /** `POST permissions/:id/reply {reply}`: "once", "always", "reject". */
    suspend fun reply(b: ChatBackend, id: String, reply: String): String? =
        when (val r = b.replyPermission(id, reply)) {
            is ApiResult.Ok -> null
            else -> r.friendly("send your answer")
        }
}

// MARK: - Terminal list and live sessions (records)

/** Optional parts of a new terminal (web `createTerminalSession`, iOS CreateOptions): a program instead of the shell, its args, env and listed name. */
data class TerminalCreateOptions(
    val command: String? = null,
    val args: List<String> = emptyList(),
    val env: Map<String, String> = emptyMap(),
    val title: String? = null,
)

/** One open terminal as the runtime lists it (`GET /terminal/list`, iOS TerminalListItem). */
data class TerminalListItem(
    val sessionId: String,
    val paneId: String? = null,
    val title: String = "",
    val cwd: String = "",
    val command: List<String> = emptyList(),
    val running: Boolean = true,
    val exitCode: Int? = null,
    val pid: Int? = null,
    val createdAt: String? = null,
    /** Last visible rows, top to bottom; only when `preview` was asked for. */
    val preview: List<String>? = null,
)

/** One process that has a live session open (`holders[]`). [app]: "Terminal", "iTerm2", "Allternit", "Ghostty", "other". */
data class LiveSessionHolder(
    val pid: Int,
    val app: String,
    val terminalId: String? = null,
    val paneId: String? = null,
    val tty: String? = null,
    val startedAt: String? = null,
)

/** A Move job (`move` on a live session, `GET .../adopt/:jobId`). [state]: waiting_idle | stopping | starting | done | failed. */
data class AdoptJob(
    val jobId: String,
    val state: String,
    val terminalId: String? = null,
    val code: String? = null,
    val error: String? = null,
    /** Phone-only line under the progress (a cancel that came too late). */
    val notice: String? = null,
) {
    val finished: Boolean get() = state == "done" || state == "failed"
}

/** A coding-agent CLI session on the computer (`GET /api/v1/live-sessions`, camelCase; iOS LiveSession). */
data class LiveSession(
    val sessionId: String,
    val address: String = "",
    val name: String = "",
    val harness: String = "",
    val machineName: String = "",
    val pid: Int? = null,
    val cwd: String = "",
    /** busy | idle | offline */
    val status: String = "idle",
    val holders: List<LiveSessionHolder> = emptyList(),
    val carriedFlags: List<String> = emptyList(),
    val canCopy: Boolean = false,
    /** Running outside Allternit. */
    val external: Boolean = false,
    /** now | busy | open_elsewhere | no_resume | here */
    val movable: String? = null,
    /** The exact argv the runtime runs for each action: shown, never built on the phone. */
    val moveCommand: List<String>? = null,
    val copyCommand: List<String>? = null,
    val newCommand: List<String>? = null,
    val move: AdoptJob? = null,
)

/** A terminal-list or live-sessions call's answer. */
sealed interface LiveCall<out T> {
    data class Ok<T>(val value: T) : LiveCall<T>
    /** The runtime has no such route (404, an older runtime): hide the section quietly. */
    data object Unsupported : LiveCall<Nothing>
    data object NoComputer : LiveCall<Nothing>
    /** [code] is the runtime's `{error}` code, or "unreachable" when no answer came. */
    data class Failed(val code: String, val message: String, val status: Int? = null) : LiveCall<Nothing>
}

/** What an adopt call started: a Move job to follow, the new terminal (copy / new), or a cancel that went through. */
sealed interface AdoptStart {
    data class Job(val job: AdoptJob) : AdoptStart
    data class Terminal(val id: String) : AdoptStart
    data object Cancelled : AdoptStart
}

/** The wire shapes, lenient like the iOS decoders: a missing field reads empty, so one odd row never fails a list. */
internal object LiveWire {
    private fun strings(e: JsonElement?): List<String>? =
        (e as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.takeIf { p -> p !is JsonNull }?.content }

    private fun objects(body: JsonElement?, key: String): List<JsonObject> = when (body) {
        is JsonArray -> body.mapNotNull { it as? JsonObject }
        is JsonObject -> body.list(key)
        else -> emptyList()
    }

    fun terminal(o: JsonObject): TerminalListItem? {
        val id = o.str("session_id")?.takeIf { it.isNotEmpty() } ?: return null
        return TerminalListItem(
            sessionId = id, paneId = o.str("pane_id"), title = o.str("title").orEmpty(), cwd = o.str("cwd").orEmpty(),
            command = strings(o["command"]).orEmpty(), running = o.bool("running") ?: true,
            exitCode = o.long("exit_code")?.toInt(), pid = o.long("pid")?.toInt(), createdAt = o.str("created_at"),
            preview = strings(o["preview"]),
        )
    }

    /** `{terminals: [...]}` (a bare array is read too). One row per id: the list keys its rows by id. */
    fun terminals(body: JsonElement): List<TerminalListItem> = objects(body, "terminals").mapNotNull(::terminal).distinctBy { it.sessionId }

    fun job(o: JsonObject?): AdoptJob? = o?.let {
        AdoptJob(it.str("jobId").orEmpty(), it.str("state") ?: "waiting_idle", it.str("terminalId"), it.str("code"), it.str("error"))
    }

    private fun holder(o: JsonObject): LiveSessionHolder? {
        val pid = o.long("pid")?.toInt() ?: return null
        return LiveSessionHolder(pid, o.str("app") ?: "other", o.str("terminalId"), o.str("paneId"), o.str("tty"), o.str("startedAt"))
    }

    fun session(o: JsonObject): LiveSession? {
        val id = o.str("sessionId")?.takeIf { it.isNotEmpty() } ?: return null
        return LiveSession(
            sessionId = id, address = o.str("address").orEmpty(), name = o.str("name").orEmpty(), harness = o.str("harness").orEmpty(),
            machineName = o.str("machineName").orEmpty(), pid = o.long("pid")?.toInt(), cwd = o.str("cwd").orEmpty(),
            status = o.str("status") ?: "idle", holders = o.list("holders").mapNotNull(::holder),
            carriedFlags = strings(o["carriedFlags"]).orEmpty(), canCopy = o.bool("canCopy") == true, external = o.bool("external") == true,
            movable = o.str("movable"), moveCommand = strings(o["moveCommand"]), copyCommand = strings(o["copyCommand"]),
            newCommand = strings(o["newCommand"]), move = job(o.child("move")),
        )
    }

    /** A bare array, or `{sessions: [...]}`. */
    fun sessions(body: JsonElement): List<LiveSession> = objects(body, "sessions").mapNotNull(::session).distinctBy { it.sessionId }

    /** The adopt reply for [mode]: a move needs a job id, copy and new a terminal id. */
    fun adoptReply(mode: String, o: JsonObject?): LiveCall<AdoptStart> = when (mode) {
        "cancel" -> LiveCall.Ok(AdoptStart.Cancelled)
        "move" -> o.str("jobId")?.takeIf { it.isNotEmpty() }
            ?.let { LiveCall.Ok(AdoptStart.Job(AdoptJob(it, o.str("state") ?: "waiting_idle"))) }
            ?: LiveCall.Failed("bad_response", "Your computer didn't return a move job.")
        else -> o.str("terminalId")?.takeIf { it.isNotEmpty() }
            ?.let { LiveCall.Ok(AdoptStart.Terminal(it)) }
            ?: LiveCall.Failed("bad_response", "Your computer didn't return a terminal.")
    }
}

/** A pending permission ask with its diff (iOS PermissionRequest + metadata). */
data class ChangeRequest(
    val id: String,
    val permission: String,
    val patterns: List<String>,
    val path: String?,
    val diff: String?,
    val additions: Int,
    val deletions: Int,
)
