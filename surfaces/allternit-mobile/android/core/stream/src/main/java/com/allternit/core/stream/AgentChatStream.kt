package com.allternit.core.stream

import com.allternit.core.api.ApiCall
import com.allternit.core.api.ApiJson
import com.allternit.core.api.gen.AgentChatRequest
import com.allternit.core.api.gen.AllternitApi
import java.io.IOException
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.callbackFlow
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull
import okhttp3.HttpUrl
import okhttp3.OkHttpClient
import okhttp3.Response
import okhttp3.sse.EventSource
import okhttp3.sse.EventSourceListener
import okhttp3.sse.EventSources

/**
 * One frame of a chat reply, the Kotlin twin of iOS `AgentChatEvent`
 * (Core/API/Models/AgentChatEvent.swift). Frames come from gizzi-code's `/api/agent-chat`
 * or allternit-api's bridge (`gizzi_chat_stream.rs`, `v1_routes.rs`).
 */
sealed interface ChatStreamEvent {
    /** `{"type":"message_start","messageId","modelId","runtimeModelId"}` */
    data class MessageStart(val messageId: String?, val modelId: String?, val runtimeModelId: String?) : ChatStreamEvent

    /** `content_block_delta` with a `text_delta`, or a `chunk_type:"text"` chunk. */
    data class TextDelta(val messageId: String?, val partId: String?, val text: String) : ChatStreamEvent

    /** `content_block_delta` with `{"type":"thinking_delta","thinking"}`. */
    data class ThinkingDelta(val text: String) : ChatStreamEvent

    /** `content_block_start` with a `tool_use` block, or a `chunk_type:"tool_call"` chunk. */
    data class ToolCall(val toolCallId: String, val toolName: String) : ChatStreamEvent

    /**
     * `tool_result`. [frame] is the whole frame: the cards read `metadata` (send_user_message,
     * automation, lookup `metadata.card`, connector_suggest, settings), `result`/`output`
     * (browser_tasks, capture_screen_context) and run_code's top-level `files`/`run`.
     */
    data class ToolResult(val toolCallId: String, val toolName: String, val frame: JsonObject) : ChatStreamEvent

    /** `tool_error` */
    data class ToolError(val toolCallId: String, val toolName: String?, val error: String) : ChatStreamEvent

    /** `artifact` / `artifact.created` / `artifact-created` (needs a kind and a non-empty title). */
    data class Artifact(val artifactId: String, val kind: String, val title: String, val url: String?, val content: String?) :
        ChatStreamEvent

    /** `finish`: the turn ended. [usage] is v1_routes.rs `usage_from_message_info`, when reported. */
    data class Finish(
        val messageId: String?,
        val status: String?,
        val error: String?,
        val errorName: String?,
        val errorMessage: String?,
        val usage: JsonElement?,
    ) : ChatStreamEvent

    /** `{"type":"error","error"}` or a `chunk_type:"error"` chunk. */
    data class StreamError(val message: String) : ChatStreamEvent

    /** `message_stop`, a `chunk_type:"done"` chunk, or `data: [DONE]`. */
    data object Done : ChatStreamEvent

    /** `question.asked`: the agent asks the user ([request] = `properties`, or the frame itself). */
    data class Question(val request: JsonObject) : ChatStreamEvent

    /** `question.replied` / `question.rejected` */
    data class QuestionClosed(val requestId: String) : ChatStreamEvent

    /**
     * The stream could not start or broke. [status] is the HTTP status when there was one
     * (409 + [code] `no_brain`: the computer has no model signed in); null for network errors.
     */
    data class Failed(val status: Int?, val code: String?, val message: String?) : ChatStreamEvent

    val isTerminal: Boolean get() = this is Finish || this === Done || this is Failed
}

/** Frame decoding, shared by the live stream and tests (feed [SseParser] output in). */
object AgentChatFrames {
    /** Decodes one SSE frame; null for frames the app ignores (unknown types, keep-alives). */
    fun decode(event: SseEvent, json: Json = ApiJson.default): ChatStreamEvent? = decode(event.data, json)

    fun decode(data: String, json: Json = ApiJson.default): ChatStreamEvent? {
        val payload = data.trim()
        if (payload.isEmpty()) return null
        if (payload == "[DONE]") return ChatStreamEvent.Done
        val frame = runCatching { json.parseToJsonElement(payload) as? JsonObject }.getOrNull() ?: return null

        frame.str("chunk_type")?.let { return decodeChunk(it, frame["chunk"], json) }

        return when (frame.str("type")) {
            "message_start" -> ChatStreamEvent.MessageStart(frame.str("messageId"), frame.str("modelId"), frame.str("runtimeModelId"))
            "content_block_delta" -> {
                val delta = frame["delta"] as? JsonObject
                val thinking = delta?.str("thinking")
                val text = delta?.str("text")
                when {
                    delta?.str("type") == "thinking_delta" && thinking != null -> ChatStreamEvent.ThinkingDelta(thinking)
                    text != null -> ChatStreamEvent.TextDelta(frame.str("messageId"), frame.str("partId"), text)
                    else -> null
                }
            }
            "content_block_start" -> {
                val block = frame["content_block"] as? JsonObject
                val id = block?.str("id")
                if (block?.str("type") == "tool_use" && id != null) ChatStreamEvent.ToolCall(id, block.str("name") ?: "Tool") else null
            }
            "tool_result" -> ChatStreamEvent.ToolResult(frame.str("toolCallId") ?: "", frame.str("toolName") ?: "Tool", frame)
            "tool_error" -> ChatStreamEvent.ToolError(
                frame.str("toolCallId") ?: "",
                frame.str("toolName"),
                frame.str("error") ?: "Tool execution failed",
            )
            "finish" -> {
                val metadata = frame["metadata"] as? JsonObject
                val details = metadata?.get("errorDetails") as? JsonObject
                ChatStreamEvent.Finish(
                    messageId = frame.str("messageId"),
                    status = frame.str("status"),
                    error = metadata?.str("error"),
                    errorName = details?.str("name"),
                    errorMessage = details?.str("message"),
                    usage = frame["usage"]?.takeUnless { it is JsonNull },
                )
            }
            "message_stop" -> ChatStreamEvent.Done
            "error" -> ChatStreamEvent.StreamError(frame.str("error") ?: "Stream error")
            "artifact", "artifact.created", "artifact-created" -> {
                val kind = frame.str("kind")
                val title = frame.str("title")?.trim()
                val id = frame.str("artifactId") ?: frame.str("id")
                if (kind.isNullOrEmpty() || title.isNullOrEmpty() || id.isNullOrEmpty()) {
                    null
                } else {
                    ChatStreamEvent.Artifact(id, kind, title, frame.str("url"), frame.str("content"))
                }
            }
            "question.asked" -> ChatStreamEvent.Question(frame["properties"] as? JsonObject ?: frame)
            "question.replied", "question.rejected" -> {
                val id = (frame["properties"] as? JsonObject)?.str("requestID") ?: frame.str("requestID")
                id?.let { ChatStreamEvent.QuestionClosed(it) }
            }
            else -> null
        }
    }

    /** BackendChatChunk `{ chunk, chunk_type, session_id }` (web `tryParseChunkPayload`). */
    private fun decodeChunk(type: String, chunk: JsonElement?, json: Json): ChatStreamEvent? {
        val text = (chunk as? JsonPrimitive)?.takeIf { it.isString }?.content
        return when (type) {
            "text" -> text?.let { ChatStreamEvent.TextDelta(null, null, it) }
            "tool_call", "tool_result" -> {
                // The payload is a stringified object; a plain string yields no tool event.
                val payload = text?.let { runCatching { json.parseToJsonElement(it) as? JsonObject }.getOrNull() } ?: return null
                val name = payload.str("toolName")
                val id = payload.str("toolCallId") ?: "${name ?: "tool"}-${System.currentTimeMillis()}"
                if (type == "tool_call") ChatStreamEvent.ToolCall(id, name ?: "Tool") else ChatStreamEvent.ToolResult(id, name ?: "Tool", payload)
            }
            "error" -> ChatStreamEvent.StreamError(text ?: "Stream error")
            "done" -> ChatStreamEvent.Done
            else -> null
        }
    }

    private fun JsonObject.str(key: String): String? = (this[key] as? JsonPrimitive)?.takeIf { it.isString }?.contentOrNull
}

/**
 * Sends a chat turn to the user's connected computer and streams the reply.
 * The request comes from the generated [AllternitApi.agentChatRequest]; okhttp-sse reads the
 * event stream and [AgentChatFrames] decodes each frame. The flow ends after a terminal
 * event; collecting stops are cancellation (the HTTP call is cancelled). Server-side abort
 * stays the caller's job (`POST agent-sessions/{id}/abort`), as on iOS.
 */
class AgentChatStream(
    private val client: OkHttpClient,
    private val api: AllternitApi,
    private val json: Json = ApiJson.default,
) {
    fun send(instanceBaseUrl: HttpUrl, body: AgentChatRequest): Flow<ChatStreamEvent> = callbackFlow {
        val request = api.agentChatRequest(instanceBaseUrl, body)
        val listener = object : EventSourceListener() {
            override fun onEvent(eventSource: EventSource, id: String?, type: String?, data: String) {
                val event = AgentChatFrames.decode(SseEvent(id, type, data), json) ?: return
                trySend(event)
                if (event.isTerminal) {
                    eventSource.cancel()
                    close()
                }
            }

            override fun onClosed(eventSource: EventSource) {
                close()
            }

            override fun onFailure(eventSource: EventSource, t: Throwable?, response: Response?) {
                val failed = when {
                    response != null && !response.isSuccessful -> {
                        val text = runCatching { response.body?.string().orEmpty() }.getOrDefault("")
                        val err = ApiCall.httpError(response.code, text, json)
                        ChatStreamEvent.Failed(err.status, err.code, err.message)
                    }
                    t is IOException || t == null -> ChatStreamEvent.Failed(null, "network", t?.message)
                    // e.g. a 2xx that is not text/event-stream
                    else -> ChatStreamEvent.Failed(response?.code, "bad_stream", t?.message)
                }
                trySend(failed)
                close()
            }
        }
        val source = EventSources.createFactory(client).newEventSource(request, listener)
        awaitClose { source.cancel() }
    }
}
