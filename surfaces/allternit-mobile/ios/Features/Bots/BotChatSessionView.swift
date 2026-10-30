import SwiftUI
import UIKit

/// A bot chat (allternit-ai src/views/bots/BotChatSessionView.tsx), hosted
/// on the shared chat stack: `ChatContentView` + `ChatViewModel` (agent
/// session SSE via AgentChatClient). With no explicit session it opens the
/// bot's main chat, recovering it before ever creating one
/// (BotModeStore.mainChatSessionId).
///
/// The phone thread header (ThreadHeader.tsx, phone): Back, the bot chip in
/// the middle (avatar with its status dot, name, what it is doing; a tap
/// opens the bot page) and ⋯ with the rest. The chat opens on the bot's live
/// stage (BotChatStage); a swipe up or a send flies it into the chip's
/// avatar. The composer is the thread composer: the thread's tabs in the
/// tray above the card (BotThreadTopDeck: the open thread · Threads · Tasks
/// · Bot) and the compact deck (Agent mode · Plugins · Artifacts) under it.
struct BotChatSessionView: View {
    let bot: AgentRecord
    let explicitSessionId: String?

    enum DeckTab: Hashable { case thread, threads, tasks, bot }

    @StateObject private var viewModel = ChatViewModel()
    @ObservedObject private var store = BotModeStore.shared
    @ObservedObject private var status = BotStatusStore.shared
    @ObservedObject private var desktop = BotDesktopStore.shared
    @ObservedObject private var colors = BotColorStore.shared
    @ObservedObject private var backdrops = StageBackdropStore.shared
    @EnvironmentObject private var navigator: BotNavigator
    @State private var isBackgroundPresented = false
    @State private var sessionId: String? = nil
    @State private var resolveError: String? = nil
    @State private var deckTab: DeckTab = .thread
    @State private var actionError: String? = nil
    @State private var isActing = false
    /// The bot has flown off its stage into the thread top.
    @State private var docked = false
    @Environment(\.dismiss) private var dismiss
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private var isMainChat: Bool { explicitSessionId == nil }

    private var thread: BotThread? {
        guard let sessionId else { return nil }
        return store.threads.first { $0.currentSessionId == sessionId }
    }

    /// The bot stands on its stage (the thread tab, not yet docked).
    private var onStage: Bool { deckTab == .thread && !docked && sessionId != nil }

    private var placeholder: String {
        switch deckTab {
        case .threads: return "Start a new thread…"
        case .tasks: return "Ask for a task in this thread…"
        default: return thread != nil && !isMainChat ? "Steer this thread…" : "Message \(bot.botDisplayName)…"
        }
    }

    /// This bot's background: its own look, else Settings (Plain when off).
    private var backdropVariant: String { backdrops.variant(for: .bot, scope: .bot(bot.id)) }

    var body: some View {
        content
            // The bot's colour: a wash at the top of its chat (and its bar), your
            // bubbles, and the drawer while this chat is open. With a stage
            // background the screens above draw only the wash over it.
            .background {
                if backdropVariant == "plain" {
                    BotWashedBackground().ignoresSafeArea()
                } else {
                    StageBackdropView(variant: backdropVariant, colors: [colors.hex(for: bot)]).ignoresSafeArea()
                }
            }
            .environment(\.botTint, colors.hex(for: bot))
            .claimsBotTint(bot.id)
            // The phone thread header (ThreadHeader, phone): round Back · bot
            // chip · ⋯, drawn by the view instead of the system bar.
            .toolbar(.hidden, for: .navigationBar)
            .safeAreaInset(edge: .top, spacing: 0) { threadTop }
            .backdropTouches(backdropVariant != "plain")
            .environment(\.backdropUnderneath, backdropVariant != "plain")
            .publishesRailBackdrop(backdropVariant == "plain" ? nil : RailBackdropStore.Look(
                colors: [colors.hex(for: bot)].compactMap { $0 }, variant: backdropVariant, mode: .bots))
            .sheet(isPresented: $isBackgroundPresented) {
                StageBackdropScopeSheet(scope: .bot(bot.id), title: "\(bot.botDisplayName)'s background",
                                        colors: [colors.hex(for: bot)])
            }
            .background(KeepsSwipeBack())
            .task {
                status.subscribe(botId: bot.id)
                desktop.fetchStatusIfNeeded(botId: bot.id)
                await resolve()
            }
            .task { await InappCallStore.shared.loadCallableContacts(botId: bot.id) }
            .onDisappear { status.unsubscribe(botId: bot.id) }
            .onChange(of: viewModel.transientError) { _, error in
                // A main-chat pin whose session is gone: drop it so the next
                // open recovers or recreates (web resolveCanonicalChatSession).
                if isMainChat, let error, error.hasPrefix("Couldn't load the conversation") {
                    store.forgetMainChat(for: bot.id)
                }
            }
            // Sending (a prompt, a suggestion, a hand-off) flies the bot up.
            .onChange(of: viewModel.isStreaming) { _, streaming in
                if streaming { dock() }
            }
            .onChange(of: sessionId) { _, _ in docked = false }
            .alert("Something went wrong", isPresented: Binding(
                get: { actionError != nil }, set: { if !$0 { actionError = nil } }
            )) {
                Button("OK", role: .cancel) { actionError = nil }
            } message: { Text(actionError ?? "") }
    }

    /// The flight between the stage and the thread top (FLIGHT: 0.44s).
    private var flight: Animation? {
        reduceMotion ? nil : .timingCurve(0.65, 0, 0.35, 1, duration: 0.44)
    }

    private func dock() {
        guard !docked else { return }
        withAnimation(flight) { docked = true }
    }

    private func undock() {
        withAnimation(flight) {
            deckTab = .thread
            docked = false
        }
    }

    // MARK: - Content

    @ViewBuilder
    private var content: some View {
        switch deckTab {
        case .thread:
            chat
        case .threads:
            VStack(spacing: 0) {
                ScrollView {
                    BotThreadsPanel(botId: bot.id, greeting: nil) { open($0) }
                        .padding(.vertical, 4)
                }
                .refreshable { await store.refreshThreads() }
                BotTabComposer(placeholder: placeholder, topDeck: AnyView(deck), onSend: startThread)
            }
        case .tasks:
            VStack(spacing: 0) {
                BotTasksView(bot: bot)
                BotTabComposer(placeholder: placeholder, topDeck: AnyView(deck)) { text in
                    // A task asked for here goes into this thread's chat.
                    deckTab = .thread
                    viewModel.sendMessage(text)
                }
            }
        case .bot:
            VStack(spacing: 0) {
                BotPageView(bot: bot,
                            onOpenThreads: { deckTab = .threads },
                            onOpenTasks: { deckTab = .tasks })
                // The Bot tab is the bot's own page, not a chat: only the
                // deck stays, to switch back.
                ScrollView(.horizontal, showsIndicators: false) {
                    deck.padding(.horizontal, 8)
                }
                .frame(maxWidth: .infinity, minHeight: 46, alignment: .leading)
                .background(RoundedRectangle(cornerRadius: 16).fill(Color("BgPanel")))
                .overlay(RoundedRectangle(cornerRadius: 16).stroke(Color("BorderSubtle")))
                .padding(.horizontal, 14)
                .padding(.top, 6)
                .padding(.bottom, 8)
            }
        }
    }

    @ViewBuilder
    private var chat: some View {
        if let sessionId {
            VStack(spacing: 0) {
                if onStage {
                    BotChatStage(
                        bot: bot,
                        statusLine: stageStatus,
                        working: status.isWorking(for: bot.id) || viewModel.isStreaming,
                        needs: store.threads(for: bot.id).filter { $0.group == "waiting" && $0.currentSessionId != sessionId },
                        suggestions: store.threads(for: bot.id).filter { $0.status == "proposed" },
                        onOpen: { open($0) },
                        onApprove: { act($0, .approve) },
                        onDecline: { act($0, .decline) },
                        onHandoff: { text, files in viewModel.sendMessage(text, attachments: files) },
                        onDock: dock,
                        sessionId: sessionId,
                        liveSteps: viewModel.isStreaming
                            ? BotStageLiveModel.liveSteps(viewModel.messages.last { $0.role == "assistant" })
                            : [],
                        onStop: viewModel.isStreaming ? { viewModel.stopStreaming() } : nil,
                        // Take over: the bot's computer, as the web opens its computer pane.
                        onTakeOver: { navigator.open(.botComputer(bot.id)) }
                    )
                    .transition(.asymmetric(
                        insertion: .scale(scale: 0.2, anchor: .top).combined(with: .opacity),
                        removal: .scale(scale: 0.15, anchor: .top).combined(with: .opacity)
                    ))
                }
                ChatContentView(sessionId: sessionId, viewModel: viewModel,
                                composerTopDeck: AnyView(deck), composerPlaceholder: placeholder)
                    .environment(\.chatBot, bot)
                    .environment(\.chatBotOnStage, onStage)
            }
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
            VStack(spacing: 12) {
                BotAvatarView(bot: bot, size: 72, live: true)
                ProgressView()
                Text("Opening \(bot.botDisplayName)…")
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    /// Under the bot on its stage: what it is doing, or nothing when idle.
    private var stageStatus: String? {
        let live = liveText
        return live.live ? live.text : nil
    }

    // MARK: - Deck

    private var deckThreadName: String {
        if isMainChat { return "Main chat" }
        if let title = thread?.title, !title.isEmpty { return title }
        return "Thread"
    }

    /// The thread's tabs (BotThreadTopDeck, phone: labels only), in the
    /// tray above the composer card.
    private var deck: some View {
        let waiting = store.waitingCount(for: bot.id)
        return HStack(spacing: 2) {
            deckButton(.thread, label: deckThreadName, truncates: true)
            deckButton(.threads, label: "Threads", dot: waiting > 0 ? waiting : nil)
            deckButton(.tasks, label: "Tasks", meta: thread?.progressLabel)
            deckButton(.bot, label: "Bot")
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Bot thread")
    }

    private func deckButton(_ tab: DeckTab, label: String, truncates: Bool = false,
                            meta: String? = nil, dot: Int? = nil) -> some View {
        let on = deckTab == tab
        return Button { deckTab = tab } label: {
            HStack(spacing: 6) {
                Text(label)
                    .font(.system(size: 12.5, weight: on ? .semibold : .regular))
                    .foregroundColor(on ? Color("TextPrimary") : Color("TextSecondary"))
                    .lineLimit(1)
                    .frame(maxWidth: truncates ? 96 : nil)
                    .fixedSize(horizontal: !truncates, vertical: false)
                if let meta {
                    Text(meta)
                        .font(.system(size: 11).monospacedDigit())
                        .foregroundColor(Color("TextTertiary"))
                }
                if let dot {
                    Circle().fill(Theme.statusWarning).frame(width: 6, height: 6)
                        .accessibilityLabel("\(dot) waiting on you")
                }
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .frame(minHeight: 32)
            .background(RoundedRectangle(cornerRadius: 8).fill(on ? Color("BorderStrong") : .clear))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(on ? [.isSelected] : [])
    }

    // MARK: - Header

    /// What the bot is doing now (live-status.ts), else "Ready".
    private var liveText: (text: String, waiting: Bool, live: Bool) {
        if let thread, thread.group == "waiting" {
            return (thread.statusLine ?? "Needs you", true, true)
        }
        if status.needsAttention(for: bot.id) { return ("Needs you", true, true) }
        if let thread, thread.group == "working" {
            return (thread.statusLine ?? "Working", false, true)
        }
        if status.isWorking(for: bot.id) { return ("Working", false, true) }
        return ("Ready", false, false)
    }

    /// Round Back, the bot chip in the middle (avatar, name, what it is
    /// doing; a tap opens the Bot tab), and ⋯ with everything else.
    private var threadTop: some View {
        HStack(spacing: 8) {
            Button { dismiss() } label: {
                Image(systemName: "chevron.left")
                    .font(.system(size: 16, weight: .bold))
                    .modifier(ThreadRoundButton())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Back")
            Spacer(minLength: 0)
            botChip
            Spacer(minLength: 0)
            HStack(spacing: 6) {
                BotCallButton(botId: bot.id, style: .round)
                threadMenu
            }
            .frame(minWidth: 40, alignment: .trailing)
        }
        .padding(.horizontal, 12)
        .padding(.top, 4)
        .padding(.bottom, 6)
        .background { BotWashedBackground(color: .some(colors.hex(for: bot))).ignoresSafeArea(edges: .top) }
    }

    private var botChip: some View {
        let live = liveText
        return HStack(spacing: 8) {
            // The bot's spot in the thread top: empty while it stands on its
            // stage; once docked, a tap brings it back out.
            Button { undock() } label: {
                BotAvatarView(bot: bot, size: 28)
                    .overlay(alignment: .bottomTrailing) {
                        Circle()
                            .fill(live.waiting ? Theme.statusWarning : live.live ? Theme.statusInfo : Color("TextTertiary"))
                            .frame(width: 8, height: 8)
                            .overlay(Circle().stroke(Color("BgPanel"), lineWidth: 2))
                            .offset(x: 2, y: 2)
                    }
                    .opacity(onStage ? 0 : 1)
                    .scaleEffect(onStage ? 0.4 : 1)
            }
            .buttonStyle(.plain)
            .disabled(onStage)
            .accessibilityLabel("Show \(bot.botDisplayName)")
            .accessibilityHidden(onStage)
            Button { deckTab = .bot } label: {
                VStack(alignment: .leading, spacing: 0) {
                    BotNameText(bot.botDisplayName, size: 14)
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    HStack(spacing: 4) {
                        if live.live {
                            Circle()
                                .fill(live.waiting ? Theme.statusWarning : Theme.statusInfo)
                                .frame(width: 6, height: 6)
                        }
                        Text(live.text).lineLimit(1)
                    }
                    .font(.system(size: 11))
                    .foregroundColor(live.waiting ? Theme.statusWarning : Color("TextTertiary"))
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Open \(bot.botDisplayName), \(live.text)")
        }
        .padding(.leading, 6)
        .padding(.trailing, 14)
        .padding(.vertical, 4)
        .frame(minHeight: 40)
        .background(Capsule().fill(Color("BgPanel").opacity(0.88)))
        .overlay(Capsule().stroke(Color("BorderSubtle")))
        .shadow(color: .black.opacity(0.06), radius: 4, y: 1)
    }

    /// ⋯: everything else — computer, switch thread, the stage, Go ahead /
    /// Not now on a suggestion, Resolve, bot settings.
    private var threadMenu: some View {
        Menu {
            Button { navigator.open(.botComputer(bot.id)) } label: {
                Label(desktop.entry(for: bot.id)?.status?.status == .running ? "Computer · running" : "Computer",
                      systemImage: "desktopcomputer")
            }
            Button { deckTab = .threads } label: {
                Label("Switch thread", systemImage: "list.bullet")
            }
            Button { undock() } label: {
                Label("Show \(bot.botDisplayName)", systemImage: "sparkles")
            }
            if let thread {
                if thread.status == "proposed" {
                    Button { act(thread, .approve) } label: { Label("Go ahead", systemImage: "play") }
                    Button { act(thread, .decline) } label: { Label("Not now", systemImage: "xmark") }
                } else if thread.kind == "task" && thread.group != "resolved" {
                    Button { act(thread, .resolve) } label: { Label("Resolve", systemImage: "checkmark.circle") }
                }
            }
            Button { isBackgroundPresented = true } label: {
                Label("Background", systemImage: "paintbrush")
            }
            Button { navigator.open(.botHome(bot.id, .config)) } label: {
                Label("Bot settings", systemImage: "gearshape")
            }
        } label: {
            Image(systemName: "ellipsis")
                .font(.system(size: 16, weight: .bold))
                .modifier(ThreadRoundButton())
        }
        .disabled(isActing)
        .accessibilityLabel("Thread actions")
    }

    private enum ThreadAction { case approve, decline, resolve }

    private func act(_ thread: BotThread, _ action: ThreadAction) {
        isActing = true
        Task {
            defer { isActing = false }
            do {
                switch action {
                case .approve: try await store.approve(thread)
                case .decline: try await store.decline(thread)
                case .resolve: try await store.resolve(thread)
                }
            } catch {
                switch action {
                case .approve: actionError = "Couldn't start it. Try again."
                case .decline: actionError = "Couldn't close the suggestion. Try again."
                case .resolve: actionError = "Couldn't resolve the thread. Try again."
                }
            }
        }
    }

    // MARK: - Actions

    /// From the Threads tab, what you type starts a new thread.
    private func startThread(_ text: String) async throws {
        let id = try await store.newThreadSession(for: bot, title: BotLaunchpadView.projectTitle(from: text))
        try await BotModeClient.shared.sendUserMessage(sessionId: id, text: text)
        navigator.open(.chat(botId: bot.id, sessionId: id))
        await store.refreshThreads()
    }

    /// A thread picked from the Threads tab opens in its own chat.
    private func open(_ thread: BotThread) {
        guard let id = thread.currentSessionId else { return }
        if id == sessionId {
            deckTab = .thread
        } else {
            navigator.open(.chat(botId: bot.id, sessionId: id))
        }
    }

    private func resolve() async {
        resolveError = nil
        if let explicitSessionId {
            sessionId = explicitSessionId
            return
        }
        do {
            sessionId = try await store.mainChatSessionId(for: bot)
        } catch is CancellationError {
        } catch {
            resolveError = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }
}

/// The phone thread header's round buttons (PHONE_ROUND_BUTTON): 40pt,
/// hairline, the panel at 88%, a soft shadow.
struct ThreadRoundButton: ViewModifier {
    func body(content: Content) -> some View {
        content
            .foregroundColor(Color("TextPrimary"))
            .frame(width: 40, height: 40)
            .background(Circle().fill(Color("BgPanel").opacity(0.88)))
            .overlay(Circle().stroke(Color("BorderSubtle")))
            .shadow(color: .black.opacity(0.06), radius: 4, y: 1)
            .frame(width: 44, height: 44)
            .contentShape(Circle())
    }
}

/// Keeps the edge swipe back working on a pushed screen that hides the
/// system bar (SwiftUI turns it off with the bar). Only pops when there is
/// somewhere to go back to.
struct KeepsSwipeBack: UIViewControllerRepresentable {
    func makeUIViewController(context: Context) -> Controller { Controller() }
    func updateUIViewController(_ controller: Controller, context: Context) {}

    final class Controller: UIViewController, UIGestureRecognizerDelegate {
        override func viewWillAppear(_ animated: Bool) {
            super.viewWillAppear(animated)
            navigationController?.interactivePopGestureRecognizer?.isEnabled = true
            navigationController?.interactivePopGestureRecognizer?.delegate = self
        }

        func gestureRecognizerShouldBegin(_ gestureRecognizer: UIGestureRecognizer) -> Bool {
            (navigationController?.viewControllers.count ?? 0) > 1
        }
    }
}
