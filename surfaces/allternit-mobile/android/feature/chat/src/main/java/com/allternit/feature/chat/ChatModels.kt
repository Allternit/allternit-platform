package com.allternit.feature.chat

import com.allternit.core.api.gen.AgentSessionMessage
import java.time.OffsetDateTime
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.longOrNull

/**
 * A staged composer attachment handed to the chat on send (the shell's ComposerAttachment).
 * Photos and files carry a content [uri]; the camera carries JPEG [bytes]; a GitHub link
 * carries its URL in [uri] with [kind] "github".
 */
data class ChatAttachmentInput(
    val name: String,
    val uri: String?,
    val kind: String,
    val bytes: ByteArray? = null,
    val mediaType: String? = null,
)

/** One composer send (iOS ChatView onSend → ChatViewModel.sendMessage). */
data class ChatSendRequest(
    val text: String,
    val attachments: List<ChatAttachmentInput> = emptyList(),
    /** The composer pill's catalog id ("provider/model"); null lets the backend pick. */
    val runtimeModelId: String? = null,
    /** ModelStore.effortForSend: only for models that take an effort level. */
    val effort: String? = null,
    /** The deck's armed Agent mode id (`metadata.agentModeId`). */
    val agentModeId: String? = null,
    /** + sheet tool toggles (iOS ToolOptionsStore.optionsForSend). */
    val webSearch: Boolean = false,
    val research: Boolean = false,
    /** Standing instructions for this turn (agent-chat `systemPrompt`, iOS AgentChatRequest). Null falls back to [ChatViewModel.systemPromptFor]. */
    val systemPrompt: String? = null,
    /** Explicit Image deck action, never inferred from text. */
    val browserImage: Boolean = false,
    /** Set only by a foreground composer; helpers and routines leave this false. */
    val personTurn: Boolean = false,
    /** Raw composer text of an Image or Artifact mode send (`metadata.image_prompt`); person sends only. */
    val imagePrompt: String? = null,
)

/** iOS ToolCallRecord.State / MessageRecord.ToolStatus.State. */
enum class ToolState { Running, Done, Failed }

/**
 * One tool call of a reply (iOS ToolCallRecord). [metadata], [output], [files] and [run] are
 * what the cards read: lookup `metadata.card`, connector_suggest / settings `metadata` or the
 * output text, run_code's top-level `files` / `run`.
 */
data class ToolCallRecord(
    val id: String,
    val toolName: String,
    val state: ToolState,
    val title: String? = null,
    val error: String? = null,
    val output: String? = null,
    val input: JsonObject? = null,
    val metadata: JsonObject? = null,
    val files: JsonElement? = null,
    val run: JsonObject? = null,
    val startedAt: Long? = null,
    val finishedAt: Long? = null,
    /** Local validated ACI result retained when the runtime later replaces output text. */
    val browserImageResult: String? = null,
    /** Engine picture or GIF folded onto this call from an assistant file part with source `tool-call://<id>`: a data URL, up to 8 MiB. */
    val attachedImageUrl: String? = null,
    val attachedImageMime: String? = null,
)

/** The single status line under a reply (iOS MessageRecord.ToolStatus). */
data class ToolStatus(
    val toolName: String,
    val text: String,
    val state: ToolState,
    val startedAt: Long? = null,
    val elapsedMs: Long? = null,
)

/** Client wall-clock readout (iOS PerfStats): chars/4 is an estimate, shown with "~". */
data class PerfStats(val timeToFirstTokenMs: Long, val generationMs: Long, val approxTokens: Int) {
    val tokensPerSecond: Double get() = if (generationMs > 0) approxTokens / (generationMs / 1000.0) else 0.0
}

/** A typed artifact attached to a reply (iOS ArtifactRecord, from frames or `file` parts). */
data class ArtifactRecord(val id: String, val kind: String, val title: String, val url: String?, val content: String?)

/**
 * A permission the agent waits on (iOS AgentPermissionAsk). [id] is the request id the reply
 * goes to; [permission] the wire type ("bash", "edit", ...); [patterns] what it would touch.
 */
data class PermissionAsk(
    val id: String,
    val sessionId: String,
    val permission: String,
    val title: String,
    val patterns: List<String>,
    val answered: String? = null,
)

/** A question the agent asked (`question.asked`), answered in place. */
data class QuestionAsk(val id: String, val sessionId: String?, val questions: List<QuestionItem>, val closed: Boolean = false)

data class QuestionItem(val header: String?, val question: String, val options: List<String>, val multiple: Boolean, val plan: PlanDraft? = null)

/** A plan the agent wants approved (gizzi `plan_exit`; web `Question.plan`): steps, the body as Markdown, and the saved plan artifact. */
data class PlanDraft(val title: String, val body: String, val steps: List<PlanStep>, val artifactId: String? = null, val artifactVersion: Int = 1)

data class PlanStep(val title: String, val detail: String? = null)

/** Reads `question.plan`; null when it has no steps (the question then draws as a normal card). */
fun parsePlanDraft(q: JsonObject): PlanDraft? {
    val raw = q["plan"] as? JsonObject ?: return null
    val steps = (raw["steps"] as? JsonArray).orEmpty().mapNotNull { el ->
        val o = el as? JsonObject
        val t = o?.str("title") ?: (el as? JsonPrimitive)?.takeIf { it.isString }?.contentOrNull
        t?.trim()?.takeIf { it.isNotEmpty() }?.let { PlanStep(it, o?.str("detail")?.trim()?.takeIf { d -> d.isNotEmpty() }) }
    }
    if (steps.isEmpty()) return null
    val art = raw["artifact"] as? JsonObject
    val title = raw.str("title")?.trim()?.takeIf { it.isNotEmpty() } ?: q.str("header")?.takeIf { it.isNotBlank() } ?: "Plan"
    return PlanDraft(title, raw.str("body").orEmpty(), steps, art?.str("id"), (art?.get("version") as? JsonPrimitive)?.intOrNull ?: 1)
}

/**
 * The inline error card's data (iOS ChatError, ChatErrorCardView): built from a finish frame
 * with status "error", a stream error, or a transport failure.
 */
data class ChatError(val name: String?, val message: String?, val raw: String, val typed: ChatErrorInfo? = null) {
    val isModelError: Boolean get() = name == NO_BRAIN || name == "ProviderModelNotFoundError"

    /**
     * The `claude_link_*` code of a Claude organization refusal (iOS ChatErrorAction.forClaudeLink), from the
     * error name or the refusal body `{"error":{"code","message"},"fallbacks":[...]}` (HTTP error or the one
     * mid-stream `data:` frame). Null for every other error.
     */
    val claudeLinkCode: String?
        get() = name?.takeIf { it.startsWith("claude_link_") }
            ?: CLAUDE_CODE.find("$message $raw")?.groupValues?.get(1)

    /** The server's own sentence from a refusal body, when there is one. */
    private val refusalMessage: String?
        get() = if (claudeLinkCode == null) null else REFUSAL_MESSAGE.find(raw)?.groupValues?.get(1)?.takeIf { it.isNotBlank() }

    /** Use credits, Link again (disconnected only), Manage Claude organization. Empty for other errors. */
    val claudeActions: List<ChatErrorAction>
        get() {
            val code = claudeLinkCode ?: return emptyList()
            return buildList {
                add(ChatErrorAction.UseCredits)
                if (code == "claude_link_disconnected") add(ChatErrorAction.LinkAgain)
                add(ChatErrorAction.ManageClaudeOrg)
            }
        }

    val title: String
        get() = when (name) {
            NO_BRAIN -> "Pick a brain first."
            "ProviderModelNotFoundError" -> "Model not available"
            else -> "Something went wrong"
        }

    val detail: String
        get() = when (name) {
            NO_BRAIN -> "Choose the brain this chat runs on, then send again."
            "ProviderModelNotFoundError" -> "The model for this chat couldn't be found. Pick another model and try again."
            else -> refusalMessage ?: message?.trim()?.takeIf { it.isNotEmpty() } ?: "The response failed before it could complete."
        }

    companion object {
        const val NO_BRAIN = "no_brain"
        private val CLAUDE_CODE = Regex("\"?(claude_link_[a-z_]+)\"?")
        private val REFUSAL_MESSAGE = Regex("\"message\"\\s*:\\s*\"((?:[^\"\\\\]|\\\\.)*)\"")
        /** The API's streamed no_brain error text (iOS AgentChatClientError.noBrainMessage). */
        const val NO_BRAIN_MESSAGE = "Pick a brain first."

        fun connectionInterrupted(description: String) = ChatError(null, description, description)
    }
}

/** A button a Claude organization refusal adds to the error card (iOS ChatErrorAction). The chat decides what each does. */
enum class ChatErrorAction(val title: String, val tag: String) {
    UseCredits("Use Allternit credits for this message", "error-use-credits"),
    LinkAgain("Link again", "error-link-again"),
    ManageClaudeOrg("Manage Claude organization", "error-manage-claude-org"),
}

/** One feed row (iOS MessageRecord). */
data class MessageRecord(
    val id: String,
    val role: String,
    val content: String,
    val reasoning: String = "",
    val isStreaming: Boolean = false,
    val toolStatus: ToolStatus? = null,
    val toolCalls: List<ToolCallRecord> = emptyList(),
    val questions: List<QuestionAsk> = emptyList(),
    val permissions: List<PermissionAsk> = emptyList(),
    /** MCP task and question ids from this reply; the latest state lives in [McpTasksStore] (drawn only with the `mcp.tasks` flag). */
    val mcpTaskIds: List<String> = emptyList(),
    val mcpFormIds: List<String> = emptyList(),
    val artifacts: List<ArtifactRecord> = emptyList(),
    val error: ChatError? = null,
    val perfStats: PerfStats? = null,
    /** Names of the files sent with a user turn (shown as chips under the bubble). */
    val attachmentNames: List<String> = emptyList(),
    val createdAt: Long = System.currentTimeMillis(),
    /** Wave H2: the feed's "Voice chat ended" card (iOS MessageRecord.voiceSummary); a local system row. */
    val voiceSummary: com.allternit.feature.chat.voice.VoiceSummary? = null,
) {
    /** Folds an engine picture (`tool-call://<id>` file part) onto its call; false when no such call is here yet. */
    fun attachToolImage(sourceUri: String?, url: String?, mime: String?): MessageRecord? {
        val callId = sourceUri?.takeIf { it.startsWith("tool-call://") }?.removePrefix("tool-call://") ?: return null
        if (url.isNullOrEmpty()) return null
        val index = toolCalls.indexOfFirst { it.id == callId }
        if (index < 0) return null
        val kind = mime?.takeIf { it.startsWith("image/") } ?: url.substringAfter("data:", "").substringBefore(';').takeIf { it.startsWith("image/") } ?: "image/png"
        return copy(toolCalls = toolCalls.toMutableList().also { it[index] = it[index].copy(attachedImageUrl = url, attachedImageMime = kind) })
    }

    /** Inserts or updates one tool call from a stream frame (iOS upsertToolCall). */
    fun upsertToolCall(call: ToolCallRecord): MessageRecord {
        val index = toolCalls.indexOfFirst { it.id == call.id }
        if (index < 0) return copy(toolCalls = toolCalls + call)
        val old = toolCalls[index]
        val merged = old.copy(
            toolName = call.toolName.ifEmpty { old.toolName },
            state = call.state,
            title = call.title ?: old.title,
            error = call.error ?: old.error,
            output = call.output ?: old.output,
            input = call.input?.takeIf { it.isNotEmpty() } ?: old.input,
            metadata = call.metadata ?: old.metadata,
            files = call.files ?: old.files,
            run = call.run ?: old.run,
            startedAt = old.startedAt ?: call.startedAt,
            finishedAt = call.finishedAt ?: old.finishedAt,
            browserImageResult = call.browserImageResult ?: old.browserImageResult,
            attachedImageUrl = call.attachedImageUrl ?: old.attachedImageUrl,
            attachedImageMime = call.attachedImageMime ?: old.attachedImageMime,
        )
        return copy(toolCalls = toolCalls.toMutableList().also { it[index] = merged })
    }

    companion object {
        /**
         * A wire message → a feed row (iOS ChatViewModel.mapRecord): content and thinking as-is,
         * tool calls from `metadata.parts` (gizzi tool parts), and file parts as artifact cards.
         */
        fun fromWire(record: AgentSessionMessage): MessageRecord {
            val parts = ((record.metadata as? JsonObject)?.get("parts") as? JsonArray).orEmpty().mapNotNull { it as? JsonObject }
            val tools = parts.mapIndexedNotNull { index, part ->
                if (part.str("type") != "tool") return@mapIndexedNotNull null
                val name = part.str("tool")?.takeIf { it.isNotEmpty() } ?: return@mapIndexedNotNull null
                val state = part.obj("state")
                val time = state?.obj("time")
                ToolCallRecord(
                    id = part.str("callID") ?: part.str("id") ?: "$name-$index",
                    toolName = name,
                    state = when (state?.str("status")) {
                        "error" -> ToolState.Failed
                        "pending", "running" -> ToolState.Running
                        else -> ToolState.Done
                    },
                    title = state?.str("title"),
                    error = state?.str("error"),
                    output = state?.get("output")?.asText(),
                    input = state?.obj("input"),
                    metadata = state?.obj("metadata") ?: part.obj("metadata"),
                    files = part["files"] ?: state?.get("files"),
                    run = part.obj("run") ?: state?.obj("run"),
                    startedAt = time?.long("start"),
                    finishedAt = time?.long("end"),
                )
            }
            val attached = HashMap<String, Pair<String, String?>>()
            val artifacts = parts.filter { it.str("type") == "file" }.filter { part ->
                val src = part.obj("source")?.str("uri") ?: part.str("sourceUri")
                val url = part.str("url")
                if (src != null && src.startsWith("tool-call://") && !url.isNullOrEmpty() && tools.any { it.id == src.removePrefix("tool-call://") }) {
                    attached[src.removePrefix("tool-call://")] = url to (part.str("mime") ?: part.str("mimeType"))
                    false
                } else true
            }.map { part ->
                val url = part.str("url")
                val title = part.str("filename") ?: url?.substringAfterLast('/') ?: "Attachment"
                val ext = title.substringAfterLast('.', "").lowercase().ifEmpty { "text" }
                ArtifactRecord("${record.id}-$title-${url.orEmpty()}", ext, title, url, null)
            }
            return MessageRecord(
                id = record.id,
                role = record.role,
                content = if (tools.isEmpty()) record.content else stripToolMarkers(record.content),
                reasoning = record.thinking.orEmpty(),
                toolCalls = tools.map { t -> attached[t.id]?.let { (u, m) -> t.copy(attachedImageUrl = u, attachedImageMime = m?.takeIf { it.startsWith("image/") } ?: "image/png") } ?: t },
                artifacts = if (record.role == "user") emptyList() else artifacts,
                attachmentNames = if (record.role == "user") artifacts.map { it.title } else emptyList(),
                createdAt = parseMillis(record.timestamp) ?: System.currentTimeMillis(),
            )
        }

        /** The gateway's `[Tool name]` placeholder lines duplicate the tool chip rendered from parts; drop them. */
        private val toolMarker = Regex("""^[ \t]*\[Tool [^\]\n]*\][ \t]*\r?\n?""", RegexOption.MULTILINE)
        private fun stripToolMarkers(text: String): String = text.replace(toolMarker, "").trim()

        private fun parseMillis(value: String?): Long? =
            value?.let { runCatching { OffsetDateTime.parse(it).toInstant().toEpochMilli() }.getOrNull() }
    }
}

// MARK: - JSON helpers (the cards read loosely typed tool metadata)

internal fun JsonObject.str(key: String): String? = (this[key] as? JsonPrimitive)?.takeIf { it.isString }?.contentOrNull

internal fun JsonObject.obj(key: String): JsonObject? = this[key] as? JsonObject

internal fun JsonObject.arr(key: String): List<JsonElement> = (this[key] as? JsonArray).orEmpty()

internal fun JsonObject.long(key: String): Long? = (this[key] as? JsonPrimitive)?.longOrNull

internal fun JsonObject.double(key: String): Double? = (this[key] as? JsonPrimitive)?.doubleOrNull

internal fun JsonObject.bool(key: String): Boolean? = (this[key] as? JsonPrimitive)?.booleanOrNull

/** Any scalar as display text: strings as-is, numbers and booleans by their JSON text. */
internal fun JsonObject.text(key: String): String? = when (val v = this[key]) {
    null, JsonNull -> null
    is JsonPrimitive -> v.contentOrNull
    else -> v.toString()
}

/** A tool output as text: a JSON string's content, else the JSON itself. */
internal fun JsonElement.asText(): String? = when (this) {
    JsonNull -> null
    is JsonPrimitive -> if (isString) content else toString()
    else -> toString()
}
