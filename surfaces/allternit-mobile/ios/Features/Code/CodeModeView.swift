import SwiftUI

// MARK: - Code mode root

/// Code mode, laid out like the m.allternit.com phone Code screen
/// (fabric-session/phone/screens/LazyScreen.tsx with CodeRoot under it):
///
/// - The "Connect a computer" notice, then the phone header: ☰ on the left;
///   no title on the launchpad (the gizzi code lockup names it), "Terminal"
///   in the terminal. Trailing: the panes menu (in a thread), the open-count
///   button (the sessions overview), New code thread (launchpad only), and
///   Open / Close terminal.
/// - The launchpad (CodeCanvas `LaunchStage`): lockup + What's new, the
///   greeting, "Run a command or describe a task to start coding.", the
///   Show usage pill / usage card, and the terminal-style prompt with its
///   deck row on the bottom edge.
/// - A thread: the session row and the shared chat feed (CodeThreadChatView),
///   or, with Terminal on, the same thread drawn as a terminal session (dark,
///   `❯` prompts, `●` tool lines) with the Code prompt under it. The choice
///   is kept per thread.
/// - The shell (PhoneCodeTerminal, the terminal view's Shell button and the
///   panes menu): the computer's raw shell, one dark surface, header included.
///
/// Threads are tagged `origin_surface='code'` on create because the app
/// mode is `.code` here (ChatViewModel's session context).
struct CodeModeView: View {
    @Binding var isSidebarOpen: Bool
    @Binding var selectedSessionId: String?

    @EnvironmentObject private var modeStore: AppModeStore
    @StateObject private var terminalStore = CodeTerminalStore()
    @StateObject private var instanceStore = InstanceStore.shared

    @State private var sessions: [AgentSession] = []
    @State private var thread: CodeThreadTarget? = nil
    /// The backend session of the thread on screen (set once it exists).
    @State private var activeSessionId: String? = nil
    /// The computer's raw shell is on screen.
    @State private var shellOpen = false
    /// Threads shown as a terminal session: session ids, or a fresh thread's
    /// key until its session exists.
    @State private var terminalKeys = Set(UserDefaults.standard.stringArray(forKey: CodeModeView.terminalKeysKey) ?? [])
    @State private var overviewKind: CodeOverviewKind? = nil
    @State private var paneSheet: PaneSheet? = nil
    @State private var isRenamePresented = false
    @State private var renameText = ""
    @State private var actionError: String? = nil
    @State private var folder: CodeFolderInfo? = nil
    @State private var draft = ""
    @State private var greeting = CodeModeView.greetings.randomElement() ?? "What's up next?"
    @AppStorage("allternit:code:show-usage") private var showUsage = false
    @AppStorage(CodePermissionMode.storageKey) private var permissionRaw = CodePermissionMode.manual.rawValue
    #if DEBUG
    /// One-shot latch for the `-open-code-*` launch args.
    @State private var didApplyDebugArgs = false
    #endif

    static let greetings = [
        "What's up next?",
        "Ready to ship something?",
        "What are we building today?",
        "Pick a task and let's run it.",
        "Need a hand with the codebase?",
    ]

    enum PaneSheet: String, Identifiable {
        case changes, browser, files
        var id: String { rawValue }
    }

    /// Open code threads: code-surface sessions that aren't archived.
    private var codeSessions: [AgentSession] {
        sessions
            .filter { $0.originSurface == AppMode.code.originSurface && $0.active }
            .sorted { CodeSessionDates.timestamp($0) > CodeSessionDates.timestamp($1) }
    }

    private var inThread: Bool { thread != nil }

    static let terminalKeysKey = "allternit:code:terminal-threads"

    /// The thread on screen's key in `terminalKeys`.
    private var threadKey: String? { thread.map { activeSessionId ?? $0.id.uuidString } }

    /// The thread on screen is drawn as a terminal session.
    private var terminalLook: Bool { threadKey.map(terminalKeys.contains) ?? false }

    /// Dark chrome: the shell, or a thread in its terminal look.
    private var darkChrome: Bool { shellOpen || terminalLook }

    var body: some View {
        VStack(spacing: 0) {
            ConnectComputerView()
            header
            ZStack {
                Group {
                    if let thread {
                        CodeThreadChatView(
                            target: thread,
                            title: activeSessionId.flatMap { id in sessions.first { $0.id == id }?.name },
                            folder: folder,
                            onSessionChanged: sessionChanged,
                            onRename: beginRename,
                            onArchive: archiveActive,
                            terminal: terminalLook,
                            onLeaveTerminal: { setTerminalLook(false) },
                            onOpenShell: { shellOpen = true }
                        )
                        .id(thread.id)
                    } else {
                        launchpad
                    }
                }
                .opacity(shellOpen ? 0 : 1)
                .allowsHitTesting(!shellOpen)
                if shellOpen {
                    PhoneCodeTerminal(store: terminalStore)
                }
            }
        }
        .background((darkChrome ? TerminalPalette.background : Color("BgPrimary")).ignoresSafeArea())
        .task {
            await instanceStore.refreshIfNeeded()
            await loadSessions()
            await loadFolder()
            #if DEBUG
            applyDebugArgs()
            #endif
        }
        .onChange(of: selectedSessionId) { _, id in
            // A code session picked in the drawer opens here.
            if let id, modeStore.mode == .code {
                open(sessionId: id)
                DispatchQueue.main.async { selectedSessionId = nil }
            }
        }
        .onChange(of: permissionRaw) { _, _ in
            if let activeSessionId { applyPermission(to: activeSessionId) }
        }
        .sheet(item: $overviewKind) { kind in
            CodeOverviewSheet(
                kind: kind,
                sessions: codeSessions,
                terminals: terminalStore,
                onOpenSession: { session in
                    shellOpen = false
                    open(sessionId: session.id)
                },
                onNew: newThread,
                onArchive: { session in archive(session.id) },
                onOpenMenu: openDrawer
            )
            .presentationDragIndicator(.visible)
        }
        .sheet(item: $paneSheet) { sheet in
            switch sheet {
            case .files:
                FileBrowserView(instanceStore: instanceStore)
            case .changes:
                if let activeSessionId {
                    SessionDiffListView(sessionId: activeSessionId, instanceStore: instanceStore)
                }
            case .browser:
                if let ptyId = terminalStore.livePtyId {
                    DevServerPortsView(ptyID: ptyId, instanceStore: instanceStore)
                }
            }
        }
        .alert("Rename", isPresented: $isRenamePresented) {
            TextField("Session name", text: $renameText)
            Button("Cancel", role: .cancel) {}
            Button("Rename", action: commitRename)
        }
        .alert("Couldn't update the session", isPresented: Binding(get: { actionError != nil }, set: { if !$0 { actionError = nil } })) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(actionError ?? "")
        }
    }

    // MARK: - Header

    private var header: some View {
        let ink = darkChrome ? TerminalPalette.foreground : Color("TextPrimary")
        return HStack(spacing: 0) {
            Button(action: openDrawer) {
                Image(systemName: "line.3.horizontal")
                    .font(.system(size: 19, weight: .regular))
                    .foregroundColor(ink)
                    .frame(width: 44, height: 44)
            }
            .accessibilityLabel("Open menu")

            Spacer()

            if inThread && !shellOpen {
                panesMenu(ink: ink)
            }
            CodeOverviewCountButton(
                count: shellOpen ? terminalStore.tiles.count : codeSessions.count,
                noun: shellOpen ? "terminal sessions" : "code threads",
                tint: ink
            ) {
                overviewKind = shellOpen ? .terminals : .threads
            }
            if !shellOpen && !inThread {
                Button(action: newThread) {
                    Image(systemName: "square.and.pencil")
                        .font(.system(size: 19))
                        .foregroundColor(ink)
                        .frame(width: 44, height: 44)
                }
                .accessibilityLabel("New code thread")
            }
            Button(action: {
                if shellOpen {
                    shellOpen = false
                } else if terminalLook {
                    setTerminalLook(false)
                } else {
                    openTerminal()
                }
            }) {
                Image(systemName: shellOpen ? "xmark" : (terminalLook ? "bubble.left.and.bubble.right" : "apple.terminal"))
                    .font(.system(size: 18))
                    .foregroundColor(ink)
                    .frame(width: 44, height: 44)
            }
            .accessibilityLabel(shellOpen ? "Close shell" : (terminalLook ? "Show chat" : "Open terminal"))
            .accessibilityAddTraits(terminalLook ? .isSelected : [])
        }
        .overlay {
            if shellOpen {
                Text("Shell")
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundColor(ink)
                    .allowsHitTesting(false)
            }
        }
        .padding(.horizontal, 6)
        .frame(height: 52)
        .background(darkChrome ? TerminalPalette.background : Color("BgPrimary"))
    }

    /// The session's panes and actions as one header button
    /// (PhonePanesButton): Panes, then the session actions.
    private func panesMenu(ink: Color) -> some View {
        Menu {
            Section("Panes") {
                Button(action: {}) { Label("Session", systemImage: "sidebar.left") }
                Button(action: { setTerminalLook(!terminalLook) }) {
                    Label(terminalLook ? "Chat" : "Terminal", systemImage: terminalLook ? "bubble.left.and.bubble.right" : "apple.terminal")
                }
                Button(action: { shellOpen = true }) { Label("Shell", systemImage: "chevron.left.forwardslash.chevron.right") }
                if activeSessionId != nil {
                    Button(action: { paneSheet = .changes }) { Label("Changes", systemImage: "plus.forwardslash.minus") }
                }
                if terminalStore.livePtyId != nil {
                    Button(action: { paneSheet = .browser }) { Label("Browser", systemImage: "globe") }
                }
                Button(action: { paneSheet = .files }) { Label("Files", systemImage: "folder") }
            }
            if activeSessionId != nil {
                Section {
                    Button(action: beginRename) { Label("Rename", systemImage: "pencil") }
                    if let activeSessionId {
                        Button(action: { UIPasteboard.general.string = activeSessionId }) {
                            Label("Copy session ID", systemImage: "doc.on.doc")
                        }
                    }
                    Button(role: .destructive, action: archiveActive) { Label("Archive", systemImage: "archivebox") }
                }
            }
        } label: {
            Image(systemName: "square.grid.2x2")
                .font(.system(size: 18))
                .foregroundColor(ink)
                .frame(width: 44, height: 44)
        }
        .accessibilityLabel("Session panes")
    }

    // MARK: - Launchpad

    private var launchpad: some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    HStack(spacing: 12) {
                        GizziCodeWordmark(height: 15)
                        Spacer(minLength: 8)
                        CodeWhatsNewButton()
                    }
                    .padding(.bottom, 20)

                    Text(greetingText)
                        .font(.system(size: 28, weight: .semibold, design: .serif))
                        .tracking(-0.56)
                        .foregroundColor(Color("TextPrimary"))
                        .fixedSize(horizontal: false, vertical: true)
                        .accessibilityAddTraits(.isHeader)

                    Text("Run a command or describe a task to start coding.")
                        .font(.system(size: 14))
                        .lineSpacing(3)
                        .foregroundColor(Color("TextSecondary"))
                        .padding(.top, 8)

                    if showUsage {
                        CodeUsageDashboard(sessions: codeSessions) { showUsage = false }
                            .padding(.top, 16)
                    } else {
                        Button(action: { showUsage = true }) {
                            HStack(spacing: 6) {
                                Image(systemName: "chart.bar")
                                    .font(.system(size: 13))
                                Text("Show usage")
                                    .font(.system(size: 13, weight: .semibold))
                            }
                            .foregroundColor(Color("TextPrimary"))
                            .padding(.horizontal, 12)
                            .frame(height: 32)
                            .background(Color("BgTertiary"))
                            .clipShape(RoundedRectangle(cornerRadius: 10))
                        }
                        .buttonStyle(.plain)
                        .padding(.top, 24)
                    }
                }
                .padding(.horizontal, 14)
                .padding(.top, 48)
                .padding(.bottom, 24)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .scrollDismissesKeyboard(.interactively)
            .refreshable { await loadSessions() }

            CodeLaunchComposer(
                text: $draft,
                folder: folder,
                onSend: { prompt in
                    thread = CodeThreadTarget(sessionId: nil, initialPrompt: prompt)
                    activeSessionId = nil
                },
                onOpenTerminal: openTerminal,
                onAttach: {
                    // Attachments live in the thread composer: carry the
                    // draft there and open a fresh thread.
                    OnboardingStore.shared.pendingPrompt = draft.isEmpty ? nil : draft
                    draft = ""
                    thread = CodeThreadTarget(sessionId: nil)
                    activeSessionId = nil
                }
            )
            .padding(.bottom, 8)
        }
    }

    /// The greeting addresses the user by first name (CodeCanvas `greetingText`).
    private var greetingText: String {
        guard let name = AuthManager.shared.firstName else { return greeting }
        return greeting.hasSuffix("?") ? "\(greeting.dropLast()), \(name)?" : "\(greeting) \(name)"
    }

    // MARK: - Actions

    private func openDrawer() {
        UIImpactFeedbackGenerator(style: .medium).impactOccurred()
        withAnimation(.spring(response: 0.35, dampingFraction: 0.86, blendDuration: 0)) {
            isSidebarOpen = true
        }
    }

    /// Terminal from the launchpad opens a fresh thread in its terminal
    /// look; in a thread it flips that thread.
    private func openTerminal() {
        if thread == nil {
            thread = CodeThreadTarget(sessionId: nil)
            activeSessionId = nil
        }
        setTerminalLook(true)
    }

    private func setTerminalLook(_ on: Bool) {
        guard let key = threadKey else { return }
        if on { terminalKeys.insert(key) } else { terminalKeys.remove(key) }
        saveTerminalKeys()
    }

    private func saveTerminalKeys() {
        // Fresh-thread keys (UUIDs) don't outlive the launch; session ids do.
        UserDefaults.standard.set(terminalKeys.filter { UUID(uuidString: $0) == nil }.sorted(), forKey: Self.terminalKeysKey)
    }

    /// New code thread: back to the launchpad (the web's `setActiveSession(null)`).
    private func newThread() {
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
        thread = nil
        activeSessionId = nil
        greeting = Self.greetings.randomElement() ?? greeting
    }

    private func open(sessionId: String) {
        thread = CodeThreadTarget(sessionId: sessionId)
        activeSessionId = sessionId
    }

    private func sessionChanged(_ id: String?) {
        guard let id, id != activeSessionId else { return }
        let isNew = activeSessionId == nil
        if isNew, let fresh = thread?.id.uuidString, terminalKeys.remove(fresh) != nil {
            // The fresh thread's terminal look moves to its new session.
            terminalKeys.insert(id)
            saveTerminalKeys()
        }
        activeSessionId = id
        if isNew, CodePermissionMode(rawValue: permissionRaw) != .manual {
            applyPermission(to: id)
        }
        if isNew, UserDefaults.standard.bool(forKey: CodeWorktree.storageKey) {
            // The launch's Worktree pick isolates this thread (CodeCanvas
            // `isolation: pendingWorktree ? 'worktree' : 'none'`).
            UserDefaults.standard.set(false, forKey: CodeWorktree.storageKey)
            Task {
                do {
                    try await AgentChatClient().updateSessionMetadata(sessionId: id, metadata: ["isolation": "worktree"])
                } catch {
                    actionError = FriendlyErrorMessage.from(error.localizedDescription)
                }
            }
        }
        Task {
            // The session names itself from the first turn; pick that up.
            await loadSessions()
            try? await Task.sleep(for: .seconds(4))
            await loadSessions()
        }
    }

    private func applyPermission(to sessionId: String) {
        let mode = CodePermissionMode(rawValue: permissionRaw) ?? .manual
        Task {
            do {
                try await CodeSessionsClient().applyPermission(sessionId: sessionId, mode: mode)
            } catch {
                actionError = FriendlyErrorMessage.from(error.localizedDescription)
            }
        }
    }

    private func beginRename() {
        guard let activeSessionId else { return }
        renameText = sessions.first { $0.id == activeSessionId }?.name ?? ""
        isRenamePresented = true
    }

    private func commitRename() {
        let name = renameText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let activeSessionId, !name.isEmpty else { return }
        Task {
            do {
                try await CodeSessionsClient().rename(sessionId: activeSessionId, name: name)
                await loadSessions()
            } catch {
                actionError = FriendlyErrorMessage.from(error.localizedDescription)
            }
        }
    }

    private func archiveActive() {
        guard let activeSessionId else { return }
        archive(activeSessionId)
    }

    /// Archive (closing a card, or Archive from a menu): it leaves the open
    /// list; the thread on screen goes back to the launchpad.
    private func archive(_ sessionId: String) {
        if sessionId == activeSessionId { newThread() }
        sessions.removeAll { $0.id == sessionId }
        Task {
            do {
                try await CodeSessionsClient().archive(sessionId: sessionId)
            } catch {
                actionError = FriendlyErrorMessage.from(error.localizedDescription)
            }
            await loadSessions()
        }
    }

    // MARK: - Loading

    @MainActor
    private func loadSessions() async {
        // GET /api/v1/agent-sessions → { sessions, count }; a failed read
        // keeps what is on screen.
        if let list = try? await AgentChatClient().listSessions() {
            sessions = list
        }
    }

    /// The computer's working folder and branch (gizzi-code `v1/path` + `v1/vcs`).
    @MainActor
    private func loadFolder() async {
        guard let baseURL = await InstanceConnection.resolveBaseURL(from: instanceStore) else {
            folder = nil
            return
        }
        folder = try? await CodeFolderClient.load(baseURL: baseURL)
    }

    #if DEBUG
    /// `-open-code-thread`, `-open-code-thread-id <id>`, `-open-code-terminal`, `-open-code-shell`,
    /// `-open-code-overview`, `-open-code-usage`, `-mesh-proxy-selfcheck`
    /// (DEBUG only): land on a state for screenshot verification. One-shot.
    private func applyDebugArgs() {
        guard !didApplyDebugArgs else { return }
        didApplyDebugArgs = true
        if launchArgumentEnabled("open-code-thread") {
            thread = CodeThreadTarget(sessionId: nil)
        }
        if let threadId = UserDefaults.standard.string(forKey: "open-code-thread-id") {
            open(sessionId: threadId)
        }
        if launchArgumentEnabled("open-code-terminal") {
            openTerminal()
        }
        if launchArgumentEnabled("open-code-shell") {
            shellOpen = true
        }
        if launchArgumentEnabled("open-code-overview") {
            overviewKind = .threads
        }
        if launchArgumentEnabled("open-code-usage") {
            showUsage = true
        }
        if launchArgumentEnabled("mesh-proxy-selfcheck") {
            MeshClient.runProxySelfCheck()
        }
    }
    #endif
}
