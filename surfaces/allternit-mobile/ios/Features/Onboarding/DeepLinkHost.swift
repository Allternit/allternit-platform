import SwiftUI

/// Acts on `DeepLinkRouter.pending` once the workspace is on screen
/// (attached to `MainWorkspaceView`). A link that arrived during a cold
/// launch, sign-in or onboarding waits in the router until this appears.
///
/// - session → Chats with that session (`.openChatSession`)
/// - thread  → the thread's current chat session, else the bot's page
/// - bot     → the bot's page in a sheet
/// - channel → the bots hub
/// - settings → Settings
/// - invite  → the web invite page in-app
/// - computers → the Computers list (Settings → Devices) in a sheet
/// - decisions → Decisions in a sheet
struct DeepLinkHost: ViewModifier {
    @ObservedObject private var router = DeepLinkRouter.shared
    @EnvironmentObject private var modeStore: AppModeStore
    @EnvironmentObject private var agentModeStore: AgentModeStore

    @State private var botSheet: AgentRecord?
    @State private var isSettingsPresented = false
    @State private var isComputersPresented = false
    @State private var isDecisionsPresented = false
    @State private var inviteURL: IdentifiableURL?
    @State private var linkError: String?
    @State private var isResolving = false

    func body(content: Content) -> some View {
        content
            .onAppear { takePending() }
            .onChange(of: router.pending) { _, _ in takePending() }
            .overlay(alignment: .top) {
                if isResolving {
                    HStack(spacing: 8) {
                        ProgressView()
                        Text("Opening…")
                            .font(.subheadline)
                            .foregroundColor(Color("TextPrimary"))
                    }
                    .padding(.horizontal, 14)
                    .padding(.vertical, 8)
                    .background(Color("BgPanel"))
                    .clipShape(Capsule())
                    .shadow(color: .black.opacity(0.12), radius: 8, y: 2)
                    .padding(.top, 8)
                    .transition(.opacity)
                }
            }
            .sheet(item: $botSheet) { agent in
                NavigationStack {
                    BotHomeView(initialAgent: agent)
                        .toolbar {
                            ToolbarItem(placement: .topBarLeading) {
                                Button("Done") { botSheet = nil }
                            }
                        }
                }
                .environmentObject(modeStore)
                .environmentObject(agentModeStore)
            }
            .sheet(isPresented: $isSettingsPresented) {
                SettingsView()
            }
            .sheet(isPresented: $isComputersPresented) {
                NavigationStack {
                    DevicePairingView()
                        .toolbar {
                            ToolbarItem(placement: .topBarLeading) {
                                Button("Done") { isComputersPresented = false }
                            }
                        }
                }
            }
            .sheet(isPresented: $isDecisionsPresented) {
                DecisionsScreen()
            }
            .sheet(item: $inviteURL) { item in
                SafariView(url: item.url)
                    .ignoresSafeArea()
            }
            .alert(
                "Couldn't open that link",
                isPresented: Binding(get: { linkError != nil }, set: { if !$0 { linkError = nil } })
            ) {
                Button("OK", role: .cancel) { linkError = nil }
            } message: {
                Text(linkError ?? "")
            }
    }

    private func takePending() {
        guard let link = router.consume() else { return }
        // A new link replaces whatever a previous link opened.
        botSheet = nil
        isSettingsPresented = false
        isComputersPresented = false
        isDecisionsPresented = false
        inviteURL = nil
        switch link {
        case .session(let id):
            openSession(id)
        case .thread(let botId, let threadId):
            Task { await openThread(botId: botId, threadId: threadId) }
        case .bot(let id):
            Task { await openBot(id) }
        case .channel:
            modeStore.selectBarItem(.agents)
        case .settings:
            isSettingsPresented = true
        case .invite(let url):
            inviteURL = IdentifiableURL(url: url)
        case .computers:
            isComputersPresented = true
        case .decisions:
            isDecisionsPresented = true
        }
    }

    private func openSession(_ id: String, agentId: String? = nil) {
        var info: [String: String] = ["sessionId": id]
        if let agentId { info["agentId"] = agentId }
        NotificationCenter.default.post(name: .openChatSession, object: nil, userInfo: info)
    }

    private func openThread(botId: String, threadId: String) async {
        isResolving = true
        defer { isResolving = false }
        if let threads = try? await ThreadsClient.shared.listThreads(botId: botId),
           let thread = threads.first(where: { $0.id == threadId }),
           let sessionId = thread.currentSessionId, !sessionId.isEmpty {
            openSession(sessionId, agentId: botId)
            return
        }
        // No live session for the thread (or the computer is offline): the
        // bot's page lists its threads.
        await openBot(botId, resolving: false)
    }

    private func openBot(_ id: String, resolving: Bool = true) async {
        if let known = AgentHubStore.shared.agent(withId: id) {
            botSheet = known
            return
        }
        if resolving { isResolving = true }
        defer { if resolving { isResolving = false } }
        do {
            botSheet = try await AgentHubStore.shared.refreshAgent(id: id)
        } catch {
            linkError = "That bot isn't on this account, or your computer is offline. (\(error.localizedDescription))"
        }
    }
}
