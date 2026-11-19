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

    private fun v1(vararg segments: String): HttpUrl =
        config.apiBaseUrl.newBuilder().apply { segments.forEach { addPathSegment(it) } }.build()

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
    ): ApiResult<AgentSession> {
        val metadata = buildJsonObject {
            put("session_mode", sessionMode)
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

    // MARK: Chat stream

    /**
     * Streams one turn through the relay to [runtimeId]. Failures arrive as
     * [ChatStreamEvent.Failed]: 503 `runtime_warming` (the caller waits and retries),
     * `runtime_offline`, 409 `no_brain`, 428 / `compute_required` (no computer).
     */
    fun streamTurn(runtimeId: String, body: AgentChatRequest): Flow<ChatStreamEvent> {
        val envelope = buildJsonObject {
            put("method", "POST")
            put("path", "/api/agent-chat")
            put("headers", buildJsonObject {
                put("accept", "text/event-stream")
                put("content-type", "application/json")
            })
            put("body", json.encodeToString(AgentChatRequest.serializer(), body))
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
