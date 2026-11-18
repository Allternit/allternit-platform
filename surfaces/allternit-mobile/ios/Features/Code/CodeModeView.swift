import SwiftUI

// MARK: - Code mode root

/// Code mode, laid out like the m.allternit.com phone Code screen
/// (fabric-session/phone/screens/LazyScreen.tsx with CodeRoot under it):
///
/// - The "Connect a computer" notice, then the phone header: ☰ on the left,
///   the view button (Code · Terminal · Shell picker) and the sessions
///   button (a task list) on the right, and between them the gizzi code
///   lockup on the launchpad, the session's name (its menu) in a thread, or
///   "Shell". A thread adds the Panes menu (every desktop pane, the session
///   panel and background tasks) before them.
/// - The launchpad (CodeCanvas `LaunchStage`): What's new under the sessions
///   button, the greeting, "Run a command or describe a task to start
///   coding.", the Show usage pill / usage card, and the Code prompt.
/// - A thread: the shared chat feed (CodeThreadChatView), or, with Terminal
///   on (the view picker, or the composer's one-tap Chat/Terminal toggle),
///   the same thread drawn as a terminal session (dark, `❯` prompts, `●`
///   tool lines). The choice is kept per thread.
/// - The shell (PhoneCodeTerminal, from the view picker or the Terminal
///   pane): the computer's raw shell, one dark surface, header included;
///   the view picker leads back to Code or Terminal.
///
/// Threads are tagged `origin_surface='code'` on create because the app
/// mode is `.code` here (ChatViewModel's session context).
struct CodeModeView: View {
    @Binding var isSidebarOpen: Bool
    @Binding var selectedSessionId: String?

    @EnvironmentObject private var modeStore: AppModeStore
    @EnvironmentObject private var agentModeStore: AgentModeStore
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
    /// The Code sessions view (phone calm I9), and where it opens.
    @State private var sessionsView: CodeSessionsOpening? = nil
    @Namespace private var sessionsZoom
    @ReducedMotion private var reduceMotion
    @State private var pane: CodePaneKind? = nil
    @State private var isRenamePresented = false
    @State private var renameText = ""
    @State private var actionError: String? = nil
    @State private var folder: CodeFolderInfo? = nil
    @ObservedObject private var backdrops = StageBackdropStore.shared
    @State private var isBackgroundPresented = false
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

    /// Open code threads: code-surface sessions that aren't archived.
    private var codeSessions: [AgentSession] {
        sessions
            .filter { $0.originSurface == AppMode.code.originSurface && $0.active }
            .sorted { CodeSessionDates.timestamp($0) > CodeSessionDates.timestamp($1) }
    }

    private var inThread: Bool { thread != nil }

    /// Every Code-surface session, archived ones too (the sessions view's
    /// Archived filter), newest first.
    private var allCodeSessions: [AgentSession] {
        sessions
            .filter { $0.originSurface == AppMode.code.originSurface }
            .sorted { CodeSessionDates.timestamp($0) > CodeSessionDates.timestamp($1) }
    }

    /// iOS 18+ zooms the sessions view out of its icon
    /// (`.navigationTransition(.zoom)`); iOS 17 scales it from the corner,
    /// and reduced motion cross-fades in 120 ms (section 6).
    private var zoomsFromIcon: Bool {
        if #available(iOS 18.0, *) { return !reduceMotion }
        return false
    }

    static let terminalKeysKey = "allternit:code:terminal-threads"

    /// The thread on screen's key in `terminalKeys`.
    private var threadKey: String? { thread.map { activeSessionId ?? $0.id.uuidString } }

    /// The thread on screen is drawn as a terminal session.
    private var terminalLook: Bool { threadKey.map(terminalKeys.contains) ?? false }

    /// Dark chrome: the shell, or a thread in its terminal look.
    private var darkChrome: Bool { shellOpen || terminalLook }

    /// This workspace's background: its own look, else Settings (Code mode is
    /// off there until switched on). The shell keeps its plain terminal.
    private var backdropScope: BackdropScope { .code(folder?.directory) }
    private var codeBackdrop: String { backdrops.variant(for: .code, scope: backdropScope) }
    private var showsBackdrop: Bool { codeBackdrop != "plain" && !shellOpen && !terminalLook }

    var body: some View {
        VStack(spacing: 0) {
            ConnectComputerView()
            header
            ZStack {
                Group {
                    if let thread {
                        CodeThreadChatView(
                            target: thread,
                            folder: folder,
                            onSessionChanged: sessionChanged,
                            terminal: terminalLook,
                            onLeaveTerminal: { setTerminalLook(false) },
                            onEnterTerminal: { setTerminalLook(true) },
                            paused: shellOpen
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
                        // The shell has no composer: the view switch sits
                        // bottom right, over the terminal.
                        .overlay(alignment: .bottomTrailing) {
                            CodeViewMenu(current: .shell, tint: TerminalPalette.foreground)
                                .background(TerminalPalette.key, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                                .padding(.trailing, 12)
                                .padding(.bottom, 64)
                        }
                }
            }
        }
        .background {
            if showsBackdrop {
                // No glass panels in Code: the veil covers the whole view, at least 55%.
                StageBackdropView(variant: codeBackdrop, minVeil: 55,
                                  veilColor: terminalLook ? TerminalPalette.background : Color("BgPrimary"),
                                  darkOverride: terminalLook ? true : nil)
                    .ignoresSafeArea()
            } else {
                (darkChrome ? TerminalPalette.background : Color("BgPrimary")).ignoresSafeArea()
            }
        }
        .overlay {
            if let opening = sessionsView, !zoomsFromIcon {
                sessionsViewContent(opening)
                    .transition(reduceMotion ? .opacity : .scale(scale: 0.06, anchor: .topTrailing).combined(with: .opacity))
                    .zIndex(10)
            }
        }
        .fullScreenCover(isPresented: Binding(
            get: { sessionsView != nil && zoomsFromIcon },
            set: { if !$0 { sessionsView = nil } }
        )) {
            // Kept drawn while it zooms back into the icon (the opening only
            // matters on first appear).
            sessionsViewContent(sessionsView ?? CodeSessionsOpening())
                .modifier(CodeSessionsZoomDestination(namespace: sessionsZoom))
        }
        .backdropTouches(showsBackdrop)
        .environment(\.backdropUnderneath, showsBackdrop)
        .publishesRailBackdrop(showsBackdrop ? RailBackdropStore.Look(colors: [], variant: codeBackdrop, mode: .code) : nil)
        .sheet(isPresented: $isBackgroundPresented) {
            StageBackdropScopeSheet(scope: backdropScope, title: "\(folder?.name ?? "Code") background")
        }
        .task {
            await instanceStore.refreshIfNeeded()
            await loadSessions()
            await loadFolder()
            #if DEBUG
            applyDebugArgs()
            #endif
        }
        .onReceive(NotificationCenter.default.publisher(for: .codePickView)) { note in
            if let kind = note.object as? CodeViewKind { selectView(kind) }
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
                onNew: {
                    shellOpen = false
                    newThread()
                },
                onArchive: { session in archive(session.id) },
                onOpenMenu: openDrawer
            )
            .presentationDragIndicator(.visible)
        }
        .sheet(item: $pane) { kind in
            CodePaneSheet(
                kind: kind,
                sessionId: activeSessionId,
                folder: folder,
                instanceStore: instanceStore,
                terminals: terminalStore,
                modeStore: modeStore,
                agentModeStore: agentModeStore,
                onOpenMenu: openDrawer
            )
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
            // Tap opens the drawer; hold fans the modes out (C8).
            PhoneMenuButton(tint: ink, glyphSize: 19, onOpen: openDrawer)

            Spacer(minLength: 4)

            // Panes belong to a session: only once the thread has one.
            if inThread && activeSessionId != nil && !shellOpen {
                panesMenu(ink: ink)
            }
            // Code home and threads: the Code sessions view, grown out of
            // this icon. The shell keeps its terminal sessions overview.
            CodeSessionsButton(tint: ink) {
                if shellOpen { overviewKind = .terminals } else { openSessionsView() }
            }
            .modifier(CodeSessionsZoomSource(namespace: sessionsZoom))
        }
        .padding(.horizontal, 6)
        .frame(height: 52)
        .overlay {
            headerTitle(ink: ink)
                .padding(.horizontal, inThread && activeSessionId != nil && !shellOpen ? 100 : 56)
        }
        .background(darkChrome ? TerminalPalette.background : Color("BgPrimary"))
    }

    /// Between ☰ and the right icons: the gizzi code lockup on the launchpad,
    /// the session's name (its menu) in a thread, "Shell" in the shell.
    @ViewBuilder
    private func headerTitle(ink: Color) -> some View {
        if shellOpen {
            Text("Shell")
                .font(.system(size: 15, weight: .semibold, design: .monospaced))
                .foregroundColor(ink)
        } else if inThread {
            Menu {
                Button(action: beginRename) { Label("Rename", systemImage: "pencil") }
                if let activeSessionId {
                    Button(action: { UIPasteboard.general.string = activeSessionId }) {
                        Label("Copy session ID", systemImage: "doc.on.doc")
                    }
                }
                Button { isBackgroundPresented = true } label: { Label("Background", systemImage: "paintbrush") }
                Divider()
                Button(role: .destructive, action: archiveActive) { Label("Archive", systemImage: "archivebox") }
            } label: {
                HStack(spacing: 5) {
                    Text(threadTitle)
                        .font(.system(size: 15, weight: .semibold, design: terminalLook ? .monospaced : .default))
                        .foregroundColor(ink)
                        .lineLimit(1)
                        .truncationMode(.tail)
                    if activeSessionId != nil {
                        Image(systemName: "chevron.down")
                            .font(.system(size: 10, weight: .bold))
                            .foregroundColor(ink.opacity(0.6))
                    }
                }
            }
            .disabled(activeSessionId == nil)
            .accessibilityLabel("Session: \(threadTitle)")
        } else {
            GizziCodeWordmark(height: 15)
                .accessibilityAddTraits(.isHeader)
        }
    }

    /// The thread's name; a fresh thread shows its launch prompt until the
    /// session names itself.
    private var threadTitle: String {
        if let activeSessionId, let name = sessions.first(where: { $0.id == activeSessionId })?.name, !name.isEmpty {
            return name
        }
        if let prompt = thread?.initialPrompt, !prompt.isEmpty {
            return prompt.prefix(1).uppercased() + prompt.dropFirst()
        }
        return "New thread"
    }

    /// The session's panes (pane-manager `CODE_PANE_STRIP` plus the session
    /// panel and background tasks), as one drop-down.
    private func panesMenu(ink: Color) -> some View {
        Menu {
            ForEach(CodePaneKind.allCases) { kind in
                Button(action: { openPane(kind) }) { Label(kind.label, systemImage: kind.icon) }
            }
        } label: {
            Image(systemName: "rectangle.split.3x1")
                .font(.system(size: 17))
                .foregroundColor(ink)
                .frame(width: 44, height: 44)
        }
        .accessibilityLabel("Panes")
    }

    private func openPane(_ kind: CodePaneKind) {
        if kind == .terminal {
            shellOpen = true
        } else {
            pane = kind
        }
    }

    // MARK: - Launchpad

    private var launchpad: some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
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
                .padding(.top, 28)
                .padding(.bottom, 24)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            // What's new sits right under the sessions button.
            .safeAreaInset(edge: .top, spacing: 0) {
                HStack {
                    Spacer()
                    CodeWhatsNewButton()
                }
                .padding(.horizontal, 10)
                .padding(.top, 2)
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

    // MARK: - Code sessions view

    private func sessionsViewContent(_ opening: CodeSessionsOpening) -> some View {
        CodeSessionsView(
            sessions: allCodeSessions,
            folder: folder,
            opening: opening,
            onClose: closeSessionsView,
            onOpenSession: { session in
                closeSessionsView()
                shellOpen = false
                open(sessionId: session.id)
            },
            onArchive: { session in archive(session.id) },
            onStart: { prompt in
                closeSessionsView()
                shellOpen = false
                startFromSessionsView(prompt)
            },
            onRefresh: { await loadSessions() },
            onOpenCards: {
                // The card overview is a sheet on this view: close the
                // sessions view first so it can present.
                closeSessionsView()
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.45) { overviewKind = .threads }
            }
        )
    }

    private func openSessionsView(_ opening: CodeSessionsOpening = CodeSessionsOpening()) {
        Haptics.light()
        Task { await loadSessions() }
        if zoomsFromIcon {
            sessionsView = opening
        } else {
            withAnimation(reduceMotion ? Motion.fade : .spring(response: 0.36, dampingFraction: 0.86)) {
                sessionsView = opening
            }
        }
    }

    private func closeSessionsView() {
        if zoomsFromIcon {
            sessionsView = nil
        } else {
            withAnimation(reduceMotion ? Motion.fade : .spring(response: 0.32, dampingFraction: 0.9)) {
                sessionsView = nil
            }
        }
    }

    /// The start screen's prompt goes through the launchpad's own start path
    /// (`CodeLaunchComposer.onSend`). With no brain picked it only fills the
    /// launchpad composer, as on the web.
    private func startFromSessionsView(_ prompt: String) {
        activeSessionId = nil
        if (ModelStore.shared.selectedModelId ?? "").isEmpty {
            thread = nil
            draft = prompt
        } else {
            thread = CodeThreadTarget(sessionId: nil, initialPrompt: prompt)
        }
    }

    // MARK: - Actions

    private func openDrawer() {
        Haptics.medium()
        withAnimation(.spring(response: 0.35, dampingFraction: 0.86, blendDuration: 0)) {
            isSidebarOpen = true
        }
    }

    /// What's on screen: the chat, a thread's terminal look, or the shell.
    private var viewKind: CodeViewKind {
        shellOpen ? .shell : (terminalLook ? .terminal : .code)
    }

    /// The view picker's choice. Code closes the shell and leaves the
    /// terminal look; Terminal closes the shell and opens the terminal look;
    /// Shell opens the computer's raw shell.
    private func selectView(_ kind: CodeViewKind) {
        switch kind {
        case .code:
            shellOpen = false
            if terminalLook { setTerminalLook(false) }
        case .terminal:
            shellOpen = false
            openTerminal()
        case .shell:
            shellOpen = true
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
        Haptics.light()
        thread = nil
        activeSessionId = nil
        greeting = Self.greetings.randomElement() ?? greeting
    }

    private func open(sessionId: String) {
        thread = CodeThreadTarget(sessionId: sessionId)
        activeSessionId = sessionId
        markReviewed(sessionId)
    }

    /// Opening a session clears its "Ready for review" flag (C6). Only
    /// when it is set, so a plain open makes no extra write; a failure is
    /// silent (the flag just stays until the next open).
    private func markReviewed(_ sessionId: String) {
        if let known = sessions.first(where: { $0.id == sessionId }), !known.readyForReview { return }
        Task { try? await CodeSessionsClient().markReviewed(sessionId: sessionId) }
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
    /// `-open-code-overview`, `-open-code-usage`, `-open-code-pane <kind>`, `-mesh-proxy-selfcheck`
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
        // `-open-code-pane <kind>` (session, changes, browser, computer, files,
        // artifacts, transcript, tasks, codemap): opens a thread, then the pane.
        if let raw = UserDefaults.standard.string(forKey: "open-code-pane"), let kind = CodePaneKind(rawValue: raw) {
            if thread == nil { thread = CodeThreadTarget(sessionId: nil) }
            openPane(kind)
        }
        if launchArgumentEnabled("mesh-proxy-selfcheck") {
            MeshClient.runProxySelfCheck()
        }
        // Code sessions view (phone calm I9): `-calm-code-sessions`,
        // `-calm-code-filter=<filter>`, `-calm-code-scheduled`,
        // `-calm-code-start`, `-calm-code-device`, `-calm-code-connect`.
        var opening = CodeSessionsOpening()
        var wantsSessions = launchArgumentEnabled(LaunchArgument.calmCodeSessions)
        if let raw = launchArgumentValue(LaunchArgument.calmCodeFilter), let filter = CodeSessionFilter(argument: raw) {
            opening.filter = filter
            wantsSessions = true
        }
        if launchArgumentEnabled(LaunchArgument.calmCodeScheduled) {
            opening.screen = .scheduled
            wantsSessions = true
        }
        if launchArgumentEnabled(LaunchArgument.calmCodeStart) {
            opening.screen = .start
            wantsSessions = true
        }
        if launchArgumentEnabled(LaunchArgument.calmCodeDevice) {
            opening.deviceSheet = true
            wantsSessions = true
        }
        if launchArgumentEnabled(LaunchArgument.calmCodeConnect) {
            opening.connectSheet = true
            wantsSessions = true
        }
        if wantsSessions { sessionsView = opening }
    }
    #endif
}

// MARK: - View picker

/// Code mode's three views: the thread as a chat, the thread in its
/// terminal look, or the computer's raw shell.
enum CodeViewKind: CaseIterable {
    case code, terminal, shell

    var label: String {
        switch self {
        case .code: return "Code"
        case .terminal: return "Terminal"
        case .shell: return "Shell"
        }
    }

    var detail: String {
        switch self {
        case .code: return "The session as a chat"
        case .terminal: return "The same session, drawn as a terminal"
        case .shell: return "The computer's own command line"
        }
    }

    var icon: String {
        switch self {
        case .code: return "bubble.left"
        case .terminal: return "apple.terminal"
        case .shell: return "chevron.left.forwardslash.chevron.right"
        }
    }
}

/// The header's view button: shows the view on screen, opens the picker.
extension Notification.Name {
    /// A view picked in the Code composer's view switch (object: CodeViewKind).
    static let codePickView = Notification.Name("allternit.code.pick-view")
}

/// The Code view switch (bottom right of the composer, or of the shell): one
/// tap opens Code · Terminal · Shell with the current one checked.
struct CodeViewMenu: View {
    let current: CodeViewKind
    var tint: Color = Color("TextPrimary")

    var body: some View {
        Menu {
            ForEach(CodeViewKind.allCases, id: \.self) { kind in
                Button {
                    NotificationCenter.default.post(name: .codePickView, object: kind)
                } label: {
                    if kind == current {
                        Label(kind.label, systemImage: "checkmark")
                    } else {
                        Label(kind.label, systemImage: kind.icon)
                    }
                }
            }
        } label: {
            Image(systemName: current.icon)
                .font(.system(size: 15))
                .foregroundColor(tint)
                .frame(width: 40, height: 40)
                .contentShape(Rectangle())
        }
        .accessibilityLabel("View: \(current.label)")
        .accessibilityHint("Switch between Code, Terminal and Shell")
    }
}


// MARK: - Zoom from the sessions icon

/// The sessions icon is the zoom's source on iOS 18+ (no-op on 17).
private struct CodeSessionsZoomSource: ViewModifier {
    let namespace: Namespace.ID

    func body(content: Content) -> some View {
        if #available(iOS 18.0, *) {
            content.matchedTransitionSource(id: CodeSessionsZoomID.value, in: namespace)
        } else {
            content
        }
    }
}

/// The Code sessions view zooms out of the icon on iOS 18+.
private struct CodeSessionsZoomDestination: ViewModifier {
    let namespace: Namespace.ID

    func body(content: Content) -> some View {
        if #available(iOS 18.0, *) {
            content.navigationTransition(.zoom(sourceID: CodeSessionsZoomID.value, in: namespace))
        } else {
            content
        }
    }
}

private enum CodeSessionsZoomID {
    static let value = "code-sessions-icon"
}
