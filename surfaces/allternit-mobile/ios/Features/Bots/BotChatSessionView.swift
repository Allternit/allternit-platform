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
/// opens the bot page) and ⋯ with the rest. Under it, the thread's deck
/// (BotThreadTopDeck): the open thread · Threads · Tasks · Bot.
struct BotChatSessionView: View {
    let bot: AgentRecord
    let explicitSessionId: String?

    enum DeckTab: Hashable { case thread, threads, tasks, bot }

    @StateObject private var viewModel = ChatViewModel()
    @ObservedObject private var store = BotModeStore.shared
    @ObservedObject private var status = BotStatusStore.shared
    @ObservedObject private var desktop = BotDesktopStore.shared
    @ObservedObject private var colors = BotColorStore.shared
    @EnvironmentObject private var navigator: BotNavigator
    @State private var sessionId: String? = nil
    @State private var resolveError: String? = nil
    @State private var deckTab: DeckTab = .thread
    @State private var actionError: String? = nil
    @State private var isActing = false
    @Environment(\.dismiss) private var dismiss

    private var isMainChat: Bool { explicitSessionId == nil }

    private var thread: BotThread? {
        guard let sessionId else { return nil }
        return store.threads.first { $0.currentSessionId == sessionId }
    }

    var body: some View {
        VStack(spacing: 0) {
            deck
            content
        }
        // The bot's colour: a wash at the top of its chat (and its bar), your
        // bubbles, and the drawer while this chat is open.
        .background { BotWashedBackground().ignoresSafeArea() }
        .environment(\.botTint, colors.hex(for: bot))
        .claimsBotTint(bot.id)
        // The phone thread header (ThreadHeader, phone): round Back · bot
        // chip · ⋯, drawn by the view instead of the system bar.
        .toolbar(.hidden, for: .navigationBar)
        .safeAreaInset(edge: .top, spacing: 0) { threadTop }
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
        .alert("Something went wrong", isPresented: Binding(
            get: { actionError != nil }, set: { if !$0 { actionError = nil } }
        )) {
            Button("OK", role: .cancel) { actionError = nil }
        } message: { Text(actionError ?? "") }
    }

    // MARK: - Content

    @ViewBuilder
    private var content: some View {
        switch deckTab {
        case .thread:
            chat
        case .threads:
            ScrollView {
                BotThreadsPanel(botId: bot.id, greeting: nil) { open($0) }
                    .padding(.vertical, 4)
            }
            .refreshable { await store.refreshThreads() }
        case .tasks:
            BotTasksView(bot: bot)
        case .bot:
            BotPageView(bot: bot,
                        onOpenThreads: { deckTab = .threads },
                        onOpenTasks: { deckTab = .tasks })
        }
    }

    @ViewBuilder
    private var chat: some View {
        if let sessionId {
            ChatContentView(sessionId: sessionId, viewModel: viewModel)
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

    // MARK: - Deck

    private var deckThreadName: String {
        if isMainChat { return "Main chat" }
        if let title = thread?.title, !title.isEmpty { return title }
        return "Thread"
    }

    /// The thread's tabs (BotThreadTopDeck, phone: labels only).
    private var deck: some View {
        let waiting = store.waitingCount(for: bot.id)
        return ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 2) {
                deckButton(.thread, label: deckThreadName, truncates: true)
                deckButton(.threads, label: "Threads", dot: waiting > 0 ? waiting : nil)
                deckButton(.tasks, label: "Tasks", meta: thread?.progressLabel)
                deckButton(.bot, label: "Bot")
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 6)
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
            .background(RoundedRectangle(cornerRadius: 8).fill(on ? Color("BorderStrong") : .clear))
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
        return Button {
            deckTab = .bot
        } label: {
            HStack(spacing: 8) {
                BotAvatarView(bot: bot, size: 28)
                    .overlay(alignment: .bottomTrailing) {
                        Circle()
                            .fill(live.waiting ? Theme.statusWarning : live.live ? Theme.statusInfo : Color("TextTertiary"))
                            .frame(width: 8, height: 8)
                            .overlay(Circle().stroke(Color("BgPanel"), lineWidth: 2))
                            .offset(x: 2, y: 2)
                    }
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
            }
            .padding(.leading, 6)
            .padding(.trailing, 14)
            .padding(.vertical, 4)
            .frame(minHeight: 40)
            .background(Capsule().fill(Color("BgPanel").opacity(0.88)))
            .overlay(Capsule().stroke(Color("BorderSubtle")))
            .shadow(color: .black.opacity(0.06), radius: 4, y: 1)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Open \(bot.botDisplayName), \(live.text)")
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
            Button { navigator.open(.stage(bot.id)) } label: {
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
