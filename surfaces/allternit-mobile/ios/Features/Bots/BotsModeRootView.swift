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
    /// The bot screen on one of its tabs (bot-home-tabs.ts).
    case botHome(String, BotHomeTab)
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
/// it as the Bots mode root). The PWA's BotsScreen on the hub: header
/// "Bots" with Connections and Replay intro, and the bots launchpad
/// (BotLaunchpadView). The drawer is the bots rail while this mode is open.
struct BotsModeRootView: View {
    @StateObject private var store = BotModeStore.shared
    @StateObject private var hub = AgentHubStore.shared
    @StateObject private var auth = AuthManager.shared
    @StateObject private var navigator = BotNavigator()
    @State private var isConnectionsPresented = false

    /// Opens the shell drawer (the edge swipe also works).
    var onOpenSidebar: (() -> Void)?

    init(onOpenSidebar: (() -> Void)? = nil) {
        self.onOpenSidebar = onOpenSidebar
    }

    var body: some View {
        NavigationStack(path: $navigator.path) {
            content
                .navigationTitle("Bots")
                .navigationBarTitleDisplayMode(.inline)
                .toolbarBackground(Color("BgPrimary"), for: .navigationBar)
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
                    ToolbarItemGroup(placement: .topBarTrailing) {
                        // Connections: the apps your bots can use.
                        Button { isConnectionsPresented = true } label: {
                            Image(systemName: "square.grid.2x2")
                                .foregroundColor(Color("TextPrimary"))
                        }
                        .accessibilityLabel("Connections")
                        // Meet your bots again (the rail's ⋯ has the same entry).
                        Button { BotsIntroStore.shared.replay() } label: {
                            Image(systemName: "arrow.counterclockwise")
                                .foregroundColor(Color("TextPrimary"))
                        }
                        .accessibilityLabel("Replay intro")
                    }
                }
                .navigationDestination(for: BotRoute.self) { route in
                    BotRouteDestination(route: route)
                }
        }
        .environmentObject(navigator)
        .tint(Color("AccentPrimary"))
        .sheet(isPresented: $isConnectionsPresented) {
            ConnectorsListView()
        }
        .onAppear(perform: openPendingBot)
        .onChange(of: store.pendingOpenBotId) { _, _ in openPendingBot() }
        .onChange(of: store.pendingNewThread) { _, _ in openPendingBot() }
        .onChange(of: store.pendingOpenPhoneBotId) { _, _ in openPendingBot() }
        .onChange(of: store.pendingRoute) { _, _ in openPendingBot() }
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
        if let route = store.pendingRoute {
            store.pendingRoute = nil
            navigator.path = [route]
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
            BotLaunchpadView()
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
        case .botHome(let botId, let tab):
            withBot(botId) { BotHomeTabsView(bot: $0, initialTab: tab) }
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
