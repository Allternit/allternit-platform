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

    /**
     * Start anywhere (step 20): the computer this thread is bound to, from `metadata.startBinding`. Engine
     * threads send every turn to that computer in that folder; terminal threads show the CLI's terminal.
     */
    var binding by mutableStateOf<StartBinding?>(null)
        private set
    /** A send to the picked computer found it asleep/offline: the feed shows the cloud offer again. */
    var asleepSend by mutableStateOf<AsleepSend?>(null)
    /** One key per first send, so a retried start never starts twice (Idempotency-Key). */
    private var startKey: String? = null

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
        browserImages?.request?.call?.let { GenerationProgressStore.shared.clear(it) }
        browserImages?.dispose()
        localImages?.deactivate()
        browserImages = null
        scope.cancel()
    }

    internal var browserImages by mutableStateOf<BrowserImageJob?>(null)
        private set
    internal var imageChatVisible by mutableStateOf(false)
    val hasBrowserImageJob: Boolean get() = browserImages?.request != null
    private var imagePoll: Job? = null
    private var imagePersonTurn = false
    private val imageTurns = mutableMapOf<String, String>()

    /** The shell owns this object. Leaving the browser or chat does not dispose the job. */
    fun attachBrowserImages(context: android.content.Context, accountId: String) {
        if (browserImages != null) return
        localImages = LocalImageStore(context, accountId)
        refreshLocalImages()
        val owner = BrowserImageJob(context, accountId)
        browserImages = owner
        imagePoll = scope.launch {
            while (isActive) {
                val sid = currentSessionId
                if (imageChatVisible && owner.foreground && imagePersonTurn && sid != null && !sid.startsWith("android-local-") && owner.request == null) {
                    val response = backend.imageSync("GET")
                    if (response is ApiResult.Ok) {
                        val json = org.json.JSONTokener(response.value.toString()).nextValue()
                        val rows = when (json) {
                            is org.json.JSONArray -> json
                            is org.json.JSONObject -> json.optJSONArray("requests") ?: json.optJSONArray("pending")
                            else -> null
                        }
                        if (rows != null) for (i in 0 until rows.length()) {
                            val request = BrowserImageJob.Request.parse(rows.optJSONObject(i) ?: continue, sid) ?: continue
                            if (offerBrowserImage(request, messages.lastOrNull { m -> m.toolCalls.any { it.id == request.call } }?.id ?: continue)) break
                        }
                    }
                }
                delay(1500)
            }
        }
    }

    private fun sendBrowserImage(input: ChatSendRequest) {
        val owner = browserImages ?: run { transientError = "Open this chat on the phone to make an image."; return }
        if (isStreaming || isCreatingSession || owner.request != null) return
        if (!input.personTurn) return
        if (input.text.isBlank() || input.text.length > 16000) { transientError = "Enter an image prompt of 1–16000 characters."; return }
        if (input.attachments.isNotEmpty()) { transientError = "This browser image action supports a text prompt. Remove attachments first."; draftToRestore = input.text; return }
        isCreatingSession = true
        scope.launch {
            try {
                val sid = currentSessionId ?: ("android-local-" + UUID.randomUUID()).also { currentSessionId = it }
                if (!imageChatVisible) { draftToRestore = input.text; return@launch }
                val id = "android-image-" + UUID.randomUUID().toString()
                val r = BrowserImageJob.Request(id, sid, id, input.text, System.currentTimeMillis() + 600000, false)
                offerBrowserImage(r, null)
            } catch (e: Exception) {
                transientError = e.message ?: "Could not start the image request."
                draftToRestore = input.text
            } finally { isCreatingSession = false }
        }
    }

    private var localImages: LocalImageStore? = null
    internal var savedImageTurns by mutableStateOf<List<org.json.JSONObject>>(emptyList())
        private set
    private val claimedImages = mutableMapOf<String, String>()
    private var claimingImage = false
    private val refusedImages = mutableSetOf<String>()
    private fun refreshLocalImages() { savedImageTurns = localImages?.entries().orEmpty().map { org.json.JSONObject(it.toString()) } }

    private fun offerBrowserImage(r: BrowserImageJob.Request, originalMessage: String?): Boolean {
        val owner = browserImages ?: return false
        if (r.remote && r.id !in claimedImages) {
            val sent = lastImageText
            if (sent == null || !r.prompt.startsWith(sent)) {
                // Device guard: only the text this person sent in Image mode may reach the browser.
                if (r.id !in refusedImages) {
                    refusedImages += r.id
                    scope.launch { backend.imageSync("POST", r.id, "reply", BrowserImageJob.error("invalid_request", "This request does not match what you sent, so it was not run.").toString()) }
                }
                return false
            }
            if (claimingImage) return false
            claimingImage = true
            scope.launch {
                try {
                    val executor = UUID.randomUUID().toString()
                    val claim = backend.imageSync("POST", r.id, "claim", org.json.JSONObject().put("executorId", executor).toString())
                    if (claim is ApiResult.Ok) {
                        claimedImages[r.id] = executor
                        offerBrowserImage(r, originalMessage)
                    }
                } catch (e: CancellationException) { throw e }
                catch (e: Exception) { transientError = "Could not claim this image request. It has not been sent." }
                finally { claimingImage = false }
            }
            return true
        }
        if (localImages?.entries()?.any { it.optString("id") == r.id && it.has("result") } == true) return false
        val messageId = originalMessage ?: UUID.randomUUID().toString()
        imageTurns[r.id] = messageId
        // A relayed turn is stored before offer, including replay/decline errors.
        if (r.remote) try { localImages!!.begin(r, messageId, null, messages) } catch (e: Exception) {
            scope.launch { backend.imageSync("POST", r.id, "reply", BrowserImageJob.error("storage", "Could not save the original turn on this phone.").put("executorId", claimedImages[r.id]).toString()) }
            transientError = "Could not save the image turn on this phone."; return false
        }
        val offered = owner.offer(r, onProgress = { detail ->
            val metadata = com.allternit.core.api.ApiJson.default.parseToJsonElement(org.json.JSONObject().put("progress_detail", detail).toString()) as JsonObject
            GenerationProgressDetail.read(metadata)?.let { GenerationProgressStore.shared.set(r.call, it) }
            if (currentSessionId == r.session) update(messageId) { it.upsertToolCall(ToolCallRecord(r.call, "media_generate", ToolState.Running, metadata = metadata,
                input = buildJsonObject { put("kind", "image"); put("prompt", r.prompt) })).copy(isStreaming = true) }
            if (r.remote) scope.launch {
                val response = backend.imageSync("POST", r.id, "progress", org.json.JSONObject(detail.toString()).put("executorId", claimedImages.getValue(r.id)).toString())
                if (response is ApiResult.HttpError && response.status in listOf(404, 409) && owner.request?.id == r.id) owner.cancel()
            }
        }, onComplete = { result ->
            GenerationProgressStore.shared.clear(r.call)
            // Persist against the captured original native ID, even when another chat is displayed.
            val hasTurn = localImages?.entries()?.any { it.optString("id") == r.id } == true
            val saved = try { if (hasTurn) localImages!!.complete(r.id, result) else result } catch (e: Exception) {
                transientError = "Could not save the image locally. Free storage and keep this chat open."
                BrowserImageJob.error("storage", "Could not save the image locally.")
            }
            refreshLocalImages()
            if (currentSessionId == r.session) {
                val row = savedImageTurns.firstOrNull { it.optString("id") == r.id }
                val finalMessage = row?.let { localImages!!.message(it) }
                if (finalMessage != null) update(messageId) { old -> old.copy(toolCalls = old.toolCalls.filterNot { it.id == r.call } + finalMessage.toolCalls, isStreaming = false) }
                else update(messageId) { it.upsertToolCall(ToolCallRecord(r.call, "media_generate", ToolState.Failed, output = saved.toString(), error = saved.optString("error"))) }
            }
            if (r.remote) scope.launch {
                val reply = backend.imageSync("POST", r.id, "reply", org.json.JSONObject(result.toString()).put("executorId", claimedImages.getValue(r.id)).toString())
                if (reply !is ApiResult.Ok) transientError = "The image is saved on this phone. Its runtime reply could not sync."
            }
            if (r.remote) onSessionsChanged()
        })
        // The person's own Image-mode send is the confirmation; no dialog.
        if (offered) confirmBrowserImage()
        return offered
    }

    internal fun confirmBrowserImage() {
        val owner = browserImages ?: return
        val r = owner.request ?: return
        if (!imageChatVisible || currentSessionId != r.session || owner.confirmed) return
        if (!r.remote) {
            val user = MessageRecord(UUID.randomUUID().toString(), "user", r.prompt)
            val assistant = MessageRecord(imageTurns.getValue(r.id), "assistant", "", isStreaming = true, toolCalls = listOf(ToolCallRecord(r.call, "media_generate", ToolState.Running,
                input = buildJsonObject { put("kind", "image"); put("prompt", r.prompt) })))
            try { localImages!!.begin(r, assistant.id, user.id, messages) } catch (e: Exception) {
                transientError = "Could not save this image turn. Free storage before trying again."; return
            }
            messages = messages + user + assistant
            refreshLocalImages()
        }
        imagePersonTurn = true
        owner.confirm()
    }

    // MARK: Sessions

    /**
     * Standing instructions for a session (iOS ArtifactStudioLauncher.systemPrompt(for:)), read on
     * every send, retry and regenerate. The shell sets it; null sends none.
     */
    var systemPromptFor: ((sessionId: String) -> String?)? = null

    fun startNewSession() {
        imagePersonTurn = false
        stopStreamingInternal(cancelImage = false)
        transientError = null
        computeGate = null
        messages = emptyList()
        branches = emptyList()
        editedVersions = emptyMap()
        currentSessionId = null
        binding = null
        startKey = null
        asleepSend = null
    }

    /** Opens a history chat: `GET agent-sessions/:id/messages`, then its branches. */
    fun loadSession(sessionId: String) {
        if (sessionId == currentSessionId && (messages.isNotEmpty() || isStreaming)) return
        imagePersonTurn = false
        stopStreamingInternal(cancelImage = false)
        transientError = null
        computeGate = null
        currentSessionId = sessionId
        binding = null
        messages = emptyList()
        branches = emptyList()
        editedVersions = emptyMap()
        isLoadingHistory = true
        val local = localImages?.messages(sessionId, browserImages?.request?.call).orEmpty()
        messages = local
        if (sessionId.startsWith("android-local-")) { isLoadingHistory = false; return }
        scope.launch {
            val r = backend.listMessages(sessionId)
            if (currentSessionId != sessionId) return@launch
            isLoadingHistory = false
            when (r) {
                is ApiResult.Ok -> {
                    messages = localImages?.merge(sessionId, r.value.map(MessageRecord::fromWire), browserImages?.request?.call) ?: r.value.map(MessageRecord::fromWire)
                    loadBranches()
                    refreshPermissions()
                    // A thread started on a computer keeps running there (metadata.startBinding).
                    (backend.api.getAgentSession(sessionId) as? ApiResult.Ok)?.value?.let { s ->
                        if (currentSessionId == sessionId) binding = StartBinding.read(s.metadata)
                    }
                    // The conversation lives in the engine session on that computer, not in the agent-session.
                    binding?.takeIf { !it.isTerminal }?.let { b ->
                        BoundEngine.messages(backend, b)?.let { rows ->
                            if (currentSessionId == sessionId) messages = rows.filter { it.second.isNotBlank() && (it.first == "user" || it.first == "assistant") }
                                .mapIndexed { i, (role, text) -> MessageRecord(id = "${b.sessionId}-$i", role = role, content = text) }
                        }
                        refreshPermissions()
                    }
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
        if (browserImages?.request?.session == id) browserImages?.cancel()
        localImages?.delete(id); refreshLocalImages()
        if (id.startsWith("android-local-")) { startNewSession(); onSessionsChanged(); onDone(); return }
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
        request.imagePrompt?.takeIf { request.personTurn }?.let { lastImageText = it }
        if (request.browserImage) { sendBrowserImage(request); return }
        // Image mode: a chat with a runtime sends normally (image_prompt rides along); a phone-only chat
        // (saved locally, or no computer paired) runs the device job directly.
        if (request.agentModeId == "image" && request.personTurn && request.imagePrompt != null && request.attachments.isEmpty()) {
            if (currentSessionId?.startsWith("android-local-") == true) { sendBrowserImage(request); return }
            if (isStreaming || isCreatingSession || browserImages?.request != null) return
            isCreatingSession = true
            scope.launch {
                val pick = try { backend.relay.resolve() } finally { isCreatingSession = false }
                if (pick == RelayPick.NoComputer) sendBrowserImage(request) else sendToRuntime(request)
            }
            return
        }
        sendToRuntime(request)
    }

    /** The last Image-mode text this person sent; a runtime-issued browser-image request must start with it. */
    private var lastImageText: String? = null

    private fun sendToRuntime(request: ChatSendRequest) {
        val trimmed = request.text.trim()
        if (trimmed.isEmpty() || isStreaming || isCreatingSession || browserImages?.request != null) return
        imagePersonTurn = request.personTurn
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
            personTurn = request.personTurn,
            imagePrompt = request.imagePrompt,
            skill = request.skillId?.let { id -> buildJsonObject { put("id", id); request.skillArguments?.let { put("arguments", it) } } },
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
        if (isStreaming || isCreatingSession || hasBrowserImageJob) return
        val browserCall = messages.lastOrNull { it.role == "assistant" }?.toolCalls?.lastOrNull { it.browserImageResult != null }
        if (browserCall != null) {
            draftToRestore = browserCall.input?.str("prompt")
            transientError = "Use Image mode to confirm a new request. Check the browser first if the earlier request was interrupted."
            return
        }
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
        personTurn: Boolean = false,
        imagePrompt: String? = null,
        skill: JsonObject? = null,
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
                if (currentSessionId?.startsWith("android-local-") == true) {
                    throw SendException("This chat contains images saved on this phone. Choose Image for another picture, or start a new chat for a computer-backed model.")
                }
                // Start anywhere: the first message with a picked computer starts a session there and
                // binds this thread to it; a bound thread keeps going to the same computer.
                val bound = binding ?: if (currentSessionId == null && PendingStart.appliesTo(null, messages.count { it.role == "user" } > 1)) startOnPicked(text, hasFiles = staged.isNotEmpty()) else null
                if (bound?.isTerminal == true) {
                    // The CLI got the message as its prompt; the thread now shows its terminal (ChatPage).
                    ensureSessionId(agentModeId, bound)
                    isCreatingSession = false
                    binding = bound
                    // A CLI in a terminal takes text only: say so rather than drop the files silently.
                    if (staged.isNotEmpty()) transientError = "${bound.runtime} in a terminal takes text only, so your files weren't sent."
                    update(assistantId) { it.copy(content = "Started ${bound.runtime} in ${bound.folder.trimEnd('/').substringAfterLast('/')} on ${bound.deviceName}.") }
                    finishStreaming(assistantId)
                    return@launch
                }
                if (bound != null) {
                    // An engine-bound thread drives the session the start opened, on its computer (web
                    // send.ts): the first message went in as the start's prompt; later ones go to
                    // remote-control `…/messages`. The reply follows that session's events.
                    val first = binding == null
                    ensureSessionId(agentModeId, bound)
                    binding = bound
                    isCreatingSession = false
                    requestStartedAt = System.currentTimeMillis()
                    firstDeltaAt = null
                    startPermissionPolling(currentSessionId ?: "")
                    // The start carried the first message as its prompt, unless it had files.
                    val viaMessages = !first || staged.isNotEmpty()
                    val since = if (first) 0 else BoundEngine.messages(backend, bound)?.size ?: 0
                    if (viaMessages) {
                        val files = staged.map { a ->
                            val (bytes, mediaType) = withContext(Dispatchers.IO) { readAttachment(a) } ?: throw SendException(uploadFailed(a.name))
                            if (bytes.size > BoundEngine.MAX_FILE_BYTES) throw SendException("${a.name} is too big to send to ${bound.deviceName} (10 MB at most).")
                            BoundEngine.FilePart(mediaType, "data:$mediaType;base64," + android.util.Base64.encodeToString(bytes, android.util.Base64.NO_WRAP), a.name)
                        }
                        BoundEngine.send(backend, bound, strippedText(text), files)?.let { throw SendException(it) }
                    }
                    BoundEngine.follow(backend, bound, since, onText = { t -> update(assistantId) { it.copy(content = t) } }, onAsk = { refreshPermissions() })
                    refreshPermissions()
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
                failed = runStream(runtimeId, body, assistantId, personTurn, imagePrompt, skill)
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
    private suspend fun runStream(runtimeId: String, body: AgentChatRequest, assistantId: String, personTurn: Boolean, imagePrompt: String? = null, skill: JsonObject? = null): Boolean {
        val deadline = System.currentTimeMillis() + 120_000
        while (true) {
            var failedFrame = false
            var transport: ChatStreamEvent.Failed? = null
            backend.streamTurn(runtimeId, body, personTurn && browserImages?.foreground == true, imagePrompt?.takeIf { personTurn && browserImages?.foreground == true }, skill).collect { event ->
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
            is ChatStreamEvent.ToolProgress -> {
                // A running tool's live report: the call stays running and keeps the newest metadata.
                update(id) { it.upsertToolCall(ToolCallRecord(event.toolCallId, event.toolName, ToolState.Running, metadata = event.metadata, startedAt = System.currentTimeMillis())) }
                syncProgress(id)
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
            }.also { syncProgress(id) }
            is ChatStreamEvent.ToolError -> update(id) { m ->
                val name = event.toolName ?: m.toolStatus?.toolName ?: "Tool"
                val status = m.toolStatus?.let { s ->
                    s.copy(state = ToolState.Failed, text = event.error, elapsedMs = s.startedAt?.let { System.currentTimeMillis() - it })
                }
                m.upsertToolCall(ToolCallRecord(event.toolCallId, name, ToolState.Failed, error = event.error, finishedAt = System.currentTimeMillis()))
                    .copy(toolStatus = status)
            }.also { syncProgress(id) }
            is ChatStreamEvent.Artifact -> update(id) {
                // An engine picture or GIF (`tool-call://<callID>`) belongs inside that call's card, not in a chip.
                it.attachToolImage(event.sourceUri, event.url, event.mimeType) ?: it.copy(artifacts = it.artifacts + ArtifactRecord(event.artifactId, event.kind, event.title, event.url, event.content))
            }
            is ChatStreamEvent.Finish -> {
                val refs = MemoryRef.parseAll(event.memoryRefs)
                if (refs.isNotEmpty()) update(id) { it.copy(memoryRefs = refs) }
                if (event.status != "error") return false
                flushPending()
                val raw = event.error ?: "The generation failed."
                // A typed error frame already drew its card: one error per turn, never two.
                update(id) { if (it.error?.typed != null) it else it.copy(error = ChatError(event.errorName, event.errorMessage, raw)) }
                return true
            }
            is ChatStreamEvent.StreamError -> {
                flushPending()
                val name = if (event.message == ChatError.NO_BRAIN_MESSAGE) ChatError.NO_BRAIN else null
                val typed = if (event.code != null || event.resetAtMs != null) {
                    ChatErrorInfo.make(event.message, event.code, resetAtMs = event.resetAtMs)
                } else null
                update(id) { it.copy(error = ChatError(name, event.message, event.message, typed)) }
                return true
            }
            is ChatStreamEvent.Question -> {
                val ask = parseQuestion(event.request) ?: return false
                update(id) { m -> if (m.questions.any { it.id == ask.id }) m else m.copy(questions = m.questions + ask) }
            }
            is ChatStreamEvent.QuestionClosed -> closeQuestion(event.requestId)
            is ChatStreamEvent.McpTask -> McpTaskPart.from(event.part)?.let { part ->
                McpTasksStore.apply(part)
                update(id) { m -> if (part.taskId in m.mcpTaskIds) m else m.copy(mcpTaskIds = m.mcpTaskIds + part.taskId) }
            }
            is ChatStreamEvent.McpElicitation -> McpElicitationPart.from(event.part)?.let { part ->
                McpTasksStore.apply(part)
                update(id) { m -> if (part.id in m.mcpFormIds) m else m.copy(mcpFormIds = m.mcpFormIds + part.id) }
            }
            is ChatStreamEvent.McpElicitationResolved -> McpTasksStore.markResolved(event.requestId, event.action ?: "resolved")
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

    fun stopStreaming() = stopStreamingInternal(cancelImage = true)

    private fun stopStreamingInternal(cancelImage: Boolean) {
        if (cancelImage) browserImages?.cancel()
        val sessionId = currentSessionId
        val wasStreaming = isStreaming
        streamJob?.cancel()
        streamJob = null
        flushPending()
        streamingMessageId?.let { id -> update(id) { it.copy(isStreaming = false) } }
        isStreaming = false
        isCreatingSession = false
        streamingMessageId = null
        val bound = binding?.takeIf { !it.isTerminal }
        if (wasStreaming && bound != null) {
            // Stop on a bound engine thread aborts the session on its own computer.
            scope.launch { BoundEngine.abort(backend, bound) }
            scope.launch {
                delay(1_500)
                if (currentSessionId == sessionId) refreshPermissions()
            }
        } else if (wasStreaming && sessionId != null) {
            scope.launch { backend.abort(sessionId) }
            scope.launch {
                delay(1_500)
                if (currentSessionId == sessionId) refreshPermissions()
            }
        }
    }

    /**
     * Starts the picked computer's session for the first message (`POST /api/v1/session-start` through the
     * relay to THAT computer). Gizzi (`allternit`) is an engine session the chat drives; any other CLI is a
     * pane terminal that gets [text] as its prompt. Null when no computer is picked (today's path).
     */
    private suspend fun startOnPicked(text: String, hasFiles: Boolean = false): StartBinding? {
        val runtimeId = PendingStart.runtimeId ?: return null
        val deviceName = PendingStart.deviceName ?: "your computer"
        val runtime = contractRuntime((sessionContext.botBrain?.get("runtime") as? JsonPrimitive)?.content ?: "allternit")
        asleepSend = null
        val folder = PendingStart.folder ?: run {
            val opts = PendingStart.options ?: (SessionStartClient.options(backend, runtimeId) as? StartResult.Ok)?.value
            opts?.folders?.firstOrNull()?.let { StartFolderRef.Path(it.path) }
        } ?: throw SendException(
            if (PendingStart.asleep) asleepLine(deviceName) else "Pick a folder on $deviceName first: + › Folder.",
        )
        val picked = PendingStart.mode
        val origin = StartOrigin(surface = "android", deviceName = android.os.Build.MODEL ?: "Android phone", depth = 0)
        val key = startKey ?: UUID.randomUUID().toString().also { startKey = it }
        val request = StartRequest(folder = folder, runtime = runtime, mode = picked, // Gizzi with files: start with no prompt, then the first message (text + files) goes to remote-control.
            prompt = if (runtime == "allternit" && hasFiles) null else strippedText(text), origin = origin)
        val binding = when (val r = SessionStartClient.start(backend, runtimeId, request, key)) {
            is StartResult.Ok -> when (val v = r.value) {
                is StartResponse.Engine -> StartBinding(runtimeId, deviceName, v.directory.ifEmpty { (folder as? StartFolderRef.Path)?.path.orEmpty() }, runtime,
                    picked ?: PendingStart.defaultMode ?: StartMode.Default, "engine", sessionId = v.sessionId, origin = origin)
                is StartResponse.Terminal -> StartBinding(runtimeId, deviceName, v.cwd, v.agent.ifEmpty { runtime },
                    picked ?: PendingStart.defaultMode ?: StartMode.Default, "terminal", terminalId = v.terminalId, origin = origin)
            }
            is StartResult.Refused -> throw SendException(r.error.sentence)
            is StartResult.Unreachable -> {
                // Asleep or offline: the plain line, and the cloud offer again in the feed.
                asleepSend = AsleepSend(runtimeId, deviceName, folder)
                throw SendException(asleepLine(deviceName))
            }
        }
        startKey = null
        PendingStart.clear()
        return binding
    }

    /** "Pick it anyway" on an asleep computer: a plain line, and the cloud offer is one tap away again. */
    private fun asleepLine(name: String) =
        "$name is asleep, so the session didn't start. Open + › Computer to use a cloud computer, or wake $name and send again."

    private suspend fun ensureSessionId(agentModeId: String?, bound: StartBinding? = null): String {
        currentSessionId?.let { return it }
        val ctx = sessionContext
        val extra = bound?.let { b ->
            buildJsonObject {
                put(StartBinding.KEY, b.toJson())
                b.sessionId?.let { put("gizziSessionId", it) }
            }
        }
        val name = when (ctx.originSurface) {
            "cowork" -> "New Cowork"
            "code" -> "New Code Thread"
            else -> "New Chat"
        }
        return when (val r = backend.createSession(name, ctx.originSurface, ctx.agentId, ctx.agentName, agentModeId ?: ctx.agentModeId, bound?.mode?.id ?: PendingStart.mode?.id ?: ctx.permissionMode, ctx.sessionMode, botBrain = ctx.botBrain, extraMetadata = extra)) {
            is ApiResult.Ok -> {
                currentSessionId = r.value.id
                // The + sheet's Permission pick was for this chat only.
                PendingStart.mode = null
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
        val bound = binding?.takeIf { !it.isTerminal }
        val asks = if (bound != null) BoundEngine.permissions(backend, bound) else backend.listPermissions(sessionId)
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
            val bound = binding?.takeIf { !it.isTerminal }
            val r: ApiResult<Unit> = if (bound != null) {
                if (BoundEngine.reply(backend, bound, ask.id, reply)) ApiResult.Ok(Unit) else ApiResult.Network(java.io.IOException("${bound.deviceName} did not take the answer."))
            } else backend.replyPermission(ask.id, reply)
            when (r) {
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
                plan = parsePlanDraft(q),
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
        // The turn is over: no call of it is running any more, so its progress leaves the store.
        messages.firstOrNull { it.id == id }?.let { GenerationProgressStore.shared.sync(it.toolCalls, replyIsLive = false) }
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

    /** Keeps the shared progress store in step with one reply's tool calls. */
    private fun syncProgress(id: String) {
        val m = messages.firstOrNull { it.id == id } ?: return
        GenerationProgressStore.shared.sync(m.toolCalls, replyIsLive = m.isStreaming)
    }

    private fun isPristine(id: String): Boolean {
        if (pendingText.isNotEmpty() || pendingReasoning.isNotEmpty()) return false
        val m = messages.firstOrNull { it.id == id } ?: return false
        return m.content.isEmpty() && m.toolStatus == null && m.artifacts.isEmpty() && m.toolCalls.isEmpty() && m.questions.isEmpty() && m.mcpTaskIds.isEmpty() && m.mcpFormIds.isEmpty()
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
