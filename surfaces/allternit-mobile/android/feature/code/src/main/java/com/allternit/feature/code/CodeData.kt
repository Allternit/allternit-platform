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
