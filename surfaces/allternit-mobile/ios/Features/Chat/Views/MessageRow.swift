import SwiftUI
import MarkdownView
import UIKit

struct MessageRow: View {
    let message: MessageRecord
    let onArtifactTap: (ArtifactRecord) -> Void
    /// Error-card actions: model errors offer "Choose a model", everything
    /// else offers "Retry" (re-streams the last sent text).
    var onChooseModel: () -> Void = {}
    var onRetry: (String) -> Void = { _ in }
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

    @State private var isThinkingExpanded = false
    @State private var showsRunTree = false
    @State private var showsEditedVersions = false
    @ObservedObject private var asks = AgentAskStore.shared
    /// Settings → Capabilities → Artifacts gates the inline artifact cards;
    /// observed so toggling it applies to already-rendered rows.
    @ObservedObject private var settings = SettingsStore.shared

    /// In a bot's chat your bubbles wear the bot's colour (30% over the card).
    @Environment(\.botTint) private var botTint

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

                // Message tools: a draft card, or the held-for-approval line.
                ForEach(message.toolCalls) { call in
                    switch MessageToolResult.parse(toolName: call.toolName, output: call.output) {
                    case .draft(let draft): MessageDraftCard(draft: draft)
                    case .held(let inboxId): MessageHeldLine(inboxId: inboxId)
                    case nil: EmptyView()
                    }
                }

                // run_code: produced files (images, CSV grids, downloads) + run summary.
                ForEach(message.toolCalls) { call in
                    if let result = call.runCode, call.toolName == "run_code" {
                        RunCodeFilesView(result: result)
                    }
                }

                // Message Bubble — markdown for the assistant, plain text for the user.
                // Skipped when the message is nothing but an error card.
                if let run = hostedRun, message.role == "assistant" {
                    VStack(alignment: .leading, spacing: 6) {
                        RunStatusPill(card: run)
                        if !message.content.isEmpty || message.error == nil {
                            bubble
                        }
                        RunThoughtStream(steps: run.steps, active: run.status.isActive)
                    }
                } else if !message.content.isEmpty || message.error == nil {
                    bubble
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
                }

                // Structured failure card (replaces raw backend error text)
                if let error = message.error {
                    ChatErrorCardView(
                        error: error,
                        onChooseModel: onChooseModel,
                        onRetry: { onRetry(message.id) }
                    )
                }

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
            }

            .frame(maxWidth: message.role == "user" ? UIScreen.main.bounds.width * 0.8 : .infinity,
                   alignment: message.role == "user" ? .trailing : .leading)
        }
        .padding(.horizontal, 16)
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
        return list.filter { !asks.isResolved($0.id) }
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
                AssistantMarkdown(content: message.content, isStreaming: message.isStreaming)
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
            UIPasteboard.general.string = message.content
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

/// An assistant reply. While it streams, MarkdownView 3's incremental
/// StreamingMarkdownReader parses only the new text (in the background) instead
/// of re-parsing the whole reply on every chunk; a finished reply renders once.
private struct AssistantMarkdown: View {
    let content: String
    let isStreaming: Bool

    @State private var source: StreamingMarkdownSource? = nil
    /// Settings → General → Chat font.
    @ObservedObject private var appearance = AppearanceStore.shared

    var body: some View {
        Group {
            if let source {
                StreamingMarkdownReader(source) { MarkdownView($0) }
            } else {
                MarkdownView(content)
            }
        }
        .modifier(ChatFontModifier(font: appearance.chatFont))
        .onAppear {
            if isStreaming, source == nil { source = StreamingMarkdownSource(content) }
        }
        .onChange(of: content) { _, text in
            if isStreaming, source == nil { source = StreamingMarkdownSource(text) }
            source?.text = text
        }
        .onChange(of: isStreaming) { _, streaming in
            if !streaming { source?.finishStreaming() }
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
