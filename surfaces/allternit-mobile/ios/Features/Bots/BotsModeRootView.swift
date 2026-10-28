import SwiftUI

/// Every screen Bot Mode can push (the PWA's FabricBotView + the bot page
/// sections).
enum BotRoute: Hashable {
    /// A bot chat; nil session = the bot's main chat.
    case chat(botId: String, sessionId: String?)
    case page(String)
    /// All bots' threads, or one bot's.
    case threads(botId: String?)
    case project(BotProjectSummary)
    case groups
    case group(String)
    case computers
    case computer(ComputerRecord)
    case botComputer(String)
    case tasks(String)
    case routines(String)
    case memory(String)
    case channels(String)
    /// The bot's phone: its number, address and channel conversations.
    case phone(String)
    case stage(String)
    /// The existing full Bot Home dashboard (artifacts, automation, files).
    case home(String)
    /// The existing agent editor (Features/Agents/AgentDetailView).
    case settings(String)
}

/// Bot Mode's navigation path, shared with every pushed screen so async
/// work (a created group, a resolved chat) can push the next screen.
@MainActor
final class BotNavigator: ObservableObject {
    @Published var path: [BotRoute] = []

    func open(_ route: BotRoute) {
        path.append(route)
    }
}

/// Bot Mode root — public entry, no required init params (lane L3 mounts
/// it as the Bots mode root). The PWA's FabricBotModeRail on a phone page:
/// quick bot strip, Threads (waiting badge), Projects, Groups, Computers,
/// pinned bots, group chats, then every bot with its status dot. Tapping a
/// bot opens its main chat.
struct BotsModeRootView: View {
    @StateObject private var store = BotModeStore.shared
    @StateObject private var hub = AgentHubStore.shared
    @StateObject private var auth = AuthManager.shared
    @StateObject private var navigator = BotNavigator()

    /// Opens the shell drawer (the edge swipe also works).
    var onOpenSidebar: (() -> Void)?

    init(onOpenSidebar: (() -> Void)? = nil) {
        self.onOpenSidebar = onOpenSidebar
    }

    var body: some View {
        NavigationStack(path: $navigator.path) {
            content
                .navigationTitle("Bots")
                .toolbar {
                    if let onOpenSidebar {
                        ToolbarItem(placement: .topBarLeading) {
                            Button(action: onOpenSidebar) {
                                Image(systemName: "line.3.horizontal")
                                    .foregroundColor(Color("TextPrimary"))
                            }
                            .accessibilityLabel("Open menu")
                        }
                    }
                }
                .navigationDestination(for: BotRoute.self) { route in
                    BotRouteDestination(route: route)
                }
        }
        .environmentObject(navigator)
        .tint(Color("AccentPrimary"))
        .onAppear(perform: openPendingBot)
        .onChange(of: store.pendingOpenBotId) { _, _ in openPendingBot() }
        .onChange(of: store.pendingNewThread) { _, _ in openPendingBot() }
        .onChange(of: store.pendingOpenPhoneBotId) { _, _ in openPendingBot() }
    }

    private func openPendingBot() {
        if let botId = store.pendingOpenBotId {
            store.pendingOpenBotId = nil
            navigator.path = [.chat(botId: botId, sessionId: nil)]
        }
        if let botId = store.pendingOpenPhoneBotId {
            store.pendingOpenPhoneBotId = nil
            navigator.path = [.phone(botId)]
        }
        if store.pendingNewThread {
            store.pendingNewThread = false
            guard let bot = store.bots.first else { return }
            Task {
                // If the thread can't be made, the bot's main chat still opens.
                let sessionId = try? await store.newThreadSession(for: bot)
                navigator.path = [.chat(botId: bot.id, sessionId: sessionId)]
            }
        }
    }

    @ViewBuilder
    private var content: some View {
        if !auth.isSignedIn && !auth.isSkippingAuth {
            FriendlyStateView(
                style: .empty,
                icon: "person.crop.circle",
                title: "Sign in to meet your bots",
                message: "Your bots, their threads and group chats live in your account.",
                actionTitle: "Sign in",
                action: { auth.isPresentingAuth = true }
            )
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(Color("BgPrimary"))
        } else {
            BotRailList()
        }
    }
}

/// The rail itself (also usable on its own, e.g. inside a drawer).
struct BotRailList: View {
    @ObservedObject private var store = BotModeStore.shared
    @ObservedObject private var hub = AgentHubStore.shared
    @ObservedObject private var status = BotStatusStore.shared
    @EnvironmentObject private var navigator: BotNavigator

    var body: some View {
        let bots = store.bots
        List {
            if !bots.isEmpty {
                Section {
                    QuickBotStrip(bots: bots)
                        .listRowInsets(EdgeInsets())
                        .listRowBackground(Color.clear)
                }
            }

            Section {
                NavigationLink(value: BotRoute.threads(botId: nil)) {
                    railLabel("Threads", icon: "checklist", badge: store.waitingCount)
                }
                NavigationLink(value: BotRoute.groups) {
                    railLabel("Groups", icon: "person.3", badge: 0)
                }
                NavigationLink(value: BotRoute.computers) {
                    railLabel("Computers", icon: "desktopcomputer", badge: 0)
                }
            }

            if !store.projects.isEmpty {
                Section("Projects") {
                    ForEach(store.projects) { project in
                        NavigationLink(value: BotRoute.project(project)) {
                            HStack {
                                Label(project.title, systemImage: project.favorite ? "star.fill" : "folder")
                                    .lineLimit(1)
                                Spacer()
                                if project.counts.waiting > 0 {
                                    WaitingBadge(count: project.counts.waiting)
                                }
                            }
                        }
                    }
                }
            }

            let pinned = bots.filter { store.isPinned($0.id) }
            if !pinned.isEmpty {
                Section("Pinned bots") {
                    ForEach(pinned) { bot in botRow(bot) }
                }
            }

            Section("Group chats") {
                if store.groups.isEmpty {
                    Text("No group chats yet")
                        .font(.footnote)
                        .foregroundColor(Color("TextSecondary"))
                } else {
                    ForEach(store.groups.prefix(6)) { group in
                        NavigationLink(value: BotRoute.group(group.id)) {
                            GroupRowLabel(group: group)
                        }
                    }
                }
            }

            Section("Bots") {
                if hub.isLoading && bots.isEmpty {
                    HStack { ProgressView(); Text("Loading your bots…").foregroundColor(Color("TextSecondary")) }
                } else if let error = hub.loadError, bots.isEmpty {
                    FriendlyStateView(style: .error, icon: "exclamationmark.triangle",
                                      title: "Couldn't load your bots", message: error,
                                      actionTitle: "Retry", action: { hub.fetchAgentsIfNeeded(force: true) })
                } else if bots.isEmpty {
                    Text("No bots yet — create one in the bot hub.")
                        .font(.footnote)
                        .foregroundColor(Color("TextSecondary"))
                } else {
                    ForEach(bots.filter { !store.isPinned($0.id) }) { bot in botRow(bot) }
                }
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .refreshable { await store.refreshAll() }
        .task {
            await store.refreshAll()
            // Threads keep the waiting badge fresh while the rail is up
            // (FabricThreadsPage polls every 15s).
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 15_000_000_000)
                await store.refreshThreads()
            }
        }
    }

    private func botRow(_ bot: AgentRecord) -> some View {
        let waiting = store.waitingCount(for: bot.id) > 0 || status.needsAttention(for: bot.id)
        let working = store.workingCount(for: bot.id) > 0 || status.isWorking(for: bot.id)
        let line = store.threads(for: bot.id).first(where: { $0.group == "waiting" || $0.group == "working" })?.statusLine
        return NavigationLink(value: BotRoute.chat(botId: bot.id, sessionId: nil)) {
            HStack(spacing: 12) {
                BotStatusAvatar(bot: bot, size: 40, working: working, waiting: waiting)
                VStack(alignment: .leading, spacing: 2) {
                    Text(bot.botDisplayName)
                        .font(.body.weight(.medium))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Text(line ?? (waiting ? "Needs you" : working ? "Working…" : bot.botTagline))
                        .font(.caption)
                        .foregroundColor(waiting ? Theme.statusWarning : Color("TextSecondary"))
                        .lineLimit(1)
                }
            }
        }
        .swipeActions(edge: .trailing) {
            Button(store.isPinned(bot.id) ? "Unpin" : "Pin") { store.togglePin(bot.id) }
                .tint(Color("AccentPrimary"))
        }
        .swipeActions(edge: .leading) {
            Button("Profile") { navigator.open(.page(bot.id)) }
        }
    }

    private func railLabel(_ title: String, icon: String, badge: Int) -> some View {
        HStack {
            Label(title, systemImage: icon)
            Spacer()
            if badge > 0 { WaitingBadge(count: badge) }
        }
    }
}

/// Amber "needs you" count (the rail's waiting badge).
struct WaitingBadge: View {
    let count: Int
    var body: some View {
        Text("\(count)")
            .font(.caption2.weight(.semibold))
            .foregroundColor(Theme.statusWarning)
            .padding(.horizontal, 7)
            .padding(.vertical, 2)
            .background(Theme.statusWarning.opacity(0.18))
            .clipShape(Capsule())
            .accessibilityLabel("\(count) waiting on you")
    }
}

/// Quick bot strip (components/bots/QuickBotStrip.tsx): one tap jumps
/// straight into a bot's main chat.
struct QuickBotStrip: View {
    let bots: [AgentRecord]
    @ObservedObject private var store = BotModeStore.shared
    @EnvironmentObject private var navigator: BotNavigator

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 14) {
                ForEach(bots.prefix(12)) { bot in
                    Button {
                        navigator.open(.chat(botId: bot.id, sessionId: nil))
                    } label: {
                        VStack(spacing: 4) {
                            BotStatusAvatar(bot: bot, size: 52,
                                            working: store.workingCount(for: bot.id) > 0,
                                            waiting: store.waitingCount(for: bot.id) > 0,
                                            live: true)
                            Text(bot.botDisplayName)
                                .font(.caption2)
                                .foregroundColor(Color("TextSecondary"))
                                .lineLimit(1)
                                .frame(width: 60)
                        }
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Open \(bot.botDisplayName)")
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 8)
        }
    }
}

/// Resolves a route to its screen.
struct BotRouteDestination: View {
    let route: BotRoute
    @ObservedObject private var store = BotModeStore.shared
    @ObservedObject private var hub = AgentHubStore.shared

    var body: some View {
        switch route {
        case .chat(let botId, let sessionId):
            withBot(botId) { BotChatSessionView(bot: $0, explicitSessionId: sessionId) }
        case .page(let botId):
            withBot(botId) { BotPageView(bot: $0) }
        case .threads(let botId):
            BotThreadsView(botId: botId)
        case .project(let project):
            BotProjectHomeView(project: project)
        case .groups:
            GroupsListView()
        case .group(let id):
            GroupChatView(groupId: id)
        case .computers:
            ComputersListView()
        case .computer(let computer):
            ComputerDriveView(computer: computer)
        case .botComputer(let botId):
            withBot(botId) { BotComputerView(bot: $0) }
        case .tasks(let botId):
            withBot(botId) { BotTasksView(bot: $0) }
        case .routines(let botId):
            withBot(botId) { BotRoutinesView(bot: $0) }
        case .memory(let botId):
            withBot(botId) { BotMemoryView(bot: $0) }
        case .channels(let botId):
            withBot(botId) { BotChannelsView(bot: $0) }
        case .phone(let botId):
            withBot(botId) { BotPhoneView(bot: $0) }
        case .stage(let botId):
            withBot(botId) { BotLiveStageView(bot: $0) }
        case .home(let botId):
            withBot(botId) { BotHomeView(initialAgent: $0) }
        case .settings(let botId):
            withBot(botId) { AgentDetailView(initialAgent: $0) }
        }
    }

    @ViewBuilder
    private func withBot<Content: View>(_ botId: String, @ViewBuilder _ content: (AgentRecord) -> Content) -> some View {
        if let bot = store.bot(botId) {
            content(bot)
        } else if hub.isLoading {
            ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            FriendlyStateView(style: .empty, icon: "questionmark.circle", title: "Bot not found",
                              message: "This bot was removed or isn't shared with you.")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }
}
