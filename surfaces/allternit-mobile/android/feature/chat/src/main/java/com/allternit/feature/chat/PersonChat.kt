package com.allternit.feature.chat

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.flowOn
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.put
import okhttp3.HttpUrl
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody

/**
 * Models a chat sends straight to Allternit Cloud, with no computer (iOS PersonModels + ModelStore.cloudAlias):
 *
 * - `allternit/<alias>`: an Allternit Cloud model. The picker offers these only when no computer catalog is
 *   loaded (ModelStore.chatModels), and they go to cloud-api bare (`glm-5.3`).
 * - `plan/<vendor>/<model>`, `copilot/<model>`, `linked/claude/<model>`: the person's own plan, Copilot or
 *   Claude organization, same route, ids as they are.
 *
 * `chatgpt/…` (iOS sends it from the phone to OpenAI) has no Android sign-in, so it is not a person model here.
 * On-phone brains (`ondevice/…`) run through [OnPhoneChat].
 */
object PersonModels {
    const val CLOUD_PREFIX = "allternit/"
    private val personPrefixes = listOf("plan/", "copilot/", "linked/")

    /** A vendor plan / Copilot / Claude organization model: answers only messages the person sends. */
    fun isPersonOnly(id: String?): Boolean = id != null && personPrefixes.any { id.startsWith(it) }

    /** "allternit/glm-5.3" → "glm-5.3"; null for any other id. */
    fun cloudAlias(id: String?): String? = id?.takeIf { it.startsWith(CLOUD_PREFIX) }?.removePrefix(CLOUD_PREFIX)?.takeIf { it.isNotEmpty() }

    /** The id cloud-api takes: Cloud aliases bare, plan and Copilot ids as they are. */
    fun wireModel(id: String): String = cloudAlias(id) ?: id

    /** The vendor name for messages ("Allternit Cloud", "Claude", "GitHub Copilot"). Same table as iOS. */
    fun vendorName(id: String): String {
        if (id.startsWith(CLOUD_PREFIX)) return "Allternit Cloud"
        if (id.startsWith("copilot/")) return "GitHub Copilot"
        if (id.startsWith("linked/")) return "Claude organization"
        val parts = id.split("/")
        if (parts.size < 2) return "your plan"
        return when (parts[1]) {
            "claude" -> "Claude"
            "chatgpt" -> "ChatGPT"
            "grok" -> "Grok"
            "kimi" -> "Kimi"
            else -> parts[1].replaceFirstChar { it.uppercase() }
        }
    }
}

/** Where a chat send goes (iOS ChatViewModel startStream / routesCloudDirect). */
internal enum class SendRoute { Person, Computer, CheckComputer }

internal object SendRoutes {
    /**
     * - A plan / Copilot / Claude organization model always goes to cloud-api.
     * - An Allternit Cloud model goes to cloud-api unless the chat already lives on a computer (D8) or the
     *   computer's catalog lists it. [computerLists] null = the catalog is unknown here: resolve the relay, and
     *   go to cloud-api when there is no computer.
     * - Anything else goes to a computer.
     */
    fun route(modelId: String?, chatOnComputer: Boolean, computerLists: Boolean?): SendRoute {
        if (PersonModels.isPersonOnly(modelId)) return SendRoute.Person
        if (PersonModels.cloudAlias(modelId) == null || chatOnComputer) return SendRoute.Computer
        return when (computerLists) {
            true -> SendRoute.Computer
            false -> SendRoute.Person
            null -> SendRoute.CheckComputer
        }
    }
}

/**
 * A person-model turn the server refused. Shown under the reply in plain words, never raw JSON
 * (iOS PersonChatRefusal; contract 9.2 / 9.3 codes).
 */
class PersonChatRefusal(val code: String, val detail: String?, val vendor: String) : Exception(code) {
    val plainText: String
        get() = when (code) {
            "credits_required" -> "Allternit Cloud models need credits."
            "credits_exhausted" -> "Your Allternit Cloud credits are used up."
            "plan_person_only" -> "Your $vendor plan only answers messages you send yourself in chat."
            "plan_rate" -> "You've sent a lot of messages to your $vendor plan in the last few minutes. Wait a little and send again."
            "plan_turn_in_flight" -> "Your $vendor plan is still answering another message. Wait for it to finish."
            "plan_not_connected" -> "Your $vendor plan isn't connected. Connect it in Settings › Connections."
            "plan_no_tools" -> "Your $vendor plan answers chat only, without tools."
            "runner_busy" -> "Busy. Try again"
            "could_not_start" -> "Could not start. Try again"
            "login_expired" -> "Your $vendor sign-in expired. Sign in again in Settings › Connections."
            "vendor_limited" -> "$vendor didn't accept this message: your $vendor plan isn't active or its limit is used up. Check your $vendor subscription, or pick another brain."
            "usage_limit" -> "ChatGPT limit reached. It resets on OpenAI's schedule. Try again later, or pick another model."
            "text_only" -> "Your $vendor plan takes text only. Remove the attachment and send again."
            "signed_out_allternit" -> "Sign in to Allternit on this phone to use your $vendor plan."
            "claude_link_not_billable" -> "Your Claude organization can't be billed. It must pay Anthropic directly. Check it in Settings › Connections, or pick another model."
            "claude_link_spend_limit" -> "Your Claude organization's spend limit is reached. Raise it at platform.claude.com/linked-platforms, or pick another model."
            "claude_link_billing_unavailable" -> "Anthropic can't bill your Claude organization right now. Try again later, or pick another model."
            "claude_link_disconnected" -> "Your Claude organization is disconnected. Link it again in Settings › Connections."
            "claude_link_unavailable" -> "Claude organization billing isn't available yet. Pick another model."
            "claude_link_upstream" -> "Anthropic is busy. Try again in a moment."
            "incomplete" -> "$vendor stopped before the reply was finished${detail?.let { " ($it)" } ?: ""}."
            else -> detail?.takeIf { it.isNotEmpty() } ?: "$vendor didn't answer. Try again."
        }

    /** HTTP 402 (D3): the chat opens Cloud credits ([ChatScreen] reads the code from the card). */
    val isCreditsRefusal: Boolean get() = code == "credits_required" || code == "credits_exhausted"

    /** The inline card (iOS PersonChatRefusal.card): a usage limit or lapsed sign-in gets the typed card. */
    fun card(): ChatError {
        val raw = detail?.let { "$code: $it" } ?: code
        val typed = ChatErrorInfo.make(plainText, code).takeIf { it.kind == ChatErrorInfo.Kind.UsageLimit || it.kind == ChatErrorInfo.Kind.SignedOut }
        return ChatError(code, plainText, raw, typed, heading = "$vendor didn't answer")
    }
}

/** One tool call the model asked for (OpenAI `tool_calls`), arguments as the model wrote them. */
data class PersonToolCall(val id: String, val name: String, val arguments: String)

/** What one streamed turn produces. */
sealed interface PersonChatEvent {
    data class Delta(val text: String) : PersonChatEvent
    /** The finished tool calls of this turn, sent once at the end (only when the turn carried `tools`). */
    data class ToolCalls(val calls: List<PersonToolCall>) : PersonChatEvent
    data object Done : PersonChatEvent
}

/** One fragment of a streamed tool call (`choices[0].delta.tool_calls[i]`). */
data class ToolCallPart(val index: Int, val id: String?, val name: String?, val arguments: String?)

/** One `data:` payload of the OpenAI-style chunk stream. */
data class CloudChunk(
    val text: String? = null,
    val toolCalls: List<ToolCallPart> = emptyList(),
    val errorCode: String? = null,
    val errorMessage: String? = null,
)

/**
 * Reads cloud-api's `/v1/chat/completions` SSE lines (iOS PersonChatClient.streamCloud). Text deltas come out at
 * once; tool-call fragments are joined by index and come out at the end. A coded error in the stream throws
 * [PersonChatRefusal]. Not thread-safe: one per turn.
 */
internal class CloudStreamReader(private val vendor: String) {
    private val calls = sortedMapOf<Int, Triple<String, String, StringBuilder>>()
    var finished = false
        private set

    fun line(line: String): List<PersonChatEvent> {
        if (finished || !line.startsWith("data:")) return emptyList()
        val payload = line.substring(5).trim()
        if (payload == "[DONE]") { finished = true; return emptyList() }
        val chunk = parseChunk(payload)
        chunk.errorCode?.let { throw PersonChatRefusal(it, chunk.errorMessage, vendor) }
        for (p in chunk.toolCalls) {
            val cur = calls[p.index] ?: Triple("", "", StringBuilder())
            calls[p.index] = Triple(
                p.id?.takeIf { it.isNotEmpty() } ?: cur.first,
                cur.second + (p.name ?: ""),
                cur.third.append(p.arguments ?: ""),
            )
        }
        return chunk.text?.takeIf { it.isNotEmpty() }?.let { listOf(PersonChatEvent.Delta(it)) } ?: emptyList()
    }

    /** The stream ended: the joined tool calls (if any), then Done. */
    fun end(): List<PersonChatEvent> = buildList {
        val joined = calls.entries.mapIndexedNotNull { i, (_, c) ->
            c.second.takeIf { it.isNotEmpty() }?.let { PersonToolCall(c.first.ifEmpty { "call_$i" }, it, c.third.toString()) }
        }
        if (joined.isNotEmpty()) add(PersonChatEvent.ToolCalls(joined))
        add(PersonChatEvent.Done)
    }

    companion object {
        private val json = Json { ignoreUnknownKeys = true }

        private fun JsonObject.str(key: String): String? = (this[key] as? JsonPrimitive)?.takeIf { it !is JsonNull }?.contentOrNull

        fun parseChunk(payload: String): CloudChunk {
            val o = runCatching { json.parseToJsonElement(payload) as? JsonObject }.getOrNull() ?: return CloudChunk()
            (o["error"] as? JsonObject)?.let { e -> return CloudChunk(errorCode = e.str("code") ?: "failed", errorMessage = e.str("message")) }
            val delta = ((o["choices"] as? JsonArray)?.firstOrNull() as? JsonObject)?.get("delta") as? JsonObject ?: return CloudChunk()
            val parts = (delta["tool_calls"] as? JsonArray).orEmpty().mapIndexedNotNull { offset, el ->
                val c = el as? JsonObject ?: return@mapIndexedNotNull null
                val fn = c["function"] as? JsonObject
                ToolCallPart((c["index"] as? JsonPrimitive)?.intOrNull ?: offset, c.str("id"), fn?.str("name"), fn?.str("arguments"))
            }
            return CloudChunk(text = delta.str("content"), toolCalls = parts)
        }

        /** A non-2xx body `{"error":{"code","message"}}` or a flat `{"error":"code"}` (iOS PersonChatClient.refusal). */
        fun refusal(status: Int, body: String?, vendor: String): PersonChatRefusal {
            val o = body?.let { runCatching { json.parseToJsonElement(it) as? JsonObject }.getOrNull() }
            val e = o?.get("error") as? JsonObject
            var code = e?.str("code") ?: o?.str("error") ?: codeForStatus(status)
            // Any 402 is a credits refusal, even from a server that sends a flat body.
            if (status == 402 && code != "credits_exhausted") code = "credits_required"
            return PersonChatRefusal(code, e?.str("message"), vendor)
        }

        fun codeForStatus(status: Int): String = when (status) {
            401 -> "signed_out_allternit"
            402 -> "credits_required"
            403 -> "plan_person_only"
            429 -> "plan_rate"
            503 -> "runner_busy"
            else -> "failed"
        }
    }
}

/**
 * Sends a no-computer turn to cloud-api `POST /v1/chat/completions` with `X-Allternit-Turn: person` (iOS
 * PersonChatClient.streamCloud; contract 9.3 D7/D8): the whole history every turn, streamed SSE. The shared
 * OkHttp client adds the Allternit bearer. With [tools] (Allternit Cloud models only, contract 7) the turn may end
 * in tool calls; the browser computer loop runs them and sends the next request.
 */
class PersonChatClient(private val http: OkHttpClient, private val baseUrl: () -> HttpUrl) {

    fun stream(model: String, messages: List<JsonObject>, tools: JsonArray? = null, toolChoiceNone: Boolean = false): Flow<PersonChatEvent> = flow {
        val vendor = PersonModels.vendorName(model)
        val request = Request.Builder()
            .url(baseUrl().newBuilder().addPathSegments("v1/chat/completions").build())
            .header("Accept", "text/event-stream")
            // Set only here, on a message the person sent from the composer (D7).
            .header("X-Allternit-Turn", "person")
            .post(body(model, messages, tools, toolChoiceNone).toString().toRequestBody(JSON))
            .build()
        val call = http.newCall(request)
        // Stop cancels the call, so a blocked read ends at once.
        val handle = currentCoroutineContext()[Job]?.invokeOnCompletion { call.cancel() }
        try {
            call.execute().use { r ->
                if (!r.isSuccessful) throw CloudStreamReader.refusal(r.code, r.body.string().take(64 * 1024), vendor)
                val source = r.body.source()
                val reader = CloudStreamReader(vendor)
                while (!reader.finished) {
                    val line = source.readUtf8Line() ?: break
                    for (e in reader.line(line)) emit(e)
                }
                for (e in reader.end()) emit(e)
            }
        } finally {
            handle?.dispose()
        }
    }.flowOn(Dispatchers.IO)

    companion object {
        private val JSON = "application/json; charset=utf-8".toMediaType()

        /** The request body: `{model, messages, stream: true}`, plus `tools` / `tool_choice: "none"` for the tool loop. */
        fun body(model: String, messages: List<JsonObject>, tools: JsonArray? = null, toolChoiceNone: Boolean = false): JsonObject = buildJsonObject {
            put("model", PersonModels.wireModel(model))
            put("messages", JsonArray(messages))
            put("stream", true)
            if (tools != null) {
                put("tools", tools)
                if (toolChoiceNone) put("tool_choice", "none")
            }
        }

        fun text(role: String, content: String): JsonObject = buildJsonObject { put("role", role); put("content", content) }

        /**
         * The history a person model gets each turn (iOS ChatViewModel.personHistory, D8: replayed, no resume): every
         * finished user, assistant and system message, with [text] as the last user message.
         */
        fun history(messages: List<MessageRecord>, text: String): List<JsonObject> {
            val out = messages.filter { it.role in setOf("user", "assistant", "system") && !it.isStreaming && it.error == null && it.content.isNotBlank() }
                .map { text(it.role, it.content) }
                .toMutableList()
            if ((out.lastOrNull()?.get("role") as? JsonPrimitive)?.contentOrNull == "user") out.removeAt(out.size - 1)
            out += text("user", text)
            return out
        }
    }
}
