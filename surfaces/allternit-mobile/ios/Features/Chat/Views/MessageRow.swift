import SwiftUI
import MarkdownView

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
    var onEdit: () -> Void = {}
    /// The chat's agent session — the run tree's task list and the pending
    /// questions/permissions lookup. Nil before the first send.
    var sessionId: String? = nil
    /// Sends a prompt to the agent as a new turn (run tree Retry).
    var onSendPrompt: ((String) -> Void)? = nil

    @State private var isThinkingExpanded = false
    @State private var showsRunTree = false
    @ObservedObject private var asks = AgentAskStore.shared
    /// Settings → Capabilities → Artifacts gates the inline artifact cards;
    /// observed so toggling it applies to already-rendered rows.
    @ObservedObject private var settings = SettingsStore.shared

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

                // Message Bubble — markdown for the assistant, plain text for the user.
                // Skipped when the message is nothing but an error card.
                if !message.content.isEmpty || message.error == nil {
                    bubble
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
                        onRegenerate: onRegenerate
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

    @ViewBuilder
    private var bubble: some View {
        if message.role == "user" {
            Text(message.content)
                .font(.system(size: 15))
                .lineSpacing(15 * 0.6 - 4)
                .foregroundColor(Color("TextPrimary"))
                .padding(.horizontal, 16)
                .padding(.vertical, 10)
                .background(Color("FillNeutral"), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                .textSelection(.enabled)
                .contextMenu { bubbleContextMenu }
        } else if message.isStreaming, message.content.isEmpty, message.reasoning.isEmpty,
                  message.toolCalls.isEmpty, message.toolStatus == nil {
            // Waiting on the first words: the thinking orb.
            HStack(spacing: 8) {
                ThinkingOrb(state: .breathing, size: .inline)
                Text("Working…")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
            }
            .accessibilityElement(children: .combine)
        } else {
            AssistantMarkdown(content: message.content, isStreaming: message.isStreaming)
                .font(.system(size: 15))
                .foregroundColor(Color("TextPrimary"))
                .contextMenu { bubbleContextMenu }
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
                    ThinkingOrb(state: .forToolName(status.toolName), size: .inline)
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
