package com.allternit.feature.chat

import android.content.Context
import com.allternit.core.api.ApiCall
import com.allternit.core.api.ApiConfig
import com.allternit.core.api.ApiJson
import com.allternit.core.api.ApiResult
import com.allternit.core.api.gen.AgentChatRequest
import com.allternit.core.api.gen.AgentSession
import com.allternit.core.api.gen.AgentSessionMessage
import com.allternit.core.api.gen.AllternitApi
import com.allternit.core.api.gen.CreateAgentSessionRequest
import com.allternit.core.stream.AgentChatStream
import com.allternit.core.stream.ChatStreamEvent
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.flow.Flow
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import okhttp3.HttpUrl
import okhttp3.OkHttpClient
import okhttp3.Request

/**
 * Every call the chat makes (iOS AgentChatClient + AgentAsksClient + RuntimeRelayClient).
 *
 * - REST goes to allternit-api's `/api/v1` ([ApiConfig.apiBaseUrl]): sessions, messages, abort,
 *   revert, branches, delete, uploads, questions and permissions.
 * - `POST /api/agent-chat` goes to the user's computer through cloud-api's runtime relay, the
 *   Release default on iOS (InstanceConnection.resolve step 5) and the PWA's path on
 *   m.allternit.com: the request is wrapped in a `{ method, path, headers, body, bodyEncoding }`
 *   envelope and posted to `{cloud}/api/v1/runtime-devices/:id/proxy`; the relay's response IS
 *   the computer's event stream. [RuntimeRelay] picks the computer like iOS RuntimeRelayStore.
 */
class ChatBackend(
    private val okHttp: OkHttpClient,
    val api: AllternitApi,
    private val config: ApiConfig = ApiConfig(),
    context: Context,
) {
    private val json = ApiJson.default
    private val stream = AgentChatStream(okHttp, api, json)
    val relay = RuntimeRelay(okHttp, config, context)

    /** Public fetches (GitHub raw files, link-preview images): never carries the bearer. */
    internal val publicHttp: OkHttpClient by lazy {
        OkHttpClient.Builder().connectTimeout(15, TimeUnit.SECONDS).readTimeout(30, TimeUnit.SECONDS).build()
    }

    /** allternit-cloud-api's origin (thread shares, the runtime relay). */
    internal val cloudBaseUrl: HttpUrl get() = config.cloudBaseUrl

    /** The bearer-carrying client, for `GET uploads/:id` (run_code files). */
    internal val authedHttp: OkHttpClient get() = okHttp

    internal val mcpJson get() = json

    /** `{api}/mcp/<segments>`; each id is percent-encoded (it carries ':'). */
    internal fun mcpUrl(vararg segments: String): HttpUrl =
        config.apiBaseUrl.newBuilder().addPathSegment("mcp").apply { segments.forEachIndexed { i, s -> if (i == 1) addEncodedPathSegment(escapeMcpId(s)) else addPathSegment(s) } }.build()

    private fun v1(vararg segments: String): HttpUrl =
        config.apiBaseUrl.newBuilder().apply { segments.forEach { addPathSegment(it) } }.build()

    /** Authenticated sync only; image execution stays in this phone's WebView. */
    internal suspend fun imageSync(method: String, id: String? = null, action: String? = null, body: String? = null): ApiResult<kotlinx.serialization.json.JsonElement> {
        val parts = mutableListOf("pane-render")
        if (id != null) parts += id
        if (action != null) parts += action
        return ApiCall.execute(okHttp, json, ApiCall.request(method, v1(*parts.toTypedArray()), body), kotlinx.serialization.json.JsonElement.serializer())
    }

    // MARK: Sessions

    /**
     * `POST agent-sessions` (iOS AgentChatClient.createSession): name, origin_surface, agent ids,
     * and the metadata bag (`agentModeId`, `permissionMode`, `session_mode`).
     */
    suspend fun createSession(
        name: String,
        originSurface: String,
        agentId: String?,
        agentName: String?,
        agentModeId: String?,
        permissionMode: String?,
        sessionMode: String,
        /** Harness & Bots 2.0: the runtime this chat runs on (`metadata.runtime` + `metadata.botBrain`). */
        botBrain: JsonObject? = null,
    ): ApiResult<AgentSession> {
        val metadata = buildJsonObject {
            put("session_mode", sessionMode)
            botBrain?.let { bb ->
                (bb["runtime"] as? kotlinx.serialization.json.JsonPrimitive)?.content?.let { put("runtime", it) }
                put("botBrain", bb)
            }
            agentModeId?.let { put("agentModeId", it) }
            permissionMode?.let { put("permissionMode", it) }
        }
        return api.createAgentSession(
            CreateAgentSessionRequest(
                name = name,
                agentId = agentId,
                agentName = agentName,
                originSurface = originSurface,
                metadata = metadata,
            ),
        )
    }

    suspend fun listMessages(sessionId: String): ApiResult<List<AgentSessionMessage>> = api.listAgentSessionMessages(sessionId)

    /** `POST agent-sessions/:id/abort` (the Stop button). */
    suspend fun abort(sessionId: String): ApiResult<Unit> =
        ApiCall.executeUnit(okHttp, json, ApiCall.request("POST", v1("agent-sessions", sessionId, "abort"), "{}"))

    /** `POST agent-sessions/:id/revert {messageId}` (edit-resend). */
    suspend fun revert(sessionId: String, messageId: String): ApiResult<Unit> {
        val body = buildJsonObject { put("messageId", messageId) }.toString()
        return ApiCall.executeUnit(okHttp, json, ApiCall.request("POST", v1("agent-sessions", sessionId, "revert"), body))
    }

    /** `GET agent-sessions/:id/branches`: earlier versions of edited turns (404 → none). */
    suspend fun branches(sessionId: String): List<MessageBranch> {
        val r = ApiCall.execute(okHttp, json, ApiCall.request("GET", v1("agent-sessions", sessionId, "branches"), null), JsonArray.serializer())
        val rows = (r as? ApiResult.Ok)?.value ?: return emptyList()
        return rows.mapNotNull { MessageBranch.from(it as? JsonObject ?: return@mapNotNull null) }
    }

    /** `DELETE agent-sessions/:id`. */
    suspend fun deleteSession(sessionId: String): ApiResult<Unit> =
        ApiCall.executeUnit(okHttp, json, ApiCall.request("DELETE", v1("agent-sessions", sessionId), null))

    // MARK: Uploads

    /** `POST uploads {name, mediaType, dataBase64}` → `{uploadId, url}` (JSON, not multipart, as on iOS). */
    suspend fun upload(name: String, mediaType: String, bytes: ByteArray): ApiResult<String> {
        val body = buildJsonObject {
            put("name", name)
            put("mediaType", mediaType)
            put("dataBase64", android.util.Base64.encodeToString(bytes, android.util.Base64.NO_WRAP))
        }.toString()
        return when (val r = ApiCall.execute(okHttp, json, ApiCall.request("POST", v1("uploads"), body), JsonObject.serializer())) {
            is ApiResult.Ok -> r.value.str("url")?.let { ApiResult.Ok(it) }
                ?: ApiResult.Decode(200, kotlinx.serialization.SerializationException("upload response has no url"))
            is ApiResult.HttpError -> r
            is ApiResult.Network -> r
            is ApiResult.Decode -> r
        }
    }

    /** The bytes of an upload (`GET uploads/:id`, run_code files), cached per id by the caller. */
    internal fun uploadRequest(uploadId: String): Request = ApiCall.request("GET", v1("uploads", uploadId), null, accept = "*/*")

    // MARK: Asks

    /** `GET permissions?sessionId=` → `{permissions:[...]}` (iOS AgentAskStore.refresh: polled, not streamed). */
    suspend fun listPermissions(sessionId: String): List<PermissionAsk> {
        val url = v1("permissions").newBuilder().addQueryParameter("sessionId", sessionId).build()
        val r = ApiCall.execute(okHttp, json, ApiCall.request("GET", url, null), JsonObject.serializer())
        val rows = (r as? ApiResult.Ok)?.value?.arr("permissions").orEmpty()
        return rows.mapNotNull { PermissionAsks.from(it as? JsonObject ?: return@mapNotNull null, sessionId) }
    }

    /** `POST permissions/:id/reply {reply}`: "once" (Allow), "always", "reject" (Deny). */
    suspend fun replyPermission(id: String, reply: String): ApiResult<Unit> {
        val body = buildJsonObject { put("reply", reply) }.toString()
        return ApiCall.executeUnit(okHttp, json, ApiCall.request("POST", v1("permissions", id, "reply"), body))
    }

    /** `POST questions/:id/reply {answers:[{questionIndex, answer}]}`; a multi-pick answer is a list. */
    suspend fun replyQuestion(id: String, answers: List<List<String>>): ApiResult<Unit> {
        val body = buildJsonObject {
            put("answers", buildJsonArray {
                answers.forEachIndexed { index, picked ->
                    add(buildJsonObject {
                        put("questionIndex", index)
                        if (picked.size == 1) put("answer", picked.first()) else put("answer", JsonArray(picked.map { JsonPrimitive(it) }))
                    })
                }
            })
        }.toString()
        return ApiCall.executeUnit(okHttp, json, ApiCall.request("POST", v1("questions", id, "reply"), body))
    }

    /** `POST questions/:id/reject {}` (Skip). */
    suspend fun rejectQuestion(id: String): ApiResult<Unit> =
        ApiCall.executeUnit(okHttp, json, ApiCall.request("POST", v1("questions", id, "reject"), "{}"))

    // MARK: Session tasks (iOS SessionTasksClient, wave E run tree)

    /** `GET agent-sessions/:id/tasks`: the gizzi tasks (subagents) of a session. */
    suspend fun sessionTasks(sessionId: String): ApiResult<kotlinx.serialization.json.JsonElement> =
        ApiCall.execute(okHttp, json, ApiCall.request("GET", v1("agent-sessions", sessionId, "tasks"), null), kotlinx.serialization.json.JsonElement.serializer())

    /** `POST agent-sessions/:id/background-tasks/:task/(cancel|interrupt|resume)`. */
    suspend fun backgroundTask(sessionId: String, taskId: String, action: String, jsonBody: String = "{}"): ApiResult<Unit> =
        ApiCall.executeUnit(okHttp, json, ApiCall.request("POST", v1("agent-sessions", sessionId, "background-tasks", taskId, action), jsonBody))

    // MARK: Runtime REST (allternit-api on the user's computer)

    /**
     * One JSON call to a route that lives on the user's computer (iOS
     * InstanceConnection.runtimeAPIData + RuntimeRelayClient.send): the request rides the relay
     * envelope to the picked computer, and the relay's response IS the computer's response.
     * [path] is the runtime path with its query, e.g. `/api/v1/inbox?scope=needs_you`.
     * 503 `runtime_warming` waits and retries for up to about 45 s like iOS; an offline
     * computer is marked unreachable for the next pick.
     */
    suspend fun runtimeJson(method: String, path: String, jsonBody: String? = null, targetRuntimeId: String? = null): RuntimeCall {
        // A notification action acts on the computer the push names (iOS NotificationActions runtimeId), not the picked one.
        val runtimeId = if (!targetRuntimeId.isNullOrBlank()) targetRuntimeId else when (val pick = relay.resolve()) {
            is RelayPick.Computer -> pick.runtimeId
            RelayPick.NoComputer -> return RuntimeCall.NoComputer
            RelayPick.Unreachable -> return RuntimeCall.Failed(ChatErrors.UNREACHABLE)
            is RelayPick.Failed -> return if (pick.gate) RuntimeCall.NoComputer else RuntimeCall.Failed(pick.message)
        }
        val envelope = buildJsonObject {
            put("method", method)
            put("path", path)
            put("headers", buildJsonObject {
                put("accept", "application/json")
                if (jsonBody != null) put("content-type", "application/json")
            })
            if (jsonBody != null) put("body", jsonBody)
            put("bodyEncoding", "utf8")
        }.toString()
        repeat(WARM_TRIES) { attempt ->
            val request = ApiCall.request("POST", relay.proxyUrl(runtimeId), envelope)
            when (val r = ApiCall.execute(okHttp, json, request, kotlinx.serialization.json.JsonElement.serializer())) {
                is ApiResult.Ok -> {
                    relay.markReachable(runtimeId)
                    return RuntimeCall.Ok(r.value)
                }
                is ApiResult.Decode -> {
                    // An empty 2xx body (a bare 204-style answer) is still a success.
                    relay.markReachable(runtimeId)
                    return RuntimeCall.Ok(kotlinx.serialization.json.JsonNull)
                }
                is ApiResult.HttpError -> when {
                    r.status == 503 && r.code == "runtime_warming" && attempt < WARM_TRIES - 1 ->
                        kotlinx.coroutines.delay(WARM_DELAY_MS)
                    r.status == 503 && (r.code == "runtime_offline" || r.code == "runtime_warming") -> {
                        relay.markUnreachable(runtimeId)
                        return RuntimeCall.Failed(ChatErrors.UNREACHABLE)
                    }
                    ChatErrors.isComputeGate(r.status, r.code, r.message) -> return RuntimeCall.NoComputer
                    else -> return RuntimeCall.Http(r.status, r.code, r.message)
                }
                is ApiResult.Network -> return RuntimeCall.Failed("Could not connect to the server. Check your connection and try again.")
            }
        }
        return RuntimeCall.Failed(ChatErrors.UNREACHABLE)
    }

    /**
     * A binary GET on the user's computer through the relay (iOS PhoneTaskClient.crop): the
     * status and body as the computer sent them, or null when no computer answers.
     */
    suspend fun runtimeBytes(path: String, accept: String): Pair<Int, ByteArray>? {
        val runtimeId = (relay.resolve() as? RelayPick.Computer)?.runtimeId ?: return null
        val envelope = buildJsonObject {
            put("method", "GET")
            put("path", path)
            put("headers", buildJsonObject { put("accept", accept) })
            put("bodyEncoding", "utf8")
        }.toString()
        val request = ApiCall.request("POST", relay.proxyUrl(runtimeId), envelope, accept = accept)
        return kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.IO) {
            runCatching {
                okHttp.newCall(request).execute().use { it.code to (it.body?.bytes() ?: ByteArray(0)) }
            }.getOrNull()
        }
    }

    // MARK: Runtime sockets and streams (Code mode: the shell terminal and the Terminal-bot panes)

    /**
     * Opens a WebSocket to [path] on the user's computer through the relay (iOS
     * RuntimeRelayClient.webSocket): `POST runtime-devices/:id/socket-ticket {path}`, then the
     * cloud's `/socket?ticket=`. Frames before the tunnel's `allternit_socket_ready` sentinel are
     * dropped, as on the PWA; [onText] / [onBinary] then get every frame and [onEnd] the end.
     */
    suspend fun openRuntimeSocket(
        path: String,
        onText: (String) -> Unit,
        onBinary: (ByteArray) -> Unit,
        onEnd: () -> Unit,
        targetRuntimeId: String? = null,
    ): RuntimeSocketOpen {
        val runtimeId = if (!targetRuntimeId.isNullOrBlank()) targetRuntimeId else when (val pick = relay.resolve()) {
            is RelayPick.Computer -> pick.runtimeId
            RelayPick.NoComputer -> return RuntimeSocketOpen.NoComputer
            RelayPick.Unreachable -> return RuntimeSocketOpen.Failed(ChatErrors.UNREACHABLE)
            is RelayPick.Failed -> return if (pick.gate) RuntimeSocketOpen.NoComputer else RuntimeSocketOpen.Failed(pick.message)
        }
        val ticketUrl = config.cloudBaseUrl.newBuilder()
            .addPathSegments("api/v1/runtime-devices").addPathSegment(runtimeId).addPathSegment("socket-ticket").build()
        val ticketBody = buildJsonObject { put("path", path) }.toString()
        val ticket = when (val r = ApiCall.execute(okHttp, json, ApiCall.request("POST", ticketUrl, ticketBody), JsonObject.serializer())) {
            is ApiResult.Ok -> (r.value["ticket"] as? JsonPrimitive)?.content.orEmpty()
            is ApiResult.HttpError -> {
                if (ChatErrors.isComputeGate(r.status, r.code, r.message)) return RuntimeSocketOpen.NoComputer
                return RuntimeSocketOpen.Failed(ChatErrors.statusSentence(r.status) ?: ChatErrors.UNREACHABLE)
            }
            else -> return RuntimeSocketOpen.Failed(ChatErrors.UNREACHABLE)
        }
        if (ticket.isEmpty()) return RuntimeSocketOpen.Failed(ChatErrors.UNREACHABLE)
        val socketUrl = config.cloudBaseUrl.newBuilder()
            .addPathSegments("api/v1/runtime-devices").addPathSegment(runtimeId).addPathSegment("socket")
            .addQueryParameter("ticket", ticket).build()
        val ready = kotlinx.coroutines.CompletableDeferred<Boolean>()
        var tunnelReady = false
        val socket = okHttp.newBuilder().readTimeout(0, TimeUnit.MILLISECONDS).pingInterval(25, TimeUnit.SECONDS).build()
            .newWebSocket(
                Request.Builder().url(socketUrl).build(),
                object : okhttp3.WebSocketListener() {
                    override fun onMessage(webSocket: okhttp3.WebSocket, text: String) {
                        if (!tunnelReady) {
                            if (text.contains("allternit_socket_ready")) {
                                tunnelReady = true
                                ready.complete(true)
                            }
                            return
                        }
                        onText(text)
                    }
                    override fun onMessage(webSocket: okhttp3.WebSocket, bytes: okio.ByteString) {
                        if (tunnelReady) onBinary(bytes.toByteArray())
                    }
                    override fun onClosed(webSocket: okhttp3.WebSocket, code: Int, reason: String) {
                        ready.complete(false)
                        onEnd()
                    }
                    override fun onFailure(webSocket: okhttp3.WebSocket, t: Throwable, response: okhttp3.Response?) {
                        ready.complete(false)
                        onEnd()
                    }
                },
            )
        val ok = kotlinx.coroutines.withTimeoutOrNull(20_000L) { ready.await() } == true
        if (!ok) {
            socket.cancel()
            relay.markUnreachable(runtimeId)
            return RuntimeSocketOpen.Failed(ChatErrors.UNREACHABLE)
        }
        relay.markReachable(runtimeId)
        return RuntimeSocketOpen.Opened(socket)
    }

    /**
     * A Server-Sent-Events GET on the user's computer through the relay (iOS
     * FactoryPaneClient.screenStream). Emits each event with its data; throws
     * [NoComputerException] or [IllegalStateException] (a friendly sentence) when it can't connect.
     */
    fun runtimeEvents(path: String): Flow<RuntimeEvent> = kotlinx.coroutines.flow.flow {
        val runtimeId = when (val pick = relay.resolve()) {
            is RelayPick.Computer -> pick.runtimeId
            RelayPick.NoComputer -> throw NoComputerException()
            RelayPick.Unreachable -> throw IllegalStateException(ChatErrors.UNREACHABLE)
            is RelayPick.Failed -> if (pick.gate) throw NoComputerException() else throw IllegalStateException(pick.message)
        }
        val envelope = buildJsonObject {
            put("method", "GET")
            put("path", path)
            put("headers", buildJsonObject { put("accept", "text/event-stream") })
            put("bodyEncoding", "utf8")
        }.toString()
        val request = ApiCall.request("POST", relay.proxyUrl(runtimeId), envelope, accept = "text/event-stream")
        val client = okHttp.newBuilder().readTimeout(0, TimeUnit.MILLISECONDS).build()
        val response = kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.IO) { client.newCall(request).execute() }
        response.use { r ->
            if (!r.isSuccessful) {
                if (ChatErrors.isComputeGate(r.code, null, null)) throw NoComputerException()
                throw IllegalStateException(ChatErrors.statusSentence(r.code) ?: ChatErrors.UNREACHABLE)
            }
            val source = r.body?.source() ?: throw IllegalStateException(ChatErrors.UNREACHABLE)
            var event = "message"
            while (true) {
                val line = kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.IO) { source.readUtf8Line() } ?: break
                when {
                    line.startsWith("event:") -> event = line.removePrefix("event:").trim()
                    line.startsWith("data:") -> {
                        emit(RuntimeEvent(event, line.removePrefix("data:").trim()))
                    }
                    line.isEmpty() -> event = "message"
                }
            }
        }
    }

    // MARK: Chat stream

    /**
     * Streams one turn through the relay to [runtimeId]. Failures arrive as
     * [ChatStreamEvent.Failed]: 503 `runtime_warming` (the caller waits and retries),
     * `runtime_offline`, 409 `no_brain`, 428 / `compute_required` (no computer).
     */
    fun streamTurn(runtimeId: String, body: AgentChatRequest, personTurn: Boolean = false): Flow<ChatStreamEvent> {
        val envelope = buildJsonObject {
            put("method", "POST")
            put("path", "/api/agent-chat")
            put("headers", buildJsonObject {
                put("accept", "text/event-stream")
                put("content-type", "application/json")
            })
            val encoded = json.encodeToJsonElement(AgentChatRequest.serializer(), body) as JsonObject
            val marked = if (personTurn) JsonObject(encoded + ("metadata" to JsonObject((encoded["metadata"] as? JsonObject).orEmpty() + ("turn" to JsonPrimitive("person"))))) else encoded
            put("body", marked.toString())
            put("bodyEncoding", "utf8")
        }.toString()
        val request = ApiCall.request("POST", relay.proxyUrl(runtimeId), envelope, accept = "text/event-stream")
        return stream.stream(request)
    }
}

/** An earlier version of an edited turn (iOS MessageBranch, `GET .../branches`). */
data class MessageBranch(val id: String, val messageId: String?, val role: String, val content: String, val createdAt: Long?) {
    companion object {
        fun from(o: JsonObject): MessageBranch? {
            val content = o.str("content") ?: o.str("text") ?: return null
            return MessageBranch(
                id = o.str("id") ?: content.hashCode().toString(),
                messageId = o.str("messageId") ?: o.str("message_id") ?: o.str("parentMessageId"),
                role = o.str("role") ?: "user",
                content = content,
                createdAt = o.long("created_at") ?: o.long("createdAt"),
            )
        }
    }
}

/** iOS AgentPermissionAsk decoding and labels (Features/Decisions/Models/AgentAsk.swift). */
internal object PermissionAsks {
    fun from(o: JsonObject, sessionId: String): PermissionAsk? {
        val id = o.str("id") ?: return null
        val session = o.str("sessionID") ?: o.str("sessionId") ?: o.str("session_id") ?: sessionId
        val permission = o.str("permission") ?: o.str("type") ?: "tool"
        val patterns = (o.arr("patterns").ifEmpty { o.arr("pattern") }).mapNotNull { (it as? JsonPrimitive)?.content }
        val review = o.obj("metadata")?.obj("review")
        allowsAlwaysById[id] = review?.str("class") != "confirm_now"
        return PermissionAsk(
            id = id,
            sessionId = session,
            permission = permission,
            title = actionLabel(permission, o.obj("metadata")?.str("folderAction") ?: o.str("folderAction")),
            patterns = patterns,
        )
    }

    /** confirm_now asks never offer "Always" (iOS allowsAlways). */
    private val allowsAlwaysById = java.util.concurrent.ConcurrentHashMap<String, Boolean>()

    fun allowsAlways(id: String): Boolean = allowsAlwaysById[id] ?: true

    fun actionLabel(permission: String, folderAction: String? = null): String = when (permission.lowercase()) {
        "bash", "shell" -> "Run a command"
        "edit", "write", "patch", "multiedit" -> "Edit files"
        "read" -> "Read files"
        "webfetch", "fetch" -> "Open a web page"
        "run_code" -> "Run Python code"
        "external_directory" -> "Work outside the project folder"
        "computer.access" -> "Use apps on your computer"
        "folder_access" -> if (folderAction == "request_delete") "Delete from a folder" else "Open a folder"
        else -> "Use $permission"
    }
}

/** The outcome of [ChatBackend.runtimeJson]. */
sealed interface RuntimeCall {
    data class Ok(val body: kotlinx.serialization.json.JsonElement) : RuntimeCall
    /** No computer on the account (or the compute gate refused): not an error, an empty state. */
    data object NoComputer : RuntimeCall
    /** The computer answered with an error status. */
    data class Http(val status: Int, val code: String?, val message: String?) : RuntimeCall
    /** Network, relay or unreachable computer; [message] is already friendly. */
    data class Failed(val message: String) : RuntimeCall
}

private const val WARM_TRIES = 15
private const val WARM_DELAY_MS = 3_000L

/** The outcome of [ChatBackend.openRuntimeSocket]. */
sealed interface RuntimeSocketOpen {
    data class Opened(val socket: okhttp3.WebSocket) : RuntimeSocketOpen
    data object NoComputer : RuntimeSocketOpen
    data class Failed(val message: String) : RuntimeSocketOpen
}

/** One Server-Sent Event from [ChatBackend.runtimeEvents]. */
data class RuntimeEvent(val event: String, val data: String)

/** Thrown by [ChatBackend.runtimeEvents] when the account has no computer (the 428 / compute_required state). */
class NoComputerException : Exception("No computer")
