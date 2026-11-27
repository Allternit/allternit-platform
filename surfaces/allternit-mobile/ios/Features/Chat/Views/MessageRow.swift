import SwiftUI
import MarkdownView
import UIKit
import QuickLook
import AllternitFilesKit

struct MessageRow: View {
    let message: MessageRecord
    let onArtifactTap: (ArtifactRecord) -> Void
    /// Error-card actions: model errors offer "Choose a model", everything
    /// else offers "Retry" (re-streams the last sent text).
    var onChooseModel: () -> Void = {}
    var onRetry: (String) -> Void = { _ in }
    /// A failed picture that needs ChatGPT sign-in: opens the sign-in browser.
    var onSignInToChatGPT: () -> Void = {}
    var availableErrorActions: Set<ChatErrorAction.Kind> = []
    var onErrorAction: (String, ChatErrorAction) -> Void = { _, _ in }
    /// Phase 8 action bar: true only on the most recent assistant reply —
    /// its row gets the retry (regenerate) button.
    var isLastAssistant: Bool = false
    /// Phase 8 edit: true only on the most recent user bubble — its
    /// long-press menu gets "Edit" (fill the composer; re-sending truncates
    /// the conversation after it).
    var isLastUser: Bool = false
    var onRegenerate: () -> Void = {}
    var onShareLink: (() -> Void)? = nil
    var onEdit: () -> Void = {}
    /// Earlier versions of this user message (it was edited and re-sent),
    /// newest first; non-empty shows the "Edited" chip.
    var editedVersions: [MessageBranch] = []
    /// The chat's agent session — the run tree's task list and the pending
    /// questions/permissions lookup. Nil before the first send.
    var sessionId: String? = nil
    /// Sends a prompt to the agent as a new turn (run tree Retry).
    var onSendPrompt: ((String) -> Void)? = nil
    /// The phone or computer run this reply hosts (`PhoneRunHosting`): its pill sits on the
    /// reply's top-left and its steps under the reply text, before the action icons.
    var hostedRun: PhoneTaskCardModel? = nil

    @State private var runStepsOpen = false
    @State private var isThinkingExpanded = false
    @State private var showsRunTree = false
    /// media_generate calls whose dot reveal is playing. The bubble (which may
    /// hold the same finished picture) waits until the reveal ends, so the
    /// picture shows once, in the place it will live.
    @State private var revealingCalls: Set<String> = []
    private var holdsBubble: Bool { message.role == "assistant" && !revealingCalls.isEmpty }
    @State private var showsEditedVersions = false
    @ObservedObject private var asks = AgentAskStore.shared
    /// Settings → Capabilities → Artifacts gates the inline artifact cards;
    /// observed so toggling it applies to already-rendered rows.
    @ObservedObject private var settings = SettingsStore.shared

    /// In a bot's chat your bubbles wear the bot's colour (30% over the card).
    @Environment(\.botTint) private var botTint
    /// The chat's bot, which names the screen in a screen context reply.
    @Environment(\.chatBot) private var chatBot

    var body: some View {
        // Voice-mode summary card (Phase 7b): a centered card row with no
        // avatars/bubble — the conversation itself renders as normal turns.
        if let voiceSummary = message.voiceSummary {
            VoiceSummaryCardView(summary: voiceSummary)
                .frame(maxWidth: .infinity)
                .padding(.horizontal, 12)
        } else {
            messageRowBody
        }
    }

    private var messageRowBody: some View {
        // Web chat: the user's message is a soft bubble on the right (max
        // 80% wide); the reply is plain text across the column — no avatars.
        HStack(alignment: .top, spacing: 0) {
            if message.role == "user" {
                Spacer(minLength: 0)
            }

            VStack(alignment: message.role == "user" ? .trailing : .leading, spacing: 8) {
                // Collapsible "thinking" block (reply.reasoning.delta)
                if !message.reasoning.isEmpty {
                    thinkingBlock
                }

                // Tool calls on a rail (Features/RunTree). Falls back to the
                // one-line status for streams that only name a tool.
                if !message.toolCalls.isEmpty {
                    ToolCallsRail(
                        calls: message.toolCalls,
                        isStreaming: message.isStreaming,
                        permissions: rowPermissions,
                        onOpenRunTree: { showsRunTree = true },
                        onRetry: isLastAssistant && !message.isStreaming ? onRegenerate : nil
                    )
                } else if let toolStatus = message.toolStatus {
                    toolStatusLine(toolStatus)
                }

                // load_skill: the skill's steps as a checklist under the turn.
                ForEach(message.toolCalls) { call in
                    if SkillResult.isLoadSkill(call.toolName), call.state == .done,
                       let result = SkillResult.parse(call.output ?? ""), !result.steps.isEmpty {
                        SkillChecklistCard(
                            name: SkillNames.name(for: result.id), result: result,
                            ticks: SkillResult.ticks(steps: result.steps, calls: message.toolCalls)
                        )
                    }
                }

                // media_generate: the generation's progress (pulse, preview or
                // frame N of M) while it runs, the failure card if it fails.
                ForEach(message.toolCalls) { call in
                    if GenerationProgressStore.isMediaGenerate(call.toolName) {
                        MediaGenerateCard(
                            call: call,
                            isNewestTurn: isLastAssistant,
                            replyIsLive: message.isStreaming,
                            onChooseModel: onChooseModel,
                            onRetry: { onRetry(message.id) },
                            onSignInToChatGPT: onSignInToChatGPT,
                            onRevealing: { revealing in
                                if revealing { revealingCalls.insert(call.id) } else { revealingCalls.remove(call.id) }
                            }
                        )
                    }
                }

                // One-line run status ("phase - note"), only while busy.
                if message.isStreaming, let busy = message.busyLine, !busy.isEmpty {
                    Text(busy)
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                        .accessibilityLabel("Status: \(busy)")
                }

                // Message tools: a draft card, or the held-for-approval line.
                ForEach(message.toolCalls) { call in
                    switch MessageToolResult.parse(toolName: call.toolName, output: call.output) {
                    case .draft(let draft): MessageDraftCard(draft: draft)
                    case .held(let inboxId): MessageHeldLine(inboxId: inboxId)
                    case nil: EmptyView()
                    }
                }

                // capture_screen_context: the agent looked at the screen.
                if message.role == "assistant", let looked = lookedAtScreen {
                    ScreenContextChip(label: looked)
                }

                // browser_tasks: one card per call (status, result, final page).
                ForEach(message.toolCalls) { call in
                    if call.toolName == "browser_tasks", let output = call.output {
                        let tasks = BrowserTaskResult.parse(output)
                        if !tasks.isEmpty { BrowserTasksCard(tasks: tasks) }
                    }
                }

                // run_code: produced files (images, CSV grids, downloads) + run summary.
                ForEach(message.toolCalls) { call in
                    if let result = call.runCode, call.toolName == "run_code" {
                        RunCodeFilesView(result: result)
                    }
                }

                // send_user_message: the agent's message to you, with its files.
                ForEach(message.toolCalls) { call in
                    if let sent = call.userMessage, call.toolName == AgentUserMessage.toolName {
                        AgentUserMessageCard(message: sent)
                    }
                }

                // automation: the scheduled task the agent made or changed, with Cancel.
                ForEach(message.toolCalls) { call in
                    if call.toolName == AgentAutomationResult.toolName, let result = call.automation, let task = result.card {
                        AgentAutomationCard(task: task, via: result.via)
                    }
                }

                // Gap tools: one stub card per tool (G4-G7 fill them in).
                ForEach(message.toolCalls) { call in
                    if let gap = call.gap {
                        switch call.toolName {
                        case GapToolMetadata.secondOpinion: SecondOpinionCard(gap: gap)
                        case GapToolMetadata.prComments: PrCommentsCard(gap: gap)
                        case GapToolMetadata.statusUpdate: StatusUpdateRow(gap: gap)
                        case GapToolMetadata.feedbackNote: FeedbackNoteCard(gap: gap)
                        case GapToolMetadata.userLocation: UserLocationCard(gap: gap, onSend: onSendPrompt)
                        case GapToolMetadata.voiceControl: VoiceControlRow(gap: gap)
                        case GapToolMetadata.endConversation: EndConversationRow(gap: gap)
                        case GapToolMetadata.worktreeEnter, GapToolMetadata.worktreeExit: WorktreeCard(gap: gap)
                        default: EmptyView()
                        }
                    }
                }

                // agent_batch / sleep: progress card and wait row.
                ForEach(message.toolCalls) { call in
                    let n = ToolDescriber.normalized(call.toolName)
                    if n == "agentbatch" {
                        BatchProgressCard(call: call)
                    } else if n == "sleep" {
                        SleepRow(call: call)
                    }
                }

                // lookup: the fact card (weather, time zones, conversion, link,
                // places, images) drawn from the tool part, never retyped by the model.
                ForEach(message.toolCalls) { call in
                    if call.toolName == LookupToolMetadata.toolName, let card = call.lookupCard {
                        AnswerCardView(card: card, fallbackText: call.output)
                    }
                }

                // connector_suggest: the connectors the agent offered, with Connect.
                // settings: what changed, with Undo / Confirm / Open Settings.
                ForEach(message.toolCalls) { call in
                    if let suggestion = AgentConnectorSuggestion.parse(
                        toolName: call.toolName, metadata: call.metadata, output: call.output, input: call.input, title: call.title
                    ) {
                        ConnectSuggestCardView(suggestion: suggestion)
                    } else if let settingsResult = AgentSettingsResult.parse(
                        toolName: call.toolName, metadata: call.metadata, output: call.output
                    ) {
                        SettingsChangeCardView(result: settingsResult, toolCallId: call.id, autoApply: message.isStreaming)
                    }
                }

                // MCP 2027: Code Mode script rows and typed (structuredContent) results,
                // read from the tool part's `state.metadata.mcpScript` / `.mcp`.
                ForEach(message.toolCalls) { call in
                    if let script = McpMetaReader.script(call.metadata) {
                        McpScriptRowView(meta: script)
                    } else if let mcp = McpMetaReader.tool(call.metadata), mcp.structured != nil {
                        McpTypedResultView(meta: mcp)
                    }
                }

                // Message Bubble — markdown for the assistant, plain text for the user.
                // Skipped when the message is nothing but an error card.
                if let run = hostedRun, message.role == "assistant" {
                    VStack(alignment: .leading, spacing: 6) {
                        HStack(spacing: 8) {
                            RunStatusPill(card: run)
                            if run.status.isActive || run.endedAt != nil {
                                RunElapsedText(card: run)
                                    .font(.system(size: 12))
                                    .foregroundColor(Color("TextSecondary"))
                            }
                        }
                        if (!message.content.isEmpty || message.error == nil) && !holdsBubble {
                            bubble
                        }
                        if run.status.isActive, run.step > 0 {
                            RunStepBar(step: run.step, tint: run.status.tint)
                                .frame(maxWidth: 220, alignment: .leading)
                        }
                        // A finished run folds to its pill line; tap the line to open the steps.
                        if run.status.isActive || runStepsOpen {
                            RunThoughtStream(steps: run.steps, active: run.status.isActive)
                        } else if !run.steps.isEmpty {
                            Button { withAnimation(.easeOut(duration: 0.15)) { runStepsOpen = true } } label: {
                                Text("Show \(run.steps.count) \(run.steps.count == 1 ? "step" : "steps")")
                                    .font(.system(size: 12, weight: .medium))
                                    .foregroundColor(Color("TextSecondary"))
                                    .frame(minHeight: 28)
                                    .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                        }
                    }
                } else if (!message.content.isEmpty || message.error == nil) && !holdsBubble {
                    bubble
                }

                // Memories the reply used or changed (`metadata.memoryRefs`).
                if message.role == "assistant", !message.memoryRefs.isEmpty {
                    MemoryRefsChip(refs: message.memoryRefs)
                }

                if message.role == "user", !editedVersions.isEmpty {
                    Button { showsEditedVersions = true } label: {
                        Text("Edited")
                            .font(.system(size: 11, weight: .medium))
                            .foregroundColor(Color("TextSecondary"))
                            .padding(.horizontal, 8)
                            .padding(.vertical, 3)
                            .background(Capsule().fill(Color("FillNeutral")))
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Edited. Show earlier versions")
                    .sheet(isPresented: $showsEditedVersions) {
                        EditedVersionsSheet(branches: editedVersions)
                    }
                }

                // What the agent waits on: questions answered in place, and
                // permissions not already drawn on a rail row.
                if message.role == "assistant" {
                    ForEach(rowQuestions) { request in
                        QuestionCardView(request: request)
                    }
                    ForEach(unplacedPermissions) { ask in
                        PermissionAskView(ask: ask)
                    }
                    if McpTasksFlag.isOn {
                        McpTasksSection(taskIds: message.mcpTaskIds, formIds: message.mcpFormIds)
                    }
                }

                // Structured failure card (replaces raw backend error text)
                if let error = message.error {
                    ChatErrorCardView(
                        error: error,
                        onChooseModel: onChooseModel,
                        onRetry: { onRetry(message.id) },
                        availableActions: availableErrorActions,
                        onAction: { onErrorAction(message.id, $0) }
                    )
                }

                ACILocalImageTurnView(messageId: message.id)

                // Inline Artifact Card attachments (typed artifact.created
                // events) — gated by Settings → Capabilities → Artifacts.
                if settings.artifactsEnabled {
                    ForEach(message.artifacts) { artifact in
                        artifactCard(artifact)
                    }
                }

                // Phase 8 action bar (Claude parity): copy / share / speak /
                // 👍 / 👎 / retry under completed assistant replies. Hidden
                // while streaming and on error cards (the card owns retry).
                if message.role == "assistant", !message.isStreaming, message.error == nil {
                    MessageActionBar(
                        message: message,
                        isLastAssistant: isLastAssistant,
                        onRegenerate: onRegenerate,
                        onShareLink: onShareLink
                    )

                    if let stats = message.perfStats {
                        perfStatsLine(stats)
                    }
                }

                // Follow-ups: only under the newest finished reply, from its own
                // closing "Next step:" line (web 5.2). Hidden while streaming.
                if message.role == "assistant", isLastAssistant, !message.isStreaming,
                   message.error == nil, let onSendPrompt {
                    let followUps = FollowUps.fromText(message.content)
                    if !followUps.isEmpty {
                        FollowUpChips(prompts: followUps, onPick: onSendPrompt)
                            .id("followups-\(message.id)")
                    }
                }
            }

            .frame(maxWidth: message.role == "user" ? UIScreen.main.bounds.width * 0.8 : .infinity,
                   alignment: message.role == "user" ? .trailing : .leading)
        }
        .padding(.horizontal, 16)
        // What's on screen, for capture_screen_context (ScreenContextResponder).
        .onAppear { reportOnScreen() }
        .onChange(of: message.content) { _, _ in reportOnScreen() }
        .onChange(of: sessionId) { _, _ in reportOnScreen() }
        .onDisappear {
            ScreenContextResponder.shared.hide(messageId: message.id)
            revealingCalls.removeAll()
        }
        // Safety: the hold never outlives ~1.5 s, whatever happened to the reveal.
        .task(id: revealingCalls) {
            guard !revealingCalls.isEmpty else { return }
            try? await Task.sleep(nanoseconds: 1_500_000_000)
            if !Task.isCancelled { revealingCalls.removeAll() }
        }
        .sheet(isPresented: $showsRunTree) {
            AgentRunTreeSheet(
                steps: message.toolCalls,
                sessionId: sessionId,
                isStreaming: message.isStreaming,
                onRetry: onSendPrompt
            )
            .presentationDetents([.medium, .large])
        }
        .task(id: asksRefreshKey) {
            // The latest reply re-reads what the agent waits on when it
            // appears and when its stream settles (a reopened chat whose
            // agent still waits gets its card back).
            guard message.role == "assistant", isLastAssistant, let sessionId else { return }
            await asks.refresh(sessionId: sessionId)
        }
    }

    // MARK: - Screen context

    private func reportOnScreen() {
        let title: String
        if let chatBot, !chatBot.isPrimaryTemplateBot {
            title = "Chat with \(chatBot.name)"
        } else {
            title = "Chat"
        }
        ScreenContextResponder.shared.show(
            messageId: message.id,
            sessionId: sessionId,
            role: message.role,
            text: message.role == "user" ? ToolOptionsStore.shared.strippedText(message.content) : message.content,
            createdAt: message.createdAt,
            title: title
        )
    }

    /// "Looked at your screen" when a capture_screen_context call on this
    /// reply got a capture (a capture's output is spotlighted; a miss is the
    /// runtime's plain "didn't answer" text).
    private var lookedAtScreen: String? {
        let looked = message.toolCalls.contains { call in
            call.toolName == "capture_screen_context" && call.state == .done
                && (call.output?.contains("<<untrusted") ?? false)
        }
        return looked ? "Looked at your screen" : nil
    }

    // MARK: - Agent asks

    private var asksRefreshKey: String {
        "\(sessionId ?? "")|\(isLastAssistant)|\(message.isStreaming)"
    }

    /// The reply's own questions, plus — on the latest reply — any the
    /// session still lists. Answered ones drop out (AgentAskStore).
    private var rowQuestions: [AgentQuestionRequest] {
        var list = message.questions
        if isLastAssistant {
            for pending in asks.questions(for: sessionId) where !list.contains(where: { $0.id == pending.id }) {
                list.append(pending)
            }
        }
        return list.filter { !asks.isResolved($0.id) || message.questions.contains($0) }
    }

    private var rowPermissions: [AgentPermissionAsk] {
        var list = message.isStreaming ? message.permissions : []
        if isLastAssistant {
            for pending in asks.permissions(for: sessionId) where !list.contains(where: { $0.id == pending.id }) {
                list.append(pending)
            }
        }
        // A bound thread's approvals show once, in BoundEngineApprovals.
        return list.filter { !asks.isResolved($0.id) && !asks.isBound(sessionId) }
    }

    /// Permissions whose tool call isn't on this reply's rail get a card.
    private var unplacedPermissions: [AgentPermissionAsk] {
        rowPermissions.filter { ask in
            !(message.isStreaming && message.toolCalls.contains { $0.id == ask.toolCallId })
        }
    }

    // MARK: - Subviews

    private var userBubbleFill: AnyShapeStyle {
        guard let botTint else { return AnyShapeStyle(Color("FillNeutral")) }
        let base = UIColor(named: "BgPanel") ?? .systemBackground
        let tint = UIColor(Color(hex: botTint))
        return AnyShapeStyle(Color(UIColor { traits in
            let a = base.resolvedColor(with: traits), b = tint.resolvedColor(with: traits)
            var (r1, g1, b1, a1): (CGFloat, CGFloat, CGFloat, CGFloat) = (0, 0, 0, 0)
            var (r2, g2, b2, a2): (CGFloat, CGFloat, CGFloat, CGFloat) = (0, 0, 0, 0)
            a.getRed(&r1, green: &g1, blue: &b1, alpha: &a1)
            b.getRed(&r2, green: &g2, blue: &b2, alpha: &a2)
            return UIColor(red: r1 + (r2 - r1) * 0.3, green: g1 + (g2 - g1) * 0.3, blue: b1 + (b2 - b1) * 0.3, alpha: 1)
        }))
    }

    @ViewBuilder
    private var bubble: some View {
        if message.role == "user" {
            // History stores the sent text, tool tags included; show what was typed.
            Text(ToolOptionsStore.shared.strippedText(message.content))
                .font(.system(size: 15))
                .lineSpacing(15 * 0.6 - 4)
                .foregroundColor(Color("TextPrimary"))
                .padding(.horizontal, 16)
                .padding(.vertical, 10)
                .background(userBubbleFill, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                .textSelection(.enabled)
                .contextMenu { bubbleContextMenu }
        } else if message.isStreaming, message.content.isEmpty, message.reasoning.isEmpty,
                  message.toolCalls.isEmpty, message.toolStatus == nil {
            // Waiting on the first words: the working mark (web LiveStatusRow).
            HStack(spacing: 8) {
                WorkingMark(state: .thinking, size: 16)
                Text("Working…")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
            }
            .accessibilityElement(children: .combine)
        } else {
            VStack(alignment: .leading, spacing: 10) {
                AssistantContent(content: message.content, isStreaming: message.isStreaming)
                    .font(.system(size: 15))
                    .foregroundColor(Color("TextPrimary"))
                    .contextMenu { bubbleContextMenu }
                // Still writing: the orb (or the bot) writes under the reply (web StreamingBubble).
                if message.role == "assistant", message.isStreaming {
                    WorkingMark(state: .writing, size: 16)
                        .frame(height: 20)
                }
            }
        }
    }

    @ViewBuilder
    private var bubbleContextMenu: some View {
        // Phase 8: "Edit" on the last user bubble — fills the composer with
        // the message text; re-sending truncates the conversation after it
        // (ChatViewModel.resendEditedMessage).
        if message.role == "user", isLastUser {
            Button(action: onEdit) {
                Label("Edit", systemImage: "pencil")
            }
        }

        Button(action: {
            UIPasteboard.general.string = FileCitation.plainText(message.content)
        }) {
            Label("Copy Text", systemImage: "doc.on.doc")
        }
    }

    @ViewBuilder
    private var thinkingBlock: some View {
        VStack(alignment: .leading, spacing: 6) {
            Button(action: {
                withAnimation(.easeInOut(duration: 0.2)) {
                    isThinkingExpanded.toggle()
                }
            }) {
                HStack(spacing: 6) {
                    Image(systemName: "brain")
                        .font(.caption)
                    Text(message.isStreaming && message.content.isEmpty ? "Thinking…" : "Thinking")
                        .font(.caption)
                        .fontWeight(.semibold)
                    Image(systemName: isThinkingExpanded ? "chevron.up" : "chevron.down")
                        .font(.caption2)
                }
                .foregroundColor(Color("TextSecondary"))
            }
            .buttonStyle(.plain)

            if isThinkingExpanded {
                Text(message.reasoning)
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
                    .padding(10)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Color("BgPrimary"))
                    .cornerRadius(10)
            }
        }
    }

    @ViewBuilder
    private func toolStatusLine(_ status: MessageRecord.ToolStatus) -> some View {
        HStack(spacing: 6) {
            Group {
                switch status.state {
                case .running:
                    WorkingMark(state: WorkingMark.state(forTool: status.toolName), size: 16)
                case .done:
                    Image(systemName: "checkmark.circle.fill")
                        .foregroundColor(Color("AccentPrimary"))
                case .failed:
                    Image(systemName: "exclamationmark.triangle.fill")
                        .foregroundColor(.orange)
                }
            }
            .frame(width: 20, height: 20)

            Text(status.text)
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
                .lineLimit(1)

            // Elapsed time only — the wire protocol carries no result
            // size/token count for tool calls, so there's nothing honest to
            // show beyond how long it ran (PocketPal's ToolMetricsFooter
            // shows token count too, but that's backed by real llama.cpp
            // data we don't have here).
            if let elapsed = status.elapsed {
                Text("· \(formattedElapsed(elapsed))")
                    .font(.caption2)
                    .foregroundColor(Color("TextSecondary").opacity(0.7))
            }
        }
    }

    private func formattedElapsed(_ seconds: TimeInterval) -> String {
        seconds < 1 ? String(format: "%.0fms", seconds * 1000) : String(format: "%.1fs", seconds)
    }

    /// Client-side timing readout under a completed reply (PocketPal's
    /// AssistantTurnFooter-inspired) — the `~` is load-bearing, not
    /// decorative: token count is a chars/4 estimate, and the whole readout
    /// is wall-clock timing over the network, not real inference-engine
    /// telemetry (see MessageRecord.PerfStats).
    private func perfStatsLine(_ stats: MessageRecord.PerfStats) -> some View {
        Text("\(formattedElapsed(stats.timeToFirstToken)) to first token · ~\(Int(stats.approxTokensPerSecond)) tok/s")
            .font(.caption2)
            .foregroundColor(Color("TextSecondary").opacity(0.7))
            .padding(.leading, 8)
    }

    @ViewBuilder
    private func artifactCard(_ artifact: ArtifactRecord) -> some View {
        Button(action: { onArtifactTap(artifact) }) {
            HStack(spacing: 12) {
                Image(systemName: artifact.fileType.lowercased() == "svg" ? "paintpalette" : "doc.text.fill")
                    .font(.title3)
                    .foregroundColor(Color("AccentPrimary"))

                VStack(alignment: .leading, spacing: 2) {
                    Text(artifact.title)
                        .font(.subheadline)
                        .fontWeight(.semibold)
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)

                    Text(artifact.isPreviewable ? "Click to open interactive preview" : "Click to view")
                        .font(.caption2)
                        .foregroundColor(Color("TextSecondary"))
                }

                Spacer()

                Image(systemName: "chevron.right")
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }
            .padding(12)
            .frame(width: 250)
            .background(Color("BgPrimary"))
            .cornerRadius(12)
            .overlay(
                RoundedRectangle(cornerRadius: 12)
                    .stroke(Color("BorderSubtle"), lineWidth: 1)
            )
        }
        .transition(.slide)
    }
}

// MARK: - Screen context chip (capture_screen_context)

/// A small chip on a reply whose agent looked at the screen.
struct ScreenContextChip: View {
    let label: String

    var body: some View {
        HStack(spacing: 5) {
            Image(systemName: "eye")
                .accessibilityHidden(true)
            Text(label)
        }
        .font(.system(size: 11, weight: .medium))
        .foregroundColor(Color("TextSecondary"))
        .padding(.horizontal, 8)
        .padding(.vertical, 3)
        .background(Capsule().fill(Color("FillNeutral")))
        .accessibilityElement(children: .combine)
    }
}

// MARK: - Hidden browser tasks (browser_tasks)

/// One task's outcome from a browser_tasks call:
/// `{id, status: done|needs_login|needs_user|failed, result, final_url}`.
struct BrowserTaskResult: Identifiable, Equatable, Sendable {
    let id: String
    let status: String
    let result: String
    let finalURL: URL?

    /// Reads the tool's output: an array of results, `{results|tasks: […]}`,
    /// or one result; JSON found inside surrounding text (a spotlight
    /// wrapper) is read too. Empty when the output isn't task results.
    static func parse(_ output: String) -> [BrowserTaskResult] {
        guard let json = jsonValue(output) else { return [] }
        let items: [Any]
        if let list = json as? [Any] {
            items = list
        } else if let object = json as? [String: Any] {
            if let list = (object["results"] ?? object["tasks"]) as? [Any] {
                items = list
            } else {
                items = [object]
            }
        } else {
            return []
        }
        return items.enumerated().compactMap { index, item in
            guard let object = item as? [String: Any], let status = object["status"] as? String else { return nil }
            let id = (object["id"] as? String) ?? "task-\(index + 1)"
            let result: String
            if let text = object["result"] as? String {
                result = text
            } else if let value = object["result"], !(value is NSNull),
                      let data = try? JSONSerialization.data(withJSONObject: value, options: [.fragmentsAllowed]),
                      let text = String(data: data, encoding: .utf8) {
                result = text
            } else {
                result = ""
            }
            let raw = (object["final_url"] ?? object["finalUrl"]) as? String
            let url = raw.flatMap { URL(string: $0) }.flatMap { ["http", "https"].contains($0.scheme?.lowercased() ?? "") ? $0 : nil }
            return BrowserTaskResult(id: id, status: status, result: result, finalURL: url)
        }
    }

    private static func jsonValue(_ text: String) -> Any? {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        if let value = try? JSONSerialization.jsonObject(with: Data(trimmed.utf8)) { return value }
        guard let start = trimmed.firstIndex(where: { $0 == "[" || $0 == "{" }),
              let end = trimmed.lastIndex(where: { $0 == "]" || $0 == "}" }),
              start < end else { return nil }
        return try? JSONSerialization.jsonObject(with: Data(trimmed[start...end].utf8))
    }

    var statusLabel: String {
        switch status {
        case "done": return "Done"
        case "needs_login": return "Needs you to sign in"
        case "needs_user": return "Needs you"
        case "failed": return "Failed"
        default: return status.replacingOccurrences(of: "_", with: " ").capitalized
        }
    }

    var statusSymbol: String {
        switch status {
        case "done": return "checkmark.circle.fill"
        case "needs_login": return "person.badge.key"
        case "needs_user": return "hand.raised"
        case "failed": return "exclamationmark.triangle.fill"
        default: return "circle"
        }
    }
}

/// The card for a browser_tasks call: one row per task with its status,
/// a short result, and the page it ended on (opens in Safari, where a
/// sign-in or a step the agent left for you can be finished).
struct BrowserTasksCard: View {
    let tasks: [BrowserTaskResult]

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 6) {
                Image(systemName: "globe")
                    .foregroundColor(Color("AccentPrimary"))
                    .accessibilityHidden(true)
                Text(tasks.count == 1 ? "Browser task" : "Browser tasks · \(tasks.count)")
                    .foregroundColor(Color("TextSecondary"))
            }
            .font(.caption.weight(.semibold))
            .accessibilityAddTraits(.isHeader)

            ForEach(tasks) { task in
                row(task)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("BgPrimary"))
        .cornerRadius(12)
        .overlay(
            RoundedRectangle(cornerRadius: 12)
                .stroke(Color("BorderSubtle"), lineWidth: 1)
        )
        .accessibilityElement(children: .contain)
    }

    private func row(_ task: BrowserTaskResult) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Image(systemName: task.statusSymbol)
                    .foregroundColor(task.status == "failed" ? .orange : Color("AccentPrimary"))
                    .accessibilityHidden(true)
                Text(task.statusLabel)
                    .font(.subheadline.weight(.semibold))
                    .foregroundColor(Color("TextPrimary"))
                if let host = task.finalURL?.host() {
                    Text("· \(host)")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                }
            }
            .accessibilityElement(children: .combine)

            if !task.result.isEmpty {
                Text(task.result)
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(4)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .textSelection(.enabled)
            }

            if let url = task.finalURL {
                Button {
                    // Safari itself, not an in-app view: the user's own sign-ins live there.
                    UIApplication.shared.open(url)
                } label: {
                    Label("Open in Safari", systemImage: "safari")
                        .font(.caption.weight(.semibold))
                }
                .buttonStyle(.borderless)
                .accessibilityHint("Opens \(url.host() ?? "the page") in Safari")
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("FillNeutral")))
    }
}

// MARK: - An agent's message to you (send_user_message)

/// The card for a `send_user_message` call: the text, then one row per
/// file. A file with a Files drive id opens in Quick Look (the same
/// download the Files screens use); one without stays a plain row that
/// says where it is.
struct AgentUserMessageCard: View {
    let message: AgentUserMessage

    @ObservedObject private var files = FilesDrive.shared
    @State private var openingIndex: Int?
    @State private var previewURL: URL?
    @State private var problem: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 6) {
                Image(systemName: "paperplane")
                    .foregroundColor(Color("AccentPrimary"))
                Text(message.push == "sent" ? "Sent to you · Notified" : "Sent to you")
                    .foregroundColor(Color("TextSecondary"))
            }
            .font(.caption.weight(.semibold))
            .accessibilityElement(children: .combine)
            .accessibilityLabel(message.push == "sent" ? "Message from your agent. Your devices were notified." : "Message from your agent")

            if !message.text.isEmpty {
                Text(message.text)
                    .font(.body)
                    .foregroundColor(Color("TextPrimary"))
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .textSelection(.enabled)
            }

            ForEach(Array(message.files.enumerated()), id: \.offset) { index, file in
                fileRow(file, index: index)
            }

            if let problem {
                Text(problem)
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("BgPrimary"))
        .cornerRadius(12)
        .overlay(
            RoundedRectangle(cornerRadius: 12)
                .stroke(Color("BorderSubtle"), lineWidth: 1)
        )
        .accessibilityElement(children: .contain)
        .quickLookPreview($previewURL)
    }

    /// Opens by id only when this phone has Files for a computer to fetch it from.
    private func canOpen(_ file: AgentUserMessage.File) -> Bool {
        file.id != nil && files.isEnabled
    }

    private func detail(_ file: AgentUserMessage.File) -> String {
        var parts = [FilesFormat.bytes(file.size)]
        if !canOpen(file) { parts.append("Saved on your computer") }
        return parts.joined(separator: " · ")
    }

    @ViewBuilder
    private func fileRow(_ file: AgentUserMessage.File, index: Int) -> some View {
        let row = HStack(spacing: 10) {
            Image(systemName: "doc.text")
                .font(.body)
                .foregroundColor(Color("AccentPrimary"))
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                Text(file.name)
                    .font(.subheadline.weight(.semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(2)
                    .truncationMode(.middle)
                Text(detail(file))
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
                if let caption = file.caption {
                    Text(caption)
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
            }
            Spacer(minLength: 0)
            if canOpen(file) {
                if openingIndex == index {
                    ProgressView().controlSize(.small)
                } else {
                    Image(systemName: "chevron.right")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                        .accessibilityHidden(true)
                }
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("FillNeutral")))

        let label = [file.name, detail(file), file.caption].compactMap { $0 }.joined(separator: ", ")
        if let id = file.id, canOpen(file) {
            Button {
                Task { await open(file, id: id, index: index) }
            } label: { row }
            .buttonStyle(.plain)
            .disabled(openingIndex != nil)
            .accessibilityLabel(label)
            .accessibilityHint("Opens a preview")
        } else {
            row
                .accessibilityElement(children: .ignore)
                .accessibilityLabel(label)
        }
    }

    /// Finds the item on this phone's Files computers (the id says nothing
    /// about which one) and downloads it for Quick Look.
    @MainActor
    private func open(_ file: AgentUserMessage.File, id: String, index: Int) async {
        openingIndex = index
        problem = nil
        defer { openingIndex = nil }
        if let url = await FilesDriveQuickLook.localCopy(itemId: id) {
            previewURL = url
            return
        }
        problem = "Couldn't open \(file.name) right now. It's in Agent Output in Files on your computer."
    }
}

/// Downloads a Files drive item for Quick Look: finds it on this phone's
/// Files computers (the id says nothing about which one). Agent file rows
/// and `file:drive:` citation chips (FileCitationChip) both open through it.
enum FilesDriveQuickLook {
    @MainActor
    static func localCopy(itemId id: String) async -> URL? {
        let files = FilesDrive.shared
        guard files.isEnabled else { return nil }
        for computer in files.computers {
            let model = FilesComputerModel(computerId: computer.id, name: computer.name)
            guard let item = try? await model.client.stat(id).item else { continue }
            if let url = try? await model.localCopy(of: item) { return url }
        }
        return nil
    }
}

/// A scheduled task from the `automation` tool: title, kind (Reminder or
/// Check), the next run in the task's own zone (the user's), status, and
/// Cancel. Cancel deletes the task where it lives: the allternit-api routine
/// (AutomationClient, the same delete the Automation Tasks rows use) or, when
/// it fell back to this computer's scheduler (`via: "cron"`), the cron job.
struct AgentAutomationCard: View {
    let task: AgentAutomationResult.Info
    let via: String?

    @State private var cancelled = false
    @State private var isCancelling = false
    @State private var confirmCancel = false
    @State private var problem: String?

    private var statusLabel: String {
        if cancelled { return "Cancelled" }
        switch task.status {
        case "active": return "Scheduled"
        case "paused": return "Paused"
        case "completed": return "Done"
        default: return task.status.capitalized
        }
    }

    private var canCancel: Bool { !cancelled && task.status != "completed" }

    /// "Runs once" / "Every weekday at 9:00 AM".
    private var scheduleLine: String? {
        guard let schedule = task.schedule else { return nil }
        if schedule.type == "cron", let expression = schedule.expression {
            return CronWords.describe(type: "cron", expression: expression)
        }
        return nil
    }

    private var nextLine: String? {
        if cancelled { return nil }
        guard let next = Self.inZone(task.nextRunAt, zone: task.timezone) else {
            return task.status == "completed" ? "Nothing more scheduled" : nil
        }
        return "Next: \(next)"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                Image(systemName: task.kind == "heartbeat" ? "waveform.path.ecg" : "alarm")
                    .foregroundColor(Color("AccentPrimary"))
                    .accessibilityHidden(true)
                Text(task.kindLabel)
                    .foregroundColor(Color("TextSecondary"))
                Spacer(minLength: 0)
                Text(statusLabel)
                    .foregroundColor(cancelled ? Theme.statusWarning : Color("TextSecondary"))
                    .padding(.horizontal, 8)
                    .padding(.vertical, 2)
                    .background(Capsule().fill(Color("FillNeutral")))
            }
            .font(.caption.weight(.semibold))

            Text(task.title)
                .font(.subheadline.weight(.semibold))
                .foregroundColor(Color("TextPrimary"))
                .frame(maxWidth: .infinity, alignment: .leading)
                .strikethrough(cancelled)

            VStack(alignment: .leading, spacing: 2) {
                if let nextLine { Text(nextLine) }
                if let scheduleLine { Text(scheduleLine) }
                if let target = task.targetLabel { Text(target) }
            }
            .font(.caption)
            .foregroundColor(Color("TextSecondary"))

            if let problem {
                Text(problem)
                    .font(.caption)
                    .foregroundColor(Theme.statusError)
            }

            if canCancel {
                Button(role: .destructive) { confirmCancel = true } label: {
                    if isCancelling {
                        ProgressView().controlSize(.small)
                    } else {
                        Label("Cancel", systemImage: "xmark").font(.caption.weight(.semibold))
                    }
                }
                .buttonStyle(.bordered)
                .disabled(isCancelling)
                .frame(minHeight: 32)
                .accessibilityLabel("Cancel \(task.title)")
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("BgPrimary"))
        .cornerRadius(12)
        .overlay(
            RoundedRectangle(cornerRadius: 12)
                .stroke(Color("BorderSubtle"), lineWidth: 1)
        )
        .accessibilityElement(children: .contain)
        .confirmationDialog("Cancel \(task.title)?", isPresented: $confirmCancel, titleVisibility: .visible) {
            Button("Cancel scheduled task", role: .destructive) { Task { await cancel() } }
            Button("Keep it", role: .cancel) {}
        } message: {
            Text("It won't run again.")
        }
    }

    @MainActor
    private func cancel() async {
        isCancelling = true
        problem = nil
        defer { isCancelling = false }
        do {
            if via == "cron" {
                try await CronJobStore.shared.deleteJob(id: task.id)
            } else {
                try await AutomationClient.shared.deleteRoutine(id: task.id)
            }
            cancelled = true
        } catch {
            problem = "Couldn't cancel it. \(FriendlyErrorMessage.from(error.localizedDescription))"
        }
    }

    /// "Wed, Nov 18, 9:00 AM CST": the instant in the task's zone.
    static func inZone(_ raw: String?, zone: String?) -> String? {
        guard let raw, let date = DevicePairingView.parseDate(raw) else { return nil }
        let formatter = DateFormatter()
        formatter.locale = .current
        formatter.timeZone = zone.flatMap { TimeZone(identifier: $0) } ?? .current
        formatter.setLocalizedDateFormatFromTemplate("EEE MMM d jmm zzz")
        return formatter.string(from: date)
    }
}

/// An assistant reply with its ```openui blocks drawn as cards (a `writing`
/// draft, or text for a template this app has no view for). A reply with no
/// block goes straight to the streaming markdown reader.
private struct AssistantContent: View {
    let content: String
    let isStreaming: Bool

    var body: some View {
        if AnswerTextSplitter.containsCards(content) {
            let segments = AnswerTextSplitter.split(content, isStreaming: isStreaming)
            VStack(alignment: .leading, spacing: 10) {
                ForEach(Array(segments.enumerated()), id: \.offset) { index, segment in
                    switch segment {
                    case .markdown(let text):
                        AssistantMarkdown(content: text, isStreaming: isStreaming && index == segments.count - 1)
                    case .card(let card):
                        AnswerCardView(card: card)
                    case .pendingCard:
                        HStack(spacing: 8) {
                            ProgressView().controlSize(.small)
                            Text("Writing a draft…")
                                .font(.footnote)
                                .foregroundColor(Color("TextSecondary"))
                        }
                        .accessibilityElement(children: .combine)
                    }
                }
            }
        } else {
            AssistantMarkdown(content: content, isStreaming: isStreaming)
        }
    }
}

/// An assistant reply. While it streams, MarkdownView 3's incremental
/// StreamingMarkdownReader parses only the new text (in the background) instead
/// of re-parsing the whole reply on every chunk; a finished reply renders once.
private struct AssistantMarkdown: View {
    let content: String
    let isStreaming: Bool

    @State private var source: StreamingMarkdownSource? = nil
    /// Settings → General → Chat font.
    @ObservedObject private var appearance = AppearanceStore.shared

    /// The reply with `[[cite:…]]` tokens as chip links (never the raw token).
    private var rendered: String { FileCitation.markdown(content, isStreaming: isStreaming) }

    var body: some View {
        Group {
            if let source {
                StreamingMarkdownReader(source) { MarkdownView($0) }
            } else {
                MarkdownView(rendered)
            }
        }
        .markdownElementRenderer(.link(FileCitationLinkRenderer(), urlScheme: FileCitation.scheme))
        .fileCitationHost()
        .modifier(ChatFontModifier(font: appearance.chatFont))
        .onAppear {
            if isStreaming, source == nil { source = StreamingMarkdownSource(rendered) }
        }
        .onChange(of: content) { _, raw in
            let text = FileCitation.markdown(raw, isStreaming: isStreaming)
            if isStreaming, source == nil { source = StreamingMarkdownSource(text) }
            source?.text = text
        }
        .onChange(of: isStreaming) { _, streaming in
            guard !streaming, let source else { return }
            // A held-back token tail is complete (or plain text) now.
            let text = FileCitation.markdown(content, isStreaming: false)
            if source.text != text { source.text = text }
            source.finishStreaming()
        }
    }
}

/// Settings → General → Chat font on a reply: Serif swaps the prose
/// (body, quotes, headings) to the system serif; code keeps its mono font.
private struct ChatFontModifier: ViewModifier {
    let font: AppearanceStore.ChatFont

    func body(content: Content) -> some View {
        if font == .serif {
            content
                .font(Self.serif(.body), for: .body)
                .font(Self.serif(.body), for: .blockQuote)
                .font(Self.serif(.title1), for: .h1)
                .font(Self.serif(.title2), for: .h2)
                .font(Self.serif(.title3), for: .h3)
                .font(Self.serif(.headline), for: .h4)
        } else {
            content
        }
    }

    private static func serif(_ style: UIFont.TextStyle) -> UIFont {
        let base = UIFont.preferredFont(forTextStyle: style)
        guard let descriptor = base.fontDescriptor.withDesign(.serif) else { return base }
        return UIFont(descriptor: descriptor, size: base.pointSize)
    }
}


/// The "agent is doing X" mark in a transcript (allternit-ai
/// src/components/brand/WorkingMark.tsx): the A:// orb, or, in a bot's chat,
/// that bot's avatar playing the same state. Used on the live status line,
/// running tool rows and under a reply while it streams.
struct WorkingMark: View {
    let state: AllternitOrbState
    var size: CGFloat = 16
    @Environment(\.chatBot) private var bot

    var body: some View {
        if let bot, !bot.isPrimaryTemplateBot {
            BotAvatarView(bot: bot, size: size, live: state != .idle, working: state != .idle)
                .accessibilityHidden(true)
        } else {
            AllternitOrbView(state: state, size: size)
                .accessibilityHidden(true)
        }
    }

    /// web orbStateForTool: search-like tools search, writes write, the rest work.
    static func state(forTool toolName: String?) -> AllternitOrbState {
        let name = (toolName ?? "").lowercased().replacingOccurrences(of: #"[_\-\s]"#, with: "", options: .regularExpression)
        if name.range(of: "search|grep|glob|find|fetch|browse|lookup", options: .regularExpression) != nil { return .searching }
        if name.range(of: "write|edit|create|replace|notebook", options: .regularExpression) != nil { return .writing }
        return .working
    }
}


// MARK: - Earlier versions of an edited message

/// Read-only list of what an edit replaced: the earlier user text and the
/// reply it got, newest version first.
struct EditedVersionsSheet: View {
    let branches: [MessageBranch]
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 20) {
                    ForEach(Array(branches.enumerated()), id: \.element.id) { offset, branch in
                        VStack(alignment: .leading, spacing: 8) {
                            Text(branches.count - offset == 1 ? "Original" : "Version \(branches.count - offset)")
                                .font(.system(size: 12, weight: .semibold))
                                .foregroundColor(Color("TextSecondary"))
                            ForEach(branch.messages) { message in
                                VStack(alignment: .leading, spacing: 4) {
                                    Text(message.role == "user" ? "You" : "Reply")
                                        .font(.system(size: 11, weight: .medium))
                                        .foregroundColor(Color("TextSecondary"))
                                    Text(message.role == "user" ? ToolOptionsStore.shared.strippedText(message.content) : message.content)
                                        .font(.system(size: 15))
                                        .foregroundColor(Color("TextPrimary"))
                                        .textSelection(.enabled)
                                }
                                .padding(12)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("FillNeutral")))
                            }
                        }
                    }
                }
                .padding(20)
            }
            .background(Color("BgPrimary"))
            .navigationTitle("Earlier versions")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) { Button("Done") { dismiss() } }
            }
        }
        .presentationDetents([.medium, .large])
    }
}


// MARK: - agent_batch / sleep (E2-4)

/// Counts parsed from the agent_batch result text (the tool part's metadata
/// is not carried on the record): "N ok, M failed, of T rows." / "Sample: N of T rows ok."
struct BatchProgress: Equatable {
    var isSample: Bool
    var done = 0
    var failed = 0
    var skipped = 0
    var total = 0
    var outputPath: String?

    var finished: Int { done + failed + skipped }

    init(call: ToolCallRecord) {
        isSample = call.input["mode"] == "sample"
        let text = call.output ?? ""
        func num(_ pattern: String, in s: String) -> Int? {
            guard let r = s.range(of: pattern, options: .regularExpression) else { return nil }
            return Int(s[r].filter(\.isNumber))
        }
        if text.hasPrefix("Sample:"), let m = text.range(of: #"Sample: (\d+) of (\d+)"#, options: .regularExpression) {
            isSample = true
            let nums = text[m].split(whereSeparator: { !$0.isNumber }).compactMap { Int($0) }
            if nums.count == 2 { done = nums[0]; total = nums[1]; failed = total - done }
        } else {
            done = num(#"\d+(?= ok)"#, in: text) ?? 0
            failed = num(#"\d+(?= failed)"#, in: text) ?? 0
            skipped = num(#"\d+(?= skipped)"#, in: text) ?? 0
            total = num(#"(?<=of )\d+(?= rows)"#, in: text) ?? 0
        }
        if total == 0, let t = call.title, let m = t.range(of: #"(\d+)/(\d+)"#, options: .regularExpression) {
            let nums = t[m].split(separator: "/").compactMap { Int($0) }
            if nums.count == 2 { done = nums[0]; total = nums[1] }
        }
        if let line = text.components(separatedBy: "\n").first(where: { $0.hasPrefix("Results: ") }) {
            outputPath = String(line.dropFirst(9)).trimmingCharacters(in: .whitespaces)
        }
    }
}

struct BatchProgressCard: View {
    let call: ToolCallRecord

    var body: some View {
        let p = BatchProgress(call: call)
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                Image(systemName: "tablecells")
                Text(p.isSample ? "Sample batch" : "Batch run").font(.subheadline.weight(.semibold))
                Spacer()
                if call.state == .running { ProgressView().controlSize(.small) }
            }
            ProgressView(value: Double(min(p.finished, max(p.total, 1))), total: Double(max(p.total, 1)))
            Text("\(p.done) done, \(p.failed) failed\(p.skipped > 0 ? ", \(p.skipped) skipped" : ""), \(p.total) total")
                .font(.caption).foregroundStyle(.secondary)
            if let path = p.outputPath, !p.isSample {
                Label((path as NSString).lastPathComponent, systemImage: "doc.text")
                    .font(.caption).lineLimit(1)
                    .accessibilityHint(path)
            }
        }
        .padding(12)
        .background(RoundedRectangle(cornerRadius: 12).fill(Color(.secondarySystemBackground)))
        .accessibilityElement(children: .combine)
    }
}

struct SleepRow: View {
    let call: ToolCallRecord

    var body: some View {
        let reason = call.input["reason"]?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let woke = call.output ?? ""
        let seconds = Int(call.input["seconds"] ?? "") ?? Int((call.output ?? "").components(separatedBy: " ").dropFirst().first ?? "") ?? 0
        let early = woke.contains("user_input")
        let span = seconds >= 60 ? "\(seconds / 60) min" : "\(seconds) s"
        let base = call.state == .running ? "Waiting \(span)" : (early ? "Woke early" : "Waited \(span)")
        HStack(spacing: 6) {
            Image(systemName: "hourglass")
            Text(reason.isEmpty ? base : "\(base) \u{2014} \(reason)")
        }
        .font(.footnote).foregroundStyle(.secondary)
    }
}


/// A reply's MCP tasks (orb-and-beam cards) and any server question that has
/// no task card to sit in. Behind the `mcp.tasks` flag.
private struct McpTasksSection: View {
    let taskIds: [String]
    let formIds: [String]
    @ObservedObject private var store = McpTasksStore.shared

    var body: some View {
        ForEach(taskIds, id: \.self) { McpTaskCardView(taskId: $0) }
        ForEach(looseForms) { McpElicitationFormView(request: $0) }
    }

    private var looseForms: [McpElicitationPart] {
        formIds.compactMap { store.elicitations[$0] }.filter { form in
            guard let taskId = form.taskId else { return true }
            return store.tasks[taskId] == nil
        }
    }
}
