import SwiftUI

/// A bot's thread over the Workspace floor (product shape B, phone): a bottom
/// sheet (about 45% or large) under the floor's scene strip. Nothing is
/// rebuilt here: the Thread tab is the shared chat stack BotChatSessionView
/// uses (`ChatContentView` + `ChatViewModel`, with its tool steps, inline
/// approval cards and composer), the Screen tab is the bot's computer page
/// (`BotComputerView`: View desktop, Take over / Hand back), and the docked
/// card is `OfficeApprovalCard` over the app's existing approval calls.
struct OfficeThreadSheet: View {
    let bot: AgentRecord
    /// The main bot's name, for autopilot lines ("Answered by A:// on autopilot").
    let mainBotName: String
    /// The sheet's height in points, so the floor can keep the bot above it.
    var onHeight: (CGFloat) -> Void = { _ in }
    /// An approval or answer reached the bot: the floor shows this as a done bubble.
    var onDone: (String) -> Void = { _ in }

    enum Tab: Hashable { case thread, screen }

    @StateObject private var viewModel = ChatViewModel()
    @ObservedObject private var store = BotModeStore.shared
    @ObservedObject private var status = BotStatusStore.shared
    @ObservedObject private var asks = AgentAskStore.shared
    @ObservedObject private var colors = BotColorStore.shared
    @EnvironmentObject private var navigator: BotNavigator
    @Environment(\.dismiss) private var dismiss
    @ReducedMotion private var reduceMotion

    @State private var tab: Tab = .thread
    @State private var sessionId: String?
    @State private var mainSessionId: String?
    @State private var resolveError: String?
    /// Needs put off with Not now while this sheet is open.
    @State private var skipped: Set<String> = []
    /// The latest autopilot answer or draft for this bot, as one line.
    @State private var autopilot: String?

    private var need: OfficeNeed? { OfficeNeeds.first(for: bot.id, skipping: skipped) }

    private var thread: BotThread? {
        guard let sessionId else { return nil }
        return store.threads.first { $0.currentSessionId == sessionId }
    }

    private var threadName: String {
        if sessionId == nil || sessionId == mainSessionId || thread?.isMainChat == true { return "Main chat" }
        return thread?.displayTitle ?? "Thread"
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            if let autopilot {
                Label(autopilot, systemImage: "sparkles")
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(2)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 16)
                    .padding(.bottom, 6)
                    .accessibilityLabel(autopilot)
            }
            if let need {
                OfficeApprovalCard(
                    bot: bot,
                    need: need,
                    onDone: { text in
                        onDone(text)
                        Task { await loadAutopilot() }
                    },
                    onSkip: { skipped.insert(need.id) },
                    onAnswerInThread: { answer(in: $0, needId: need.id) }
                )
                .id(need.id)
                .padding(.horizontal, 12)
                .padding(.bottom, 8)
                .transition(reduceMotion ? .opacity : .move(edge: .top).combined(with: .opacity))
            }
            Divider()
            switch tab {
            case .thread: chat
            case .screen: screen
            }
        }
        .animation(reduceMotion ? nil : .easeOut(duration: 0.2), value: need?.id)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .background(Color("BgPanel").ignoresSafeArea())
        .environment(\.botTint, colors.hex(for: bot))
        .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { onHeight($0) }
        .onChange(of: sessionId, initial: true) { _, _ in viewModel.threadProjectId = thread?.projectId }
        .task(id: bot.id) {
            status.subscribe(botId: bot.id)
            await resolve()
            await OfficeNeeds.refresh(botId: bot.id)
        }
        .task(id: bot.id) {
            while !Task.isCancelled {
                await loadAutopilot()
                try? await Task.sleep(for: .seconds(30))
            }
        }
        .onDisappear { status.unsubscribe(botId: bot.id) }
    }

    // MARK: - Header

    private var header: some View {
        VStack(spacing: 10) {
            HStack(spacing: 10) {
                BotAvatarView(bot: bot, size: 32, working: status.isWorking(for: bot.id))
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 1) {
                    BotNameText(bot.botDisplayName, size: 16)
                        .font(.headline)
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Text(subtitle)
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                }
                .accessibilityElement(children: .combine)
                Spacer(minLength: 0)
                if tab == .thread, sessionId != nil, sessionId != mainSessionId, mainSessionId != nil {
                    Button("Main chat") { sessionId = mainSessionId }
                        .font(.footnote.weight(.semibold))
                        .accessibilityHint("Shows \(bot.botDisplayName)'s main chat")
                }
                Button { dismiss() } label: {
                    Image(systemName: "xmark")
                        .font(.system(size: 14, weight: .bold))
                        .foregroundColor(Color("TextSecondary"))
                        .frame(width: 32, height: 32)
                        .background(Circle().fill(Color("FillNeutral")))
                        .frame(width: 44, height: 44)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Close")
                .accessibilityHint("Closes \(bot.botDisplayName)'s thread")
            }
            HStack(spacing: 10) {
                Picker("View", selection: $tab) {
                    Text("Thread").tag(Tab.thread)
                    Text("Screen").tag(Tab.screen)
                }
                .pickerStyle(.segmented)
                .accessibilityLabel("Thread or screen")
                // The existing full bot chat (BotChatSessionView), on the chat this sheet shows.
                Button("Open the full chat") {
                    let route = BotRoute.chat(botId: bot.id, sessionId: sessionId)
                    dismiss()
                    navigator.open(route)
                }
                .font(.footnote.weight(.semibold))
                .lineLimit(1)
                .fixedSize()
                .frame(minHeight: 44)
                .accessibilityHint("Leaves the floor for \(bot.botDisplayName)'s chat")
            }
        }
        .padding(.horizontal, 16)
        .padding(.top, 14)
        .padding(.bottom, 8)
    }

    private var subtitle: String {
        if viewModel.isStreaming || status.isWorking(for: bot.id) { return "\(threadName) · Working" }
        if let line = thread?.statusLine, !line.isEmpty { return "\(threadName) · \(line)" }
        return threadName
    }

    // MARK: - Thread

    @ViewBuilder
    private var chat: some View {
        if let sessionId {
            ChatContentView(sessionId: sessionId, viewModel: viewModel,
                            composerPlaceholder: "Message \(bot.botDisplayName)…")
                .environment(\.chatBot, bot)
        } else if let resolveError {
            FriendlyStateView(
                style: .error,
                icon: "exclamationmark.bubble",
                title: "Couldn't open \(bot.botDisplayName)",
                message: resolveError,
                actionTitle: "Try again",
                action: { Task { await resolve() } }
            )
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            VStack(spacing: 10) {
                ProgressView()
                Text("Opening \(bot.botDisplayName)…")
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    // MARK: - Screen

    /// The bot's computer page in its own stack, so its links open inside the sheet.
    private var screen: some View {
        NavigationStack {
            BotComputerView(bot: bot)
                .navigationBarTitleDisplayMode(.inline)
                .navigationDestination(for: BotRoute.self) { route in
                    switch route {
                    case .computer(let computer): ComputerDriveView(computer: computer)
                    case .computers: ComputersListView()
                    default: EmptyView()
                    }
                }
        }
    }

    // MARK: - Actions

    /// Opens on the need's own chat when it has one, else the bot's main chat.
    private func resolve() async {
        resolveError = nil
        if sessionId == nil, let needSession = need?.sessionId { sessionId = needSession }
        do {
            let main = try await store.mainChatSessionId(for: bot)
            mainSessionId = main
            if sessionId == nil { sessionId = main }
        } catch is CancellationError {
        } catch {
            if sessionId == nil { resolveError = FriendlyErrorMessage.from(error.localizedDescription) }
        }
    }

    /// Answer on a waiting thread: its chat in this sheet, or its waiting page when it has no chat yet.
    private func answer(in thread: BotThread, needId: String) {
        guard let id = thread.currentSessionId else {
            dismiss()
            navigator.openThread(thread)
            return
        }
        skipped.insert(needId)
        tab = .thread
        sessionId = id
    }

    /// The newest `autopilot_answer` / `autopilot_draft` in the bot's ledger (oldest first, so page forward).
    private func loadAutopilot() async {
        var after: Int?
        var latest: BotEvent?
        for _ in 0..<5 {
            guard let page = try? await BotEventsClient.shared.fetchEvents(
                botId: bot.id, afterSequence: after, limit: 200,
                eventTypes: ["autopilot_answer", "autopilot_draft"]) else { break }
            if let last = page.events.last {
                latest = last
                after = last.sequence
            }
            if !page.hasMore || page.events.isEmpty { break }
        }
        guard !Task.isCancelled else { return }
        if let latest { autopilot = Self.autopilotLine(latest, mainBot: mainBotName) }
    }

    /// "Answered by <main bot> on autopilot" / "Drafted by <main bot>", with the text when there is one.
    static func autopilotLine(_ event: BotEvent, mainBot: String) -> String {
        let lead = event.eventType == "autopilot_draft"
            ? "Drafted by \(mainBot)"
            : "Answered by \(mainBot) on autopilot"
        var text = ""
        if case .string(let value)? = event.payload["text"] { text = value.trimmingCharacters(in: .whitespacesAndNewlines) }
        return text.isEmpty ? lead : "\(lead): \(text.prefix(100))"
    }
}
