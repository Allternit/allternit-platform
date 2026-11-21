package com.allternit.feature.chat

import android.content.ContentResolver
import android.net.Uri
import android.webkit.MimeTypeMap
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.allternit.core.api.ApiResult
import com.allternit.core.api.gen.AgentChatMetadata
import com.allternit.core.api.gen.AgentChatRequest
import com.allternit.core.api.gen.AttachmentRef
import com.allternit.core.design.FriendlyErrorMessage
import com.allternit.core.stream.ChatStreamEvent
import java.io.IOException
import java.util.TimeZone
import java.util.UUID
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import okhttp3.Request

/**
 * Parameters for the NEXT session create (iOS SessionContext). Composer-level, so they only
 * affect sessions created after a change.
 */
data class SessionContext(
    val originSurface: String = "chat",
    val sessionMode: String = "regular",
    val agentId: String? = null,
    val agentName: String? = null,
    val agentModeId: String? = null,
    val permissionMode: String? = null,
    /**
     * Harness & Bots 2.0: a plain chat started on another runtime (Claude Code, Codex…) sends the
     * normalized `botBrain` (runtime, mode, harness) as `metadata.botBrain`; null on Allternit.
     */
    val botBrain: JsonObject? = null,
)

/**
 * Why a send could not reach a computer. Never a raw error: the feed shows the compute gate
 * (iOS ComputeGateView / ConnectComputerSheet) with a path to connect, and the draft goes back
 * to the composer.
 */
enum class ComputeGateReason {
    /** No computer is paired, or cloud-api refused for compute (428, compute_required). */
    NoComputer,
    /** Computers exist but none answers. */
    Unreachable,
}

/**
 * The chat's state and actions, a port of iOS ChatViewModel (Features/Chat/ViewModels).
 * Compose state, so the screen recomposes on change; one instance lives with the shell so a
 * running reply survives a trip to the home and back. [dispose] cancels everything.
 */
class ChatViewModel(
    private val backend: ChatBackend,
    private val contentResolver: ContentResolver,
) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    var messages by mutableStateOf<List<MessageRecord>>(emptyList())
        private set
    var isStreaming by mutableStateOf(false)
        private set
    /** True only during the first message's session-create; send is disabled meanwhile. */
    var isCreatingSession by mutableStateOf(false)
        private set
    var currentSessionId by mutableStateOf<String?>(null)
        private set
    /** Dismissible banner for load/send failures (iOS transientError). */
    var transientError by mutableStateOf<String?>(null)
    /** A failed send's text handed back to the composer (iOS draftToRestore). */
    var draftToRestore by mutableStateOf<String?>(null)
    /** A send came back `no_brain`: the screen opens the model picker. */
    var needsBrainPick by mutableStateOf(false)
    /** Set when a send could not reach a computer: the feed shows the compute gate. */
    var computeGate by mutableStateOf<ComputeGateReason?>(null)
    /** User message id → earlier versions (the "Edited" chip). */
    var editedVersions by mutableStateOf<Map<String, List<MessageBranch>>>(emptyMap())
        private set
    var isLoadingHistory by mutableStateOf(false)
        private set
    /** Called after a session is created or deleted, so the drawer's Recents reload. */
    var onSessionsChanged: () -> Unit = {}

    var sessionContext = SessionContext()

    private var streamJob: Job? = null
    private var permissionJob: Job? = null
    private var streamingMessageId: String? = null
    private var pendingText = StringBuilder()
    private var pendingReasoning = StringBuilder()
    private var flushJob: Job? = null
    private var requestStartedAt: Long? = null
    private var firstDeltaAt: Long? = null
    private var reloadBranchesAfterStream = false
    private var branches: List<MessageBranch> = emptyList()

    private data class LastSent(val text: String, val runtimeModelId: String?, val effort: String?, val refs: List<AttachmentRef>, val tools: JsonObject?)
    private var lastSent: LastSent? = null

    val lastAssistantMessageId: String? get() = messages.lastOrNull { it.role == "assistant" }?.id
    val lastUserMessageId: String? get() = messages.lastOrNull { it.role == "user" }?.id

    fun dispose() {
        scope.cancel()
    }

    // MARK: Sessions

    /**
     * Standing instructions for a session (iOS ArtifactStudioLauncher.systemPrompt(for:)), read on
     * every send, retry and regenerate. The shell sets it; null sends none.
     */
    var systemPromptFor: ((sessionId: String) -> String?)? = null

    fun startNewSession() {
        stopStreaming()
        transientError = null
        computeGate = null
        messages = emptyList()
        branches = emptyList()
        editedVersions = emptyMap()
        currentSessionId = null
    }

    /** Opens a history chat: `GET agent-sessions/:id/messages`, then its branches. */
    fun loadSession(sessionId: String) {
        if (sessionId == currentSessionId && (messages.isNotEmpty() || isStreaming)) return
        stopStreaming()
        transientError = null
        computeGate = null
        currentSessionId = sessionId
        messages = emptyList()
        branches = emptyList()
        editedVersions = emptyMap()
        isLoadingHistory = true
        scope.launch {
            val r = backend.listMessages(sessionId)
            if (currentSessionId != sessionId) return@launch
            isLoadingHistory = false
            when (r) {
                is ApiResult.Ok -> {
                    messages = r.value.map(MessageRecord::fromWire)
                    loadBranches()
                    refreshPermissions()
                }
                else -> if (r is ApiResult.HttpError && ChatErrors.isComputeGate(r.status, r.code, r.message)) {
                    computeGate = ComputeGateReason.NoComputer
                } else {
                    transientError = "Couldn't load the conversation: ${describe(r)}"
                }
            }
        }
    }

    /** `DELETE agent-sessions/:id`, then a fresh chat (the ⋯ menu's Delete chat). */
    fun deleteCurrentSession(onDone: () -> Unit) {
        val id = currentSessionId ?: return
        scope.launch {
            when (val r = backend.deleteSession(id)) {
                is ApiResult.Ok -> {
                    startNewSession()
                    onSessionsChanged()
                    onDone()
                }
                else -> transientError = "Couldn't delete this chat: ${describe(r)}"
            }
        }
    }

    // MARK: Sending

    /** The composer's send (iOS sendMessage): optimistic user bubble, then [startStream]. */
    fun send(request: ChatSendRequest) {
        val trimmed = request.text.trim()
        if (trimmed.isEmpty() || isStreaming || isCreatingSession) return
        val userId = UUID.randomUUID().toString()
        messages = messages + MessageRecord(id = userId, role = "user", content = trimmed, attachmentNames = request.attachments.map { it.name })
        startStream(
            text = enrichedText(trimmed, request.webSearch),
            runtimeModelId = request.runtimeModelId,
            effort = request.effort,
            tools = toolOptions(request),
            staged = request.attachments,
            preparedRefs = null,
            userMessageId = userId,
            agentModeId = request.agentModeId,
            systemPrompt = request.systemPrompt,
        )
    }

    /** Error-card Retry: drop the failed reply and re-stream the last text (no second upload). */
    fun retryFailed(assistantId: String, runtimeModelId: String?, effort: String?) {
        val last = lastSent ?: return
        if (isStreaming || isCreatingSession) return
        messages = messages.filterNot { it.id == assistantId }
        startStream(last.text, runtimeModelId ?: last.runtimeModelId, effort ?: last.effort, last.tools, emptyList(), last.refs, null, null)
    }

    /** Action-bar Retry on the last good reply (iOS regenerateLastResponse). */
    fun regenerate(runtimeModelId: String?, effort: String?) {
        if (isStreaming || isCreatingSession) return
        val index = messages.indexOfLast { it.role == "assistant" }
        if (index < 0) return
        val lastUserText = messages.subList(0, index).lastOrNull { it.role == "user" }?.content
        val text = lastSent?.text ?: lastUserText
        if (text.isNullOrEmpty()) return
        val refs = if (lastSent?.text == text) lastSent?.refs.orEmpty() else emptyList()
        messages = messages.toMutableList().also { it.removeAt(index) }
        startStream(text, runtimeModelId ?: lastSent?.runtimeModelId, effort ?: lastSent?.effort, lastSent?.tools, emptyList(), refs, null, null)
    }

    /**
     * Edit-resend of the last user message (iOS resendEditedMessage): revert the server
     * conversation to it (`POST .../revert`), cut the feed, send the new text. When the revert
     * can't be done nothing is resent, so the old turn is never duplicated.
     */
    fun resendEdited(messageId: String, request: ChatSendRequest) {
        if (isStreaming || isCreatingSession) return
        val index = messages.indexOfFirst { it.id == messageId }
        if (index < 0) return
        val sessionId = currentSessionId
        if (sessionId == null) {
            transientError = "Couldn't edit this message: the chat isn't saved yet."
            return
        }
        isCreatingSession = true
        scope.launch {
            val serverId = serverMessageId(messageId, index, sessionId)
            val reverted = serverId?.let { backend.revert(sessionId, it) }
            isCreatingSession = false
            if (serverId == null || reverted !is ApiResult.Ok) {
                val why = when {
                    serverId == null -> "This message hasn't synced yet. Try again in a moment."
                    else -> describe(reverted!!)
                }
                transientError = "Couldn't edit this message: $why Your edit was not sent."
                draftToRestore = request.text
                return@launch
            }
            if (currentSessionId != sessionId) return@launch
            val current = messages.indexOfFirst { it.id == messageId }
            if (current >= 0) messages = messages.subList(0, current)
            reloadBranchesAfterStream = true
            send(request)
        }
    }

    /** A local bubble id → its server id, by position among the transcript's user messages. */
    private suspend fun serverMessageId(messageId: String, localIndex: Int, sessionId: String): String? {
        if (messageId.startsWith("msg_")) return messageId
        val ordinal = messages.subList(0, localIndex).count { it.role == "user" }
        val records = (backend.listMessages(sessionId) as? ApiResult.Ok)?.value ?: return null
        return records.filter { it.role == "user" }.getOrNull(ordinal)?.id
    }

    suspend fun loadBranches() {
        val sessionId = currentSessionId ?: return
        val loaded = backend.branches(sessionId)
        if (currentSessionId != sessionId) return
        branches = loaded
        remapEditedVersions()
    }

    /** Branch rows that name a feed message attach to it; the rest attach to the last user turn. */
    private fun remapEditedVersions() {
        if (branches.isEmpty()) {
            editedVersions = emptyMap()
            return
        }
        val users = messages.filter { it.role == "user" }
        val map = mutableMapOf<String, MutableList<MessageBranch>>()
        branches.filter { it.role == "user" }.forEach { b ->
            val target = users.firstOrNull { it.id == b.messageId }?.id
                ?: users.firstOrNull { u -> (b.createdAt ?: 0L) <= u.createdAt }?.id
                ?: users.lastOrNull()?.id
                ?: return@forEach
            map.getOrPut(target) { mutableListOf() } += b
        }
        editedVersions = map.mapValues { (_, v) -> v.sortedByDescending { it.createdAt ?: 0L } }
    }

    // MARK: Stream

    private fun startStream(
        text: String,
        runtimeModelId: String?,
        effort: String?,
        tools: JsonObject?,
        staged: List<ChatAttachmentInput>,
        preparedRefs: List<AttachmentRef>?,
        userMessageId: String?,
        agentModeId: String?,
        systemPrompt: String? = null,
    ) {
        transientError = null
        draftToRestore = null
        computeGate = null
        val assistantId = UUID.randomUUID().toString()
        messages = messages + MessageRecord(id = assistantId, role = "assistant", content = "", isStreaming = true)
        isStreaming = true
        streamingMessageId = assistantId
        pendingText = StringBuilder()
        pendingReasoning = StringBuilder()

        streamJob = scope.launch {
            var failed = false
            isCreatingSession = currentSessionId == null
            try {
                // Upload staged files first. A failed upload fails the send: never drop the files
                // and send a text-only message the person didn't ask for.
                val refs = preparedRefs?.toMutableList() ?: mutableListOf()
                if (preparedRefs == null) {
                    for (a in staged) refs += uploadOne(a)
                }
                lastSent = LastSent(text, runtimeModelId, effort, refs, tools)

                // Gap run GS-D: a brain on this phone (iOS OnDeviceChat) needs no computer and no session.
                OnPhoneChat.keyOf(runtimeModelId)?.let { key ->
                    isCreatingSession = false
                    requestStartedAt = System.currentTimeMillis()
                    firstDeltaAt = null
                    val roles = mutableListOf<String>()
                    val contents = mutableListOf<String>()
                    systemPrompt?.takeIf { it.isNotBlank() }?.let { roles += "system"; contents += it }
                    messages.filter { it.id != assistantId && it.content.isNotBlank() && (it.role == "user" || it.role == "assistant") }
                        .forEach { roles += it.role; contents += it.content }
                    OnPhoneChat.stream(key, roles, contents).collect { event -> apply(event, assistantId) }
                    finishStreaming(assistantId)
                    return@launch
                }
                // Find the computer before creating the session, so a no-computer send leaves
                // no empty chat behind.
                val runtimeId = when (val pick = backend.relay.resolve()) {
                    is RelayPick.Computer -> pick.runtimeId
                    RelayPick.NoComputer -> throw GateException(ComputeGateReason.NoComputer)
                    RelayPick.Unreachable -> throw GateException(ComputeGateReason.Unreachable)
                    is RelayPick.Failed -> if (pick.gate) throw GateException(ComputeGateReason.NoComputer) else throw SendException(pick.message)
                }
                val sessionId = ensureSessionId(agentModeId)
                isCreatingSession = false
                requestStartedAt = System.currentTimeMillis()
                firstDeltaAt = null
                startPermissionPolling(sessionId)

                val body = AgentChatRequest(
                    chatId = sessionId,
                    message = text,
                    agentId = sessionContext.agentId,
                    // iOS ChatViewModel: the turn's own instructions, else the session's standing
                    // ones (a studio session's artifact note), blank → none.
                    systemPrompt = (systemPrompt ?: systemPromptFor?.invoke(sessionId))?.takeIf { it.isNotBlank() },
                    runtimeModelId = runtimeModelId,
                    effort = effort,
                    attachments = refs.ifEmpty { null },
                    metadata = AgentChatMetadata(
                        tools = tools,
                        formFactor = "phone",
                        timeZone = TimeZone.getDefault().id,
                        screenContext = "available",
                        answerCards = "widgets",
                    ),
                )
                failed = runStream(runtimeId, body, assistantId)
                finishStreaming(assistantId)
            } catch (e: CancellationException) {
                isCreatingSession = false
                flushPending()
                throw e
            } catch (e: Throwable) {
                isCreatingSession = false
                failed = true
                if (isPristine(assistantId)) {
                    // Nothing streamed back: roll back the optimistic pair, hand the draft back,
                    // and say why once.
                    messages = messages.filterNot { it.id == assistantId || it.id == userMessageId }
                    if (userMessageId != null) draftToRestore = strippedText(text)
                    when (e) {
                        is GateException -> computeGate = e.reason
                        is NoBrainException -> {
                            transientError = ChatError.NO_BRAIN_MESSAGE
                            needsBrainPick = true
                        }
                        else -> transientError = "Couldn't send the message: ${e.message?.let { FriendlyErrorMessage.from(it) } ?: "Something went wrong. Try again."}"
                    }
                    finishStreaming(assistantId)
                } else {
                    flushPending()
                    update(assistantId) { it.copy(error = ChatError.connectionInterrupted(e.message ?: "The connection dropped.")) }
                    finishStreaming(assistantId)
                }
            }
        }
    }

    private class GateException(val reason: ComputeGateReason) : Exception()
    private class NoBrainException : Exception(ChatError.NO_BRAIN_MESSAGE)
    private class SendException(message: String) : Exception(message)

    /**
     * Collects one relayed stream. Waits out a waking cloud computer (503 `runtime_warming`,
     * up to 120 s, iOS RuntimeRelayClient.wakeTimeout) as long as nothing has arrived yet.
     * Returns true when the reply failed (error frame / finish "error").
     */
    private suspend fun runStream(runtimeId: String, body: AgentChatRequest, assistantId: String): Boolean {
        val deadline = System.currentTimeMillis() + 120_000
        while (true) {
            var failedFrame = false
            var transport: ChatStreamEvent.Failed? = null
            backend.streamTurn(runtimeId, body).collect { event ->
                if (event is ChatStreamEvent.Failed) transport = event else if (apply(event, assistantId)) failedFrame = true
            }
            val t = transport ?: run {
                backend.relay.markReachable(runtimeId)
                return failedFrame
            }
            when {
                t.status == 503 && t.code == "runtime_warming" && isPristine(assistantId) && System.currentTimeMillis() < deadline -> {
                    delay(3_000)
                    continue
                }
                t.status == 503 && (t.code == "runtime_offline" || t.code == "runtime_unavailable" || t.code == "runtime_warming") -> {
                    backend.relay.markUnreachable(runtimeId)
                    throw GateException(ComputeGateReason.Unreachable)
                }
                ChatErrors.isComputeGate(t.status, t.code, t.message) -> throw GateException(ComputeGateReason.NoComputer)
                t.status == 409 && t.code == "no_brain" -> throw NoBrainException()
                t.status == null -> throw IOException(t.message ?: "Could not connect to the server. Check your connection and try again.")
                else -> throw SendException(t.message ?: t.status?.let { ChatErrors.statusSentence(it) } ?: "Something went wrong. Try again.")
            }
        }
    }

    /** Applies one frame to the streaming reply (iOS apply). True when the reply failed. */
    private fun apply(event: ChatStreamEvent, id: String): Boolean {
        when (event) {
            is ChatStreamEvent.TextDelta -> {
                if (firstDeltaAt == null) firstDeltaAt = System.currentTimeMillis()
                pendingText.append(event.text)
                scheduleFlush()
            }
            is ChatStreamEvent.ThinkingDelta -> {
                pendingReasoning.append(event.text)
                scheduleFlush()
            }
            is ChatStreamEvent.ToolCall -> update(id) {
                it.copy(toolStatus = ToolStatus(event.toolName, event.toolName, ToolState.Running, System.currentTimeMillis()))
                    .upsertToolCall(ToolCallRecord(event.toolCallId, event.toolName, ToolState.Running, startedAt = System.currentTimeMillis()))
            }
            is ChatStreamEvent.ToolResult -> update(id) { m ->
                val f = event.frame
                val call = ToolCallRecord(
                    id = event.toolCallId,
                    toolName = event.toolName,
                    state = ToolState.Done,
                    title = f.str("title"),
                    output = (f["output"] ?: f["result"])?.asText(),
                    input = f.obj("input"),
                    metadata = f.obj("metadata"),
                    files = f["files"],
                    run = f.obj("run"),
                    finishedAt = System.currentTimeMillis(),
                )
                val status = m.toolStatus?.let { s -> s.copy(state = ToolState.Done, elapsedMs = s.startedAt?.let { System.currentTimeMillis() - it }) }
                m.upsertToolCall(call).copy(toolStatus = status)
            }
            is ChatStreamEvent.ToolError -> update(id) { m ->
                val name = event.toolName ?: m.toolStatus?.toolName ?: "Tool"
                val status = m.toolStatus?.let { s ->
                    s.copy(state = ToolState.Failed, text = event.error, elapsedMs = s.startedAt?.let { System.currentTimeMillis() - it })
                }
                m.upsertToolCall(ToolCallRecord(event.toolCallId, name, ToolState.Failed, error = event.error, finishedAt = System.currentTimeMillis()))
                    .copy(toolStatus = status)
            }
            is ChatStreamEvent.Artifact -> update(id) {
                it.copy(artifacts = it.artifacts + ArtifactRecord(event.artifactId, event.kind, event.title, event.url, event.content))
            }
            is ChatStreamEvent.Finish -> {
                if (event.status != "error") return false
                flushPending()
                val raw = event.error ?: "The generation failed."
                update(id) { it.copy(error = ChatError(event.errorName, event.errorMessage, raw)) }
                return true
            }
            is ChatStreamEvent.StreamError -> {
                flushPending()
                val name = if (event.message == ChatError.NO_BRAIN_MESSAGE) ChatError.NO_BRAIN else null
                update(id) { it.copy(error = ChatError(name, event.message, event.message)) }
                return true
            }
            is ChatStreamEvent.Question -> {
                val ask = parseQuestion(event.request) ?: return false
                update(id) { m -> if (m.questions.any { it.id == ask.id }) m else m.copy(questions = m.questions + ask) }
            }
            is ChatStreamEvent.QuestionClosed -> closeQuestion(event.requestId)
            is ChatStreamEvent.MessageStart, ChatStreamEvent.Done, is ChatStreamEvent.Failed -> Unit
        }
        return false
    }

    /** The Stop button: cancel locally and abort the server-side run (`POST .../abort`). */
    /** iOS appendVoiceSummary: the "Voice chat ended · Ns" card after a voice session. */
    fun appendVoiceSummary(durationSeconds: Int) {
        messages = messages + MessageRecord(
            id = java.util.UUID.randomUUID().toString(),
            role = "system",
            content = "",
            voiceSummary = com.allternit.feature.chat.voice.VoiceSummary(durationSeconds),
        )
    }

    fun stopStreaming() {
        val sessionId = currentSessionId
        val wasStreaming = isStreaming
        streamJob?.cancel()
        streamJob = null
        flushPending()
        streamingMessageId?.let { id -> update(id) { it.copy(isStreaming = false) } }
        isStreaming = false
        isCreatingSession = false
        streamingMessageId = null
        if (wasStreaming && sessionId != null) {
            scope.launch { backend.abort(sessionId) }
            scope.launch {
                delay(1_500)
                if (currentSessionId == sessionId) refreshPermissions()
            }
        }
    }

    private suspend fun ensureSessionId(agentModeId: String?): String {
        currentSessionId?.let { return it }
        val ctx = sessionContext
        val name = when (ctx.originSurface) {
            "cowork" -> "New Cowork"
            "code" -> "New Code Thread"
            else -> "New Chat"
        }
        return when (val r = backend.createSession(name, ctx.originSurface, ctx.agentId, ctx.agentName, agentModeId ?: ctx.agentModeId, ctx.permissionMode, ctx.sessionMode, botBrain = ctx.botBrain)) {
            is ApiResult.Ok -> {
                currentSessionId = r.value.id
                onSessionsChanged()
                r.value.id
            }
            is ApiResult.HttpError -> if (ChatErrors.isComputeGate(r.status, r.code, r.message)) {
                throw GateException(ComputeGateReason.NoComputer)
            } else {
                throw SendException(r.message ?: ChatErrors.statusSentence(r.status) ?: "Something went wrong. Try again.")
            }
            is ApiResult.Network -> throw IOException("Could not connect to the server. Check your connection and try again.")
            is ApiResult.Decode -> throw SendException("Something went wrong. Try again.")
        }
    }

    // MARK: Attachments (iOS StagedAttachment → POST /uploads)

    private suspend fun uploadOne(a: ChatAttachmentInput): AttachmentRef {
        val (bytes, mediaType) = withContext(Dispatchers.IO) { readAttachment(a) } ?: throw SendException(uploadFailed(a.name))
        return when (val r = backend.upload(a.name, mediaType, bytes)) {
            is ApiResult.Ok -> AttachmentRef(url = r.value, mediaType = mediaType, name = a.name)
            else -> throw SendException(uploadFailed(a.name))
        }
    }

    private fun uploadFailed(name: String) = "Couldn't upload $name. Check the connection and try again."

    /** Bytes + media type: camera JPEG, a content Uri, or a GitHub file's raw text. */
    private fun readAttachment(a: ChatAttachmentInput): Pair<ByteArray, String>? {
        a.bytes?.let { return it to (a.mediaType ?: "application/octet-stream") }
        val uri = a.uri ?: return null
        if (a.kind == "github") {
            val raw = githubRawUrl(uri) ?: return null
            return runCatching {
                backend.publicHttp.newCall(Request.Builder().url(raw).build()).execute().use { resp ->
                    if (!resp.isSuccessful) return null
                    val bytes = resp.body?.bytes() ?: return null
                    bytes to "text/plain"
                }
            }.getOrNull()
        }
        val parsed = Uri.parse(uri)
        val type = contentResolver.getType(parsed)
            ?: MimeTypeMap.getSingleton().getMimeTypeFromExtension(a.name.substringAfterLast('.', "").lowercase())
            ?: "application/octet-stream"
        val bytes = runCatching { contentResolver.openInputStream(parsed)?.use { it.readBytes() } }.getOrNull() ?: return null
        return bytes to type
    }

    /** `github.com/o/r/blob/ref/path` → `raw.githubusercontent.com/o/r/ref/path` (iOS githubField). */
    private fun githubRawUrl(url: String): String? {
        val u = url.trim()
        if (u.startsWith("https://raw.githubusercontent.com/")) return u
        val m = Regex("""^https://github\.com/([^/]+)/([^/]+)/blob/(.+)$""").find(u) ?: return null
        return "https://raw.githubusercontent.com/${m.groupValues[1]}/${m.groupValues[2]}/${m.groupValues[3]}"
    }

    // MARK: Asks

    /**
     * Permission asks are polled (iOS AgentAskStore.refresh, `GET permissions?sessionId=`):
     * the chat frame decoder never produces them. iOS polls only after a reply finishes; the
     * agent waits on the ask mid-stream, so Android also polls every 4 s while a reply runs.
     */
    private fun startPermissionPolling(sessionId: String) {
        permissionJob?.cancel()
        permissionJob = scope.launch {
            while (isActive && isStreaming && currentSessionId == sessionId) {
                delay(4_000)
                if (isStreaming && currentSessionId == sessionId) refreshPermissions()
            }
        }
    }

    suspend fun refreshPermissions() {
        val sessionId = currentSessionId ?: return
        val asks = backend.listPermissions(sessionId)
        if (currentSessionId != sessionId) return
        val target = lastAssistantMessageId ?: return
        val open = asks.associateBy { it.id }
        messages = messages.map { m ->
            val kept = m.permissions.filter { it.answered != null || it.id in open }
            val added = if (m.id == target) asks.filter { a -> messages.none { msg -> msg.permissions.any { it.id == a.id } } } else emptyList()
            if (kept == m.permissions && added.isEmpty()) m else m.copy(permissions = kept + added)
        }
    }

    /** Allow ("once"), Always ("always") or Deny ("reject"). Errors come back to the card. */
    fun replyPermission(ask: PermissionAsk, reply: String, onError: (String) -> Unit) {
        scope.launch {
            when (val r = backend.replyPermission(ask.id, reply)) {
                is ApiResult.Ok -> messages = messages.map { m ->
                    m.copy(permissions = m.permissions.map { if (it.id == ask.id) it.copy(answered = reply) else it })
                }
                else -> onError("Couldn't send your answer: ${describe(r)}")
            }
        }
    }

    fun answerQuestion(ask: QuestionAsk, answers: List<List<String>>, onError: (String) -> Unit) {
        scope.launch {
            when (val r = backend.replyQuestion(ask.id, answers)) {
                is ApiResult.Ok -> closeQuestion(ask.id)
                else -> onError("Couldn't send your answer: ${describe(r)}")
            }
        }
    }

    fun skipQuestion(ask: QuestionAsk, onError: (String) -> Unit) {
        scope.launch {
            when (val r = backend.rejectQuestion(ask.id)) {
                is ApiResult.Ok -> closeQuestion(ask.id)
                else -> onError("Couldn't skip this question: ${describe(r)}")
            }
        }
    }

    private fun closeQuestion(id: String) {
        messages = messages.map { m ->
            if (m.questions.none { it.id == id }) m else m.copy(questions = m.questions.map { if (it.id == id) it.copy(closed = true) else it })
        }
    }

    private fun parseQuestion(o: JsonObject): QuestionAsk? {
        val id = o.str("id") ?: return null
        val items = o.arr("questions").mapNotNull { el ->
            val q = el as? JsonObject ?: return@mapNotNull null
            val text = q.str("question") ?: return@mapNotNull null
            QuestionItem(
                header = q.str("header"),
                question = text,
                options = q.arr("options").mapNotNull { opt ->
                    (opt as? JsonObject)?.str("label") ?: (opt as? JsonPrimitive)?.content
                },
                multiple = q.bool("multiple") == true || q.bool("multiSelect") == true,
            )
        }
        if (items.isEmpty()) return null
        return QuestionAsk(id, o.str("sessionID") ?: o.str("sessionId"), items)
    }

    // MARK: Flush & finish

    private fun scheduleFlush() {
        if (flushJob != null) return
        flushJob = scope.launch {
            delay(50)
            flushJob = null
            flushPending()
        }
    }

    private fun flushPending() {
        flushJob?.cancel()
        flushJob = null
        if (pendingText.isEmpty() && pendingReasoning.isEmpty()) return
        val text = pendingText.toString()
        val reasoning = pendingReasoning.toString()
        pendingText = StringBuilder()
        pendingReasoning = StringBuilder()
        val id = streamingMessageId ?: return
        update(id) { it.copy(content = it.content + text, reasoning = it.reasoning + reasoning) }
    }

    private fun finishStreaming(id: String) {
        flushPending()
        val started = requestStartedAt
        val first = firstDeltaAt
        if (started != null && first != null) {
            val chars = messages.firstOrNull { it.id == id }?.content?.length ?: 0
            val stats = PerfStats(first - started, System.currentTimeMillis() - first, maxOf(1, chars / 4))
            update(id) { it.copy(perfStats = stats) }
        }
        requestStartedAt = null
        firstDeltaAt = null
        update(id) { it.copy(isStreaming = false) }
        isStreaming = false
        streamingMessageId = null
        streamJob = null
        permissionJob?.cancel()
        permissionJob = null
        if (currentSessionId != null) {
            scope.launch {
                refreshPermissions()
                if (reloadBranchesAfterStream) {
                    reloadBranchesAfterStream = false
                    loadBranches()
                } else if (branches.isNotEmpty()) {
                    remapEditedVersions()
                }
            }
        }
    }

    private fun isPristine(id: String): Boolean {
        if (pendingText.isNotEmpty() || pendingReasoning.isNotEmpty()) return false
        val m = messages.firstOrNull { it.id == id } ?: return false
        return m.content.isEmpty() && m.toolStatus == null && m.artifacts.isEmpty() && m.toolCalls.isEmpty() && m.questions.isEmpty()
    }

    private fun update(id: String, mutate: (MessageRecord) -> MessageRecord) {
        val index = messages.indexOfFirst { it.id == id }
        if (index < 0) return
        messages = messages.toMutableList().also { it[index] = mutate(it[index]) }
    }

    private fun describe(r: ApiResult<*>): String = when (r) {
        is ApiResult.HttpError -> ChatErrors.statusSentence(r.status) ?: FriendlyErrorMessage.from(r.message ?: "Something went wrong. Try again.")
        is ApiResult.Network -> "Could not connect to the server. Check your connection and try again."
        is ApiResult.Decode -> "Something went wrong. Try again."
        is ApiResult.Ok -> ""
    }

    companion object {
        private const val WEB_SEARCH = "[web_search_enabled]"
        private const val RESEARCH = "[research_enabled]"

        /** iOS ToolOptionsStore.enrichedText: the web-search marker leads the sent text only. */
        fun enrichedText(text: String, webSearch: Boolean): String = if (webSearch) "$WEB_SEARCH $text" else text

        /** The text without the leading tool markers (what goes back to the composer). */
        fun strippedText(text: String): String {
            var body = text
            var changed = true
            while (changed) {
                changed = false
                for (p in listOf("$WEB_SEARCH ", "$RESEARCH ")) if (body.startsWith(p)) {
                    body = body.removePrefix(p)
                    changed = true
                }
            }
            return body
        }

        /** iOS ToolOptionsStore.optionsForSend → `metadata.tools`. */
        fun toolOptions(r: ChatSendRequest): JsonObject = buildJsonObject {
            put("webSearch", r.webSearch)
            put("research", r.research)
        }
    }
}
