package com.allternit.feature.chat

import com.allternit.core.api.ApiCall
import com.allternit.core.api.ApiJson
import com.allternit.core.api.ApiResult
import com.allternit.core.api.gen.AgentSession
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.put
import okhttp3.HttpUrl
import okhttp3.OkHttpClient

/** One stored chat in the list (iOS PlanThreadSummary). */
data class PlanThreadSummary(
    val id: String,
    val title: String?,
    val model: String,
    val messageCount: Int,
    val createdAt: String,
    val updatedAt: String,
)

/** One stored turn (iOS PlanThreadMessage). */
data class PlanThreadMessage(val role: String, val content: String, val model: String? = null, val at: String? = null)

data class PlanThreadDetail(val summary: PlanThreadSummary, val messages: List<PlanThreadMessage>)

/**
 * cloud-api's plan chat store (iOS PlanThreadClient, CONTRACT-connections-2026-11 9.10):
 * `POST/GET api/v1/plan-threads`, `GET .../:id`, `POST .../:id/messages`, `DELETE .../:id`.
 * Chats with no computer live here, so they show in history on every device. The shared OkHttp client adds the
 * Allternit bearer. Every failure comes back as null / false; callers fall back to this phone only without
 * telling the person (iOS).
 */
class PlanThreadClient(private val http: OkHttpClient, private val baseUrl: () -> HttpUrl) {
    private val json = ApiJson.default

    private fun url(vararg segments: String): HttpUrl =
        baseUrl().newBuilder().addPathSegments("api/v1/plan-threads").apply { segments.forEach { addPathSegment(it) } }.build()

    suspend fun create(model: String, title: String? = null, messages: List<PlanThreadMessage>? = null): PlanThreadSummary? =
        send("POST", url(), createBody(model, title, messages))?.let { summaryFrom(it) }

    suspend fun list(): List<PlanThreadSummary>? =
        send("GET", url(), null)?.let { listFrom(it) }

    suspend fun get(id: String): PlanThreadDetail? =
        send("GET", url(id), null)?.let { detailFrom(it) }

    suspend fun append(id: String, messages: List<PlanThreadMessage>): PlanThreadSummary? =
        send("POST", url(id, "messages"), appendBody(messages))?.let { summaryFrom(it) }

    /** `DELETE api/v1/plan-threads/:id` (204). A 404 means it is already gone, which is the goal. */
    suspend fun delete(id: String): Boolean {
        val r = ApiCall.executeUnit(http, json, ApiCall.request("DELETE", url(id), null))
        return r is ApiResult.Ok || (r is ApiResult.HttpError && r.status == 404)
    }

    private suspend fun send(method: String, url: HttpUrl, body: JsonObject?): JsonObject? =
        (ApiCall.execute(http, json, ApiCall.request(method, url, body?.toString()), JsonObject.serializer()) as? ApiResult.Ok)?.value

    companion object {
        const val MAX_PER_APPEND = 50
        const val ID_PREFIX = "pth_"

        /**
         * The models the store keeps: plan/..., copilot/..., linked Claude organization chats and Allternit Cloud
         * chats with no computer (`allternit/<alias>`). Not `chatgpt/...` (iOS PlanThreadClient.isStored).
         */
        fun isStored(model: String): Boolean = PersonModels.isPersonOnly(model) || PersonModels.cloudAlias(model) != null

        fun isThreadId(id: String?): Boolean = id != null && id.startsWith(ID_PREFIX)

        /** The first turn names the chat: its first 60 characters (iOS savePlanTurn). */
        fun titleFor(content: String): String {
            val t = content.trim()
            val end = if (t.codePointCount(0, t.length) <= 60) t.length else t.offsetByCodePoints(0, 60)
            return t.substring(0, end)
        }

        fun createBody(model: String, title: String?, messages: List<PlanThreadMessage>?): JsonObject = buildJsonObject {
            put("model", model)
            title?.let { put("title", it) }
            messages?.let { put("messages", messagesJson(it)) }
        }

        fun appendBody(messages: List<PlanThreadMessage>): JsonObject = buildJsonObject {
            put("messages", messagesJson(messages.take(MAX_PER_APPEND)))
        }

        private fun messagesJson(messages: List<PlanThreadMessage>): JsonArray = JsonArray(messages.map { m ->
            buildJsonObject {
                put("role", m.role)
                put("content", m.content)
                m.model?.let { put("model", it) }
                m.at?.let { put("at", it) }
            }
        })

        private fun JsonObject.str(key: String): String? = (this[key] as? JsonPrimitive)?.takeIf { it !is JsonNull }?.contentOrNull

        /** A thread object: `{id, title?, model, message_count, created_at, updated_at}`. Null when a required field is missing. */
        fun summaryFrom(o: JsonObject): PlanThreadSummary? = PlanThreadSummary(
            id = o.str("id") ?: return null,
            title = o.str("title"),
            model = o.str("model") ?: return null,
            messageCount = (o["message_count"] as? JsonPrimitive)?.intOrNull ?: return null,
            createdAt = o.str("created_at") ?: return null,
            updatedAt = o.str("updated_at") ?: return null,
        )

        /** `{ threads: [...] }`. Rows that do not decode are dropped. */
        fun listFrom(o: JsonObject): List<PlanThreadSummary>? =
            (o["threads"] as? JsonArray)?.mapNotNull { (it as? JsonObject)?.let(::summaryFrom) }

        /** A thread with `messages`; a missing list is empty (iOS PlanThreadDetail). */
        fun detailFrom(o: JsonObject): PlanThreadDetail? {
            val summary = summaryFrom(o) ?: return null
            val messages = (o["messages"] as? JsonArray).orEmpty().mapNotNull { el ->
                val m = el as? JsonObject ?: return@mapNotNull null
                PlanThreadMessage(m.str("role") ?: return@mapNotNull null, m.str("content") ?: return@mapNotNull null, m.str("model"), m.str("at"))
            }
            return PlanThreadDetail(summary, messages)
        }
    }
}

/** The chat list row for a thread, an [AgentSession] the existing list shows (iOS PlanThreadSummary.asSession). */
fun PlanThreadSummary.asSession(): AgentSession = AgentSession(
    id = id,
    name = title?.takeIf { it.isNotEmpty() },
    createdAt = createdAt,
    updatedAt = updatedAt,
    messageCount = messageCount.toLong(),
    metadata = JsonObject(mapOf("originSurface" to JsonPrimitive("chat"))) as JsonElement,
)
