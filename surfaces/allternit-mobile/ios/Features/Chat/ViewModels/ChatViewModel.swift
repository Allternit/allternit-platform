import Foundation
import SwiftUI

/// View-side chat message. Assistant messages accumulate streamed text,
/// reasoning ("thinking"), a tool-call status line, and typed artifact
/// attachments — all driven by `AgentChatEvent` frames from the agent-chat
/// stream.
struct MessageRecord: Identifiable, Equatable, Sendable {
    /// Latest known state of the most recent tool call (rendered as a status line).
    struct ToolStatus: Equatable, Sendable {
        enum State: Equatable, Sendable {
            case running
            case done
            case failed
        }

        var toolName: String
        var text: String
        var state: State
        /// Wall-clock start, for the elapsed-time readout once done/failed —
        /// nil for tool statuses restored from history (no live timing).
        var startedAt: Date? = nil
        /// Set once state leaves .running. Only elapsed time is shown, never
        /// a token/byte count — the wire protocol (AgentChatEvent) doesn't
        /// carry tool result size, and fabricating one would be dishonest.
        var elapsed: TimeInterval? = nil
    }

    /// Client-side-measured performance readout for a completed assistant
    /// reply, rendered as an inline footer (PocketPal-inspired). The backend
    /// stream (v1_routes.rs's `finish` frame) carries no token/usage
    /// telemetry at all, so this is wall-clock timing plus an approximate
    /// token count (chars/4, the common rule-of-thumb for English text) —
    /// not real inference-engine stats the way a local llama.cpp run would
    /// have. `~` prefixes in the UI make the approximation explicit rather
    /// than presenting it as precise.
    struct PerfStats: Equatable, Sendable {
        /// Time from request sent to the first content delta arriving.
        var timeToFirstToken: TimeInterval
        /// Time from the first content delta to the finish frame.
        var generationDuration: TimeInterval
        /// chars/4 estimate — not a real tokenizer count.
        var approxTokenCount: Int

        var approxTokensPerSecond: Double {
            generationDuration > 0 ? Double(approxTokenCount) / generationDuration : 0
        }
    }

    let id: String
    let role: String // "user" | "assistant" | "system"
    var content: String
    var reasoning: String = ""
    var isStreaming: Bool = false
    var toolStatus: ToolStatus? = nil
    /// Every tool call of the reply, for the tool calls rail and the agent
    /// run tree (Features/RunTree).
    var toolCalls: [ToolCallRecord] = []
    /// Questions the agent asked during this reply, answered in place
    /// (Features/Decisions QuestionCardView).
    var questions: [AgentQuestionRequest] = []
    /// Permissions the agent waited on during this reply.
    var permissions: [AgentPermissionAsk] = []
    var artifacts: [ArtifactRecord] = []
    /// Structured stream/transport failure rendered as an inline error card
    /// (ChatErrorCardView) instead of raw backend text in the bubble.
    var error: ChatError? = nil
    /// Set once, when the stream finishes normally — nil while streaming
    /// and nil for messages loaded from history (no live timing to derive
    /// it from).
    var perfStats: PerfStats? = nil
    /// Local-only record of a finished voice-mode session (Phase 7b),
    /// rendered as the "Voice chat ended · Ns" card (VoiceSummaryCardView).
    /// Never sent to or loaded from the backend — the row is a feed-local
    /// marker, like the local-only 👍/👎 feedback.
    var voiceSummary: VoiceSummary? = nil
}

/// Parameters for the NEXT session create, pushed from the view whenever
/// the app mode or composer agent state changes. Mode/toggle are
/// composer-level (pre-session only), so the context only ever affects
/// sessions created after the change — it never mutates a live session.
struct SessionContext: Equatable, Sendable {
    /// "chat" | "cowork" — the app mode's `origin_surface`.
    var originSurface: String = "chat"
    /// "regular" | "agent" — "agent" when the agent pill is on.
    var sessionMode: String = "regular"
    /// Selected registry agent (nil = backend default agent). Also sent on
    /// the agent-chat body: the bridge composes the persona, workspace
    /// files (SOUL.md/STYLE.md), and response-style preferences
    /// SERVER-SIDE from it (v1_routes.rs agent_chat_bridge) — clients
    /// never inject prompts themselves.
    var agentId: String? = nil
    var agentName: String? = nil
    /// Selected bottom-deck tile, sent as `metadata.agentModeId`.
    var agentModeId: String? = nil
    /// Selected cowork project, sent as `metadata.projectId` so the backend
    /// stamps the session for project grouping (ProjectStore selection).
    var projectId: String? = nil
    /// Incognito chat (Phase 6), sent as `metadata.ephemeral` — ephemeral
    /// sessions are excluded from history and purged on abort server-side.
    var ephemeral: Bool = false
    /// Onboarding work-profile answer (Phase 10), sent as
    /// `metadata.persona` — mirrors how projectId/agentModeId are carried.
    var persona: String? = nil
}

/// Send-path failures with user-meaningful messages (surfaced via the
/// composer's transient banner through `localizedDescription`).
enum ChatSendError: LocalizedError {
    case attachmentUploadFailed(String)

    var errorDescription: String? {
        switch self {
        case .attachmentUploadFailed(let name):
            return "Couldn't upload \(name). Check the connection and try again."
        }
    }
}

@MainActor
final class ChatViewModel: ObservableObject {    @Published var messages: [MessageRecord] = []
    @Published var isStreaming: Bool = false
    @Published var currentSessionId: String? = nil
    /// True only during the first message's session-create POST; the
    /// composer disables send on it so a slow create can't double-send.
    @Published var isCreatingSession: Bool = false
    /// Dismissible banner text for load/send failures (shown at the top of
    /// the feed); nil hides the banner.
    @Published var transientError: String? = nil
    /// Failed-send draft handed back to the composer (the message never
    /// reached the backend); ChatView restores it into `inputText`.
    @Published var draftToRestore: String? = nil

    private let chatClient: AgentChatClient

    /// Current composer mode/agent context for the next session create —
    /// set by ChatView from the mode/agent stores (see SessionContext).
    var sessionContext = SessionContext()

    /// The in-flight assistant message, when a stream is active (kept across
    /// backgrounding so the foreground reconcile can settle its state).
    private var streamingMessageId: String? = nil
    private var streamTask: Task<Void, Never>? = nil

    // MARK: - Streaming throttle (Enchanted pattern)
    //
    // Per-token deltas are appended to these buffers; a ~50ms coalescing task
    // flushes them into `messages` so SwiftUI re-renders at most ~20x/sec
    // instead of once per token. Low-frequency events (tool status, artifacts)
    // are applied to the message immediately.
    private var pendingText: String = ""
    private var pendingReasoning: String = ""
    private var flushTask: Task<Void, Never>? = nil
    private static let flushIntervalNs: UInt64 = 50_000_000

    // MARK: - Perf stats timing (client-side wall clock — see MessageRecord.PerfStats)

    /// Set right before the stream request goes out; nil once consumed by
    /// finishStreaming so a retried/new message doesn't inherit stale timing.
    private var streamRequestStartedAt: Date? = nil
    /// Set on the first content delta of the current stream.
    private var streamFirstDeltaAt: Date? = nil

    init(apiClient: APIClient = .shared) {
        self.chatClient = AgentChatClient(client: apiClient)
    }

    // MARK: - Sessions

    /// Temporary-chat mode (ChatGPT parity): the conversation works as/// normal — streaming still needs a backing agent-session — but the
    /// backing record is DELETEd the moment the chat is left, so nothing
    /// lands in history. Any `currentSessionId` seen while this is true was
    /// created inside temporary mode (loading a history chat clears it).
    @Published var isTemporaryChat = false

    /// Incognito chat (Phase 6, Claude parity): the next session create is
    /// stamped `metadata.ephemeral` so the backend excludes it from history
    /// and purges it on abort. Starting a normal new chat or loading a
    /// history conversation clears this.
    @Published var isIncognito = false

    func toggleTemporaryChat() {
        let wasTemporary = isTemporaryChat
        startNewSession()
        isTemporaryChat = !wasTemporary
    }

    /// Phase 7b: files the "Voice chat ended · Ns" card into the feed when a
    /// voice-mode session ends. The voice conversation itself already went
    /// through `sendMessage`, so only the summary marker is appended. The
    /// row is LOCAL-ONLY (like the 👍/👎 feedback) — the backend has no
    /// summary-card message type, so a history reload drops it.
    func appendVoiceSummary(durationSeconds: Int) {
        messages.append(MessageRecord(
            id: UUID().uuidString,
            role: "system",
            content: "",
            voiceSummary: VoiceSummary(durationSeconds: durationSeconds)
        ))
    }

    /// Starts a fresh chat; `ephemeral: true` enters incognito mode (the
    /// ghost-button path), the default false exits it.
    func startNewSession(ephemeral: Bool = false) {
        stopStreaming()
        discardTemporaryBackingSession()
        transientError = nil
        messages = []
        currentSessionId = nil
        isIncognito = ephemeral
        sessionContext.ephemeral = ephemeral
    }

    func loadSession(_ sessionId: String) {
        stopStreaming()
        // Opening a real conversation exits temporary mode.
        discardTemporaryBackingSession()
        isTemporaryChat = false
        // …and incognito mode — a history chat is never ephemeral.
        isIncognito = false
        sessionContext.ephemeral = false
        transientError = nil
        currentSessionId = sessionId
        messages = []

        Task { [weak self] in
            guard let self else { return }
            do {
                // GET /api/v1/agent-sessions/:id/messages → bare array
                let records = try await chatClient.listMessages(sessionId: sessionId)
                // Ignore stale responses if the user switched sessions mid-flight.
                guard self.currentSessionId == sessionId else { return }
                self.messages = records.map(Self.mapRecord)
            } catch {
                self.transientError = "Couldn't load the conversation: \(FriendlyErrorMessage.from(error.localizedDescription))"
            }
        }
    }

    /// Best-effort DELETE of a temporary chat's backing session so it never
    /// shows in history. Failures are swallowed — worst case the record
    /// survives as an ordinary (empty-looking) history row.
    private func discardTemporaryBackingSession() {
        guard isTemporaryChat, let sessionId = currentSessionId else { return }
        Task {
            try? await APIClient.shared.delete(path: "agent-sessions/\(sessionId)")
        }
    }

    /// Maps a wire message to the view model. The backend's `content` may
    /// already include reasoning text (it also ships separately in
    /// `thinking`) — mirrored as-is, like the web's `mapBackendMessage`.
    ///
    /// Reconstructs artifact cards from `metadata.parts` so file/URL
    /// attachments survive history loads and foreground reconciles.
    nonisolated private static func mapRecord(_ record: AgentSessionMessage) -> MessageRecord {
        let parts = record.metadata?.parts ?? []
        let artifacts = parts
            .filter { $0.type == "file" }
            .map { ArtifactRecord(part: $0, messageId: record.id) }
        return MessageRecord(
            id: record.id,
            role: record.role,
            content: record.content,
            reasoning: record.thinking ?? "",
            toolCalls: ToolCallRecord.fromHistory(parts),
            artifacts: artifacts
        )
    }

    /// Creates the backing agent session on first send
    /// (POST /api/v1/agent-sessions → `ses_*` record), stamped with the
    /// current mode (`origin_surface`) and agent state (`session_mode`,
    /// selected agent / tile).
    private func ensureSessionId() async throws -> String {
        if let id = currentSessionId {
            return id
        }

        let defaultName: String
        switch sessionContext.originSurface {
        case "cowork": defaultName = "New Cowork"
        case "code": defaultName = "New Code Thread"
        default: defaultName = "New Chat"
        }

        let session = try await chatClient.createSession(
            name: defaultName,
            originSurface: sessionContext.originSurface,
            sessionMode: sessionContext.sessionMode,
            agentId: sessionContext.agentId,
            agentName: sessionContext.agentName,
            agentModeId: sessionContext.agentModeId,
            projectId: sessionContext.projectId,
            ephemeral: sessionContext.ephemeral,
            persona: sessionContext.persona
        )
        currentSessionId = session.id
        return session.id
    }

    // MARK: - Sending & streaming

    /// `runtimeModelId` is the composer pill's catalog id ("provider/model");
    /// nil lets the backend pick its default model. `effort` is the
    /// reasoning-effort selection for effort-capable models. `attachments`
    /// are the composer's staged files — each is uploaded
    /// (`POST /api/v1/uploads`) before the stream starts and referenced from
    /// the agent-chat body; an upload failure fails the send.
    func sendMessage(_ text: String, attachments: [StagedAttachment] = [], runtimeModelId: String? = nil, effort: String? = nil, enrich: Bool = true) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, !isStreaming, !isCreatingSession else { return }

        let userMessageId = UUID().uuidString
        messages.append(MessageRecord(id: userMessageId, role: "user", content: trimmed))

        // The UI shows the user's original text; prefixes (web search, style)
        // are injected only into the stream payload (ChatComposer.tsx parity).
        let streamText = enrich ? ToolOptionsStore.shared.enrichedText(trimmed) : trimmed
        startStream(text: streamText, runtimeModelId: runtimeModelId, effort: effort, stagedAttachments: attachments, userMessageId: userMessageId)
    }

    /// Error-card "Retry": drops the failed assistant placeholder and
    /// re-streams the last sent text WITHOUT re-appending the user bubble
    /// (it's still in the feed above the error card). Attachments ride along
    /// as their already-uploaded refs — no second upload round-trip.
    func retryFailedMessage(_ assistantId: String, runtimeModelId: String?, effort: String? = nil) {
        guard let last = lastSent, !isStreaming, !isCreatingSession else { return }
        messages.removeAll { $0.id == assistantId }
        startStream(text: last.text,
                    runtimeModelId: runtimeModelId ?? last.runtimeModelId,
                    effort: effort ?? last.effort,
                    preparedRefs: last.attachmentRefs)
    }

    // MARK: - Regenerate & edit-resend (Phase 8)

    /// The most recent assistant/user message ids — drive the action bar's
    /// retry button and the user bubble's Edit affordance.
    var lastAssistantMessageId: String? {
        messages.last(where: { $0.role == "assistant" })?.id
    }

    var lastUserMessageId: String? {
        messages.last(where: { $0.role == "user" })?.id
    }

    /// Action-bar "Retry" on a SUCCESSFUL reply (Claude parity): removes the
    /// last assistant message and re-streams the last user text through the
    /// same `startStream` path as `retryFailedMessage`. The text comes from
    /// `lastSent` when the reply was streamed this run; for
    /// history-loaded conversations it falls back to the last user bubble.
    func regenerateLastResponse(runtimeModelId: String?, effort: String? = nil) {
        guard !isStreaming, !isCreatingSession,
              let assistantIndex = messages.lastIndex(where: { $0.role == "assistant" }) else { return }
        let lastUserText = messages[..<assistantIndex].last(where: { $0.role == "user" })?.content
        let text = lastSent?.text ?? lastUserText
        guard let text, !text.isEmpty else { return }
        // Attachment refs only apply when re-streaming the message that
        // carried them (this run's lastSent).
        let refs = lastSent?.text == text ? (lastSent?.attachmentRefs ?? []) : []
        messages.remove(at: assistantIndex)
        startStream(text: text,
                    runtimeModelId: runtimeModelId ?? lastSent?.runtimeModelId,
                    effort: effort ?? lastSent?.effort,
                    preparedRefs: refs)
    }

    /// Edit-resend: the user edited the LAST user bubble and hit send.
    /// Truncates the conversation from that message on and re-sends the new
    /// text as a fresh send.
    ///
    /// Server-side truncation: `POST /api/v1/agent-sessions/:id/revert` is
    /// live on the backend (agent_session_routes.rs — probed 2026-07: 200 on
    /// a valid session), so the revert call runs alongside the client-side
    /// truncation. A failure is logged, not swallowed — the feed reload on
    /// the next foreground reconcile is the source of truth.
    ///
    /// `attachments` are any newly-staged files in the composer while editing.
    /// Original attachments from the message being edited are not currently
    /// repopulated into the composer; this preserves at least the new ones.
    func resendEditedMessage(_ messageId: String, newText: String, attachments: [StagedAttachment] = [], runtimeModelId: String?, effort: String? = nil) {
        guard !isStreaming, !isCreatingSession,
              let index = messages.firstIndex(where: { $0.id == messageId }) else { return }
        if let sessionId = currentSessionId {
            Task { [chatClient] in
                do {
                    try await chatClient.revert(sessionId: sessionId)
                } catch {
                    print("agent-sessions: revert failed for \(sessionId): \(error.localizedDescription)")
                }
            }
        }
        messages.removeSubrange(index...)
        // Re-apply active composer prefixes to the edited resend.
        let enriched = ToolOptionsStore.shared.enrichedText(newText)
        sendMessage(enriched, attachments: attachments, runtimeModelId: runtimeModelId, effort: effort, enrich: false)
    }

    /// The last user text actually handed to the stream — the retry source.
    private var lastSent: (text: String, runtimeModelId: String?, effort: String?, attachmentRefs: [AttachmentRef])? = nil

    /// Shared send path: optimistic assistant placeholder + stream task.
    /// `userMessageId` is the optimistic user bubble created by the caller
    /// (nil on retry — the bubble is already in the feed); the
    /// never-landed rollback removes it along with the placeholder.
    /// `stagedAttachments` are uploaded first; `preparedRefs` skips the
    /// upload step (retry of an already-uploaded send).
    private func startStream(text trimmed: String, runtimeModelId: String?, effort: String? = nil, stagedAttachments: [StagedAttachment] = [], preparedRefs: [AttachmentRef]? = nil, userMessageId: String? = nil) {
        transientError = nil
        draftToRestore = nil

        let assistantId = UUID().uuidString
        messages.append(MessageRecord(id: assistantId, role: "assistant", content: "", isStreaming: true))

        isStreaming = true
        streamingMessageId = assistantId

        streamTask = Task { [weak self] in
            guard let self else { return }
            var streamFailed = false
            // True only while the first message's session-create POST is in
            // flight — the composer disables send on it (double-send guard).
            self.isCreatingSession = self.currentSessionId == nil
            do {
                // Upload staged attachments before streaming. An upload
                // failure fails the send — never silently drop the files and
                // send a text-only message the user didn't ask for.
                var attachmentRefs = preparedRefs ?? []
                if preparedRefs == nil, !stagedAttachments.isEmpty {
                    for attachment in stagedAttachments {
                        do {
                            let response = try await self.chatClient.upload(
                                name: attachment.filename,
                                mediaType: attachment.mediaType,
                                dataBase64: attachment.data.base64EncodedString()
                            )
                            attachmentRefs.append(AttachmentRef(
                                url: response.url,
                                dataBase64: nil,
                                mediaType: attachment.mediaType,
                                name: attachment.filename
                            ))
                        } catch {
                            throw ChatSendError.attachmentUploadFailed(attachment.filename)
                        }
                    }
                }
                self.lastSent = (trimmed, runtimeModelId, effort, attachmentRefs)

                let sessionId = try await self.ensureSessionId()
                self.isCreatingSession = false
                self.streamRequestStartedAt = Date()
                self.streamFirstDeltaAt = nil
                // POST /api/agent-chat — the response body IS the frame stream.
                // agentId rides along so the bridge composes persona +
                // workspace files + response-style prefs server-side.
                for try await event in self.chatClient.sendMessageStream(
                    sessionId: sessionId,
                    text: trimmed,
                    agentId: self.sessionContext.agentId,
                    runtimeModelId: runtimeModelId,
                    effort: effort,
                    attachments: attachmentRefs.isEmpty ? nil : attachmentRefs,
                    tools: ToolOptionsStore.shared.optionsForSend
                ) {
                    try Task.checkCancellation()
                    if self.apply(event, to: assistantId) {
                        streamFailed = true
                    }
                }
                // Terminal frame arrived (finish / done / [DONE]) and the
                // stream finished.
                self.finishStreaming(messageId: assistantId)
                let generator = UINotificationFeedbackGenerator()
                generator.notificationOccurred(streamFailed ? .error : .success)
            } catch is CancellationError {
                // Local cancel (stop button / backgrounding). Whoever cancelled
                // owns the state cleanup — just don't lose buffered deltas.
                self.isCreatingSession = false
                self.flushPendingDeltas()
            } catch let error as URLError where error.code == .cancelled {
                // URLSession surfaces task cancellation as URLError.cancelled.
                self.isCreatingSession = false
                self.flushPendingDeltas()
            } catch {
                self.isCreatingSession = false
                if self.streamingMessageIsPristine(assistantId) {
                    // Nothing ever streamed back (session-create / first-byte
                    // failure): roll back the optimistic pair, hand the draft
                    // back to the composer, and surface one banner.
                    self.messages.removeAll { $0.id == assistantId || $0.id == userMessageId }
                    self.draftToRestore = trimmed
                    self.transientError = "Couldn't send the message: \(FriendlyErrorMessage.from(error.localizedDescription))"
                    self.finishStreaming(messageId: assistantId)
                    let generator = UINotificationFeedbackGenerator()
                    generator.notificationOccurred(.error)
                } else {
                    self.failStreaming(messageId: assistantId, error: error)
                }
            }
        }
    }

    /// Applies one `AgentChatEvent` to the streaming assistant message.
    /// Returns true when the stream failed (error frames / finish "error") —
    /// used for the completion haptic.
    private func apply(_ event: AgentChatEvent, to messageId: String) -> Bool {
        switch event {
        case .textDelta(let payload):
            if streamFirstDeltaAt == nil {
                streamFirstDeltaAt = Date()
            }
            pendingText += payload.text
            scheduleFlush()

        case .thinkingDelta(let text):
            pendingReasoning += text
            scheduleFlush()

        case .toolCall(let payload):
            updateMessage(messageId) { message in
                message.toolStatus = MessageRecord.ToolStatus(
                    toolName: payload.toolName,
                    text: payload.toolName,
                    state: .running,
                    startedAt: Date()
                )
                message.upsertToolCall(id: payload.toolCallId, name: payload.toolName, state: .running, detail: payload.detail)
            }

        case .toolResult(let payload):
            updateMessage(messageId) { message in
                message.upsertToolCall(id: payload.toolCallId, name: payload.toolName, state: .done, detail: payload.detail)
                message.toolStatus?.state = .done
                if let startedAt = message.toolStatus?.startedAt {
                    message.toolStatus?.elapsed = Date().timeIntervalSince(startedAt)
                }
            }

        case .toolError(let payload):
            updateMessage(messageId) { message in
                var detail = payload.detail ?? AgentChatEvent.ToolDetail()
                detail.error = detail.error ?? payload.error
                message.upsertToolCall(id: payload.toolCallId, name: payload.toolName ?? message.toolStatus?.toolName ?? "Tool", state: .failed, detail: detail)
                message.toolStatus?.state = .failed
                message.toolStatus?.text = payload.error
                if let startedAt = message.toolStatus?.startedAt {
                    message.toolStatus?.elapsed = Date().timeIntervalSince(startedAt)
                }
            }

        case .artifact(let artifact):
            // Typed artifact frame — no markdown-fence regex. The card
            // appears as soon as the frame arrives.
            let record = ArtifactRecord(agentChat: artifact)
            updateMessage(messageId) { message in
                message.artifacts.append(record)
            }
            // Streams are the only artifact source (no backend list
            // endpoint), so the library collects them as they arrive.
            ArtifactLibraryStore.shared.record(record, sessionId: currentSessionId)

        case .finish(let payload):
            guard payload.status == "error" else { break }
            flushPendingDeltas()
            let raw = payload.metadata?.error ?? "The generation failed."
            let card = ChatError(
                name: payload.metadata?.errorDetails?.name,
                message: payload.metadata?.errorDetails?.message,
                raw: raw
            )
            updateMessage(messageId) { message in
                message.error = card
            }
            return true

        case .streamError(let reason):
            flushPendingDeltas()
            updateMessage(messageId) { message in
                message.error = ChatError(name: nil, message: reason, raw: reason)
            }
            return true

        case .question(let request):
            updateMessage(messageId) { message in
                if !message.questions.contains(where: { $0.id == request.id }) {
                    message.questions.append(request)
                }
            }

        case .permissionAsked(let ask):
            updateMessage(messageId) { message in
                if !message.permissions.contains(where: { $0.id == ask.id }) {
                    message.permissions.append(ask)
                }
            }

        case .messageStart, .done, .ignored:
            // messageStart carries only model labels (not rendered); done is
            // the bare terminal signal; ignored frames never reach here (the
            // client filters them) but the case is kept for exhaustiveness.
            break
        }
        return false
    }

    /// Stop button: cancels the local consumer and aborts the server-side
    /// generation (POST /api/v1/agent-sessions/:id/abort).
    func stopStreaming() {
        let sessionId = currentSessionId

        streamTask?.cancel()
        streamTask = nil
        flushPendingDeltas()

        if let messageId = streamingMessageId {
            updateMessage(messageId) { $0.isStreaming = false }
        }
        isStreaming = false
        streamingMessageId = nil

        if let sessionId {
            Task { [chatClient] in
                try? await chatClient.abort(sessionId: sessionId)
            }
        }
    }

    // MARK: - scenePhase lifecycle

    /// Called from ChatView's `scenePhase` handler. The SSE connection cannot
    /// survive backgrounding: on `.background` cancel local consumption (the
    /// generation keeps running server-side); on `.active` reload the
    /// session's messages — the canonical server state IS the reconcile path
    /// (the live protocol has no reply-state endpoint to diff against).
    func handleScenePhase(_ phase: ScenePhase) {
        switch phase {
        case .background:
            streamTask?.cancel()
            streamTask = nil
            flushPendingDeltas()
            // The local consumer is gone; mark the streaming placeholder as
            // no longer streaming so the UI doesn't stay stuck if the user
            // never foregrounds. The next reconcile will settle the server
            // state (or the abort below if the message had landed).
            if let messageId = streamingMessageId {
                updateMessage(messageId) { $0.isStreaming = false }
                streamingMessageId = nil
                isStreaming = false
            }
        case .active:
            Task { [weak self] in
                await self?.reconcileSession()
            }
        default:
            break
        }
    }

    /// Re-renders the current session from `GET .../messages` after a
    /// foreground return. Skipped while a stream is still live in-process
    /// (e.g. an `.inactive` → `.active` bounce that never severed the
    /// connection).
    private func reconcileSession() async {
        guard streamTask == nil else { return }

        // A stream cancelled by backgrounding is no longer streaming locally.
        if let messageId = streamingMessageId {
            updateMessage(messageId) { $0.isStreaming = false }
            streamingMessageId = nil
            isStreaming = false
        }

        guard let sessionId = currentSessionId else { return }

        do {
            let records = try await chatClient.listMessages(sessionId: sessionId)
            // Bail if the user switched sessions or started a new stream
            // while the reload was in flight.
            guard self.currentSessionId == sessionId, self.streamTask == nil else { return }
            self.messages = records.map(Self.mapRecord)
            self.transientError = nil
        } catch {
            self.transientError = "Couldn't refresh the conversation: \(FriendlyErrorMessage.from(error.localizedDescription))"
        }
    }

    // MARK: - Flush & finalize

    /// Schedules the ~50ms coalescing flush; no-op while one is pending.
    private func scheduleFlush() {
        guard flushTask == nil else { return }
        flushTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: Self.flushIntervalNs)
            guard let self, !Task.isCancelled else { return }
            self.flushPendingDeltas()
        }
    }

    /// Applies buffered text/reasoning deltas to the streaming message.
    private func flushPendingDeltas() {
        flushTask?.cancel()
        flushTask = nil

        let text = pendingText
        let reasoning = pendingReasoning
        pendingText = ""
        pendingReasoning = ""

        guard !text.isEmpty || !reasoning.isEmpty,
              let messageId = streamingMessageId else { return }
        updateMessage(messageId) { message in
            message.content += text
            message.reasoning += reasoning
        }
    }

    /// Normal end of a stream: flush, clear streaming flags and bookkeeping.
    private func finishStreaming(messageId: String) {
        flushPendingDeltas()

        // Perf stats need a request-start and a first-delta timestamp — both
        // absent for e.g. an error that failed before any text arrived, in
        // which case there's nothing honest to show.
        if let requestStartedAt = streamRequestStartedAt,
           let firstDeltaAt = streamFirstDeltaAt {
            let generationDuration = Date().timeIntervalSince(firstDeltaAt)
            let charCount = messages.first(where: { $0.id == messageId })?.content.count ?? 0
            let stats = MessageRecord.PerfStats(
                timeToFirstToken: firstDeltaAt.timeIntervalSince(requestStartedAt),
                generationDuration: generationDuration,
                approxTokenCount: max(1, charCount / 4)
            )
            updateMessage(messageId) { $0.perfStats = stats }
        }
        streamRequestStartedAt = nil
        streamFirstDeltaAt = nil

        updateMessage(messageId) { $0.isStreaming = false }
        isStreaming = false
        streamingMessageId = nil
        streamTask = nil

        // Streaming consumes quota — refresh the usage meter (Phase 5;
        // UsageStore dedupes concurrent fetches).
        UsageStore.shared.refresh()

        // Response notification (Phase 5): only when the app isn't in the
        // foreground — foreground completion already plays a haptic, and
        // NotificationService no-ops unless the user granted authorization
        // via the opt-in card.
        if UIApplication.shared.applicationState != .active {
            let preview = messages.first(where: { $0.id == messageId })?.content ?? ""
            Task {
                await NotificationService.postResponseNotification(preview: String(preview.prefix(120)))
            }
        }
    }

    /// Transport/creation failure: surface the error inline, then finalize.
    /// Intentional cancels (CancellationError / URLError.cancelled) stay silent.
    private func failStreaming(messageId: String, error: Error) {
        if error is CancellationError { return }
        if let urlError = error as? URLError, urlError.code == .cancelled { return }
        flushPendingDeltas()
        updateMessage(messageId) { message in
            message.error = .connectionInterrupted(error.localizedDescription)
        }
        finishStreaming(messageId: messageId)
        let generator = UINotificationFeedbackGenerator()
        generator.notificationOccurred(.error)
    }

    /// True while the streaming assistant message has received nothing —
    /// no deltas buffered or applied, no tool activity, no artifacts. Used
    /// to tell "send never landed" (roll back + restore draft) apart from a
    /// mid-stream interruption (keep the partial reply, inline error).
    private func streamingMessageIsPristine(_ messageId: String) -> Bool {
        guard pendingText.isEmpty, pendingReasoning.isEmpty,
              let message = messages.first(where: { $0.id == messageId }) else { return false }
        return message.content.isEmpty && message.toolStatus == nil && message.artifacts.isEmpty
            && message.toolCalls.isEmpty && message.questions.isEmpty
    }

    private func updateMessage(_ id: String, _ mutate: (inout MessageRecord) -> Void) {
        guard let index = messages.firstIndex(where: { $0.id == id }) else { return }
        mutate(&messages[index])
    }
}
