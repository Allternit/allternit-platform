import SwiftUI

// MARK: - Mode host shell

/// Top-level shell (the PWA phone shell, allternit-ai
/// src/fabric-session/phone/PhoneShell.tsx): one section fills the screen,
/// no bottom bar, with the one no-computer banner above it. The menu is the
/// Arc drawer (HistorySidebarView): min(92% of the screen, 440) wide, it
/// springs in OVER the screen from the left edge with a 16% scrim on the
/// strip that stays visible; the screen itself doesn't move. It closes with
/// a tap on the scrim or a drag/flick back to the edge. Cowork is NOT a
/// mode; it's a composer-level toggle inside Home.
struct MainWorkspaceView: View {
    @EnvironmentObject private var modeStore: AppModeStore
    @ObservedObject private var botColors = BotColorStore.shared
    @ObservedObject private var railBackdrop = RailBackdropStore.shared
    @ObservedObject private var computeGate = ComputeStatusStore.shared
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var isSidebarOpen = false
    @State private var dragOffset: CGFloat = 0
    @State private var selectedSessionId: String? = nil
    @State private var screenWidth: CGFloat = 390
    @State private var isOverviewPresented = false
    @State private var artifactSession: ArtifactStudioSession?
    /// A Recents row from another install, while the server says what it opens as.
    @State private var sessionLookup: SessionLookupState? = nil

    /// PhoneShell DRAWER_FRACTION / DRAWER_MAX.
    private var sidebarWidth: CGFloat { min(screenWidth * 0.92, 440) }

    /// Drawer position from 0 (closed) to 1 (open), tracking live drags.
    private var sidebarProgress: CGFloat {
        let base = isSidebarOpen ? sidebarWidth : 0
        return min(max(base + dragOffset, 0), sidebarWidth) / sidebarWidth
    }

    /// Arc spring.smooth: critically damped, never overshoots the edge.
    private var drawerAnimation: Animation {
        reduceMotion ? .linear(duration: 0.12) : .spring(response: 0.38, dampingFraction: 1, blendDuration: 0)
    }

    var body: some View {
        shellBody
            // 409 compute_required from any agent route opens the gate.
            .sheet(isPresented: $computeGate.showGateSheet) { ComputeGateSheet() }
    }

    private var shellBody: some View {
        ZStack(alignment: .leading) {
            // Root backdrop — also the strip behind the status bar / home
            // indicator (the content pane is clipped to the safe area and
            // can't paint up there itself).
            Color("BgPrimary")
                .edgesIgnoringSafeArea(.all)

            VStack(spacing: 0) {
                // `phone-main`: the banner, then the section.
                ConnectComputerView(isShellBanner: true)
                activeSection
                    .environment(\.shellShowsNoComputer, true)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
            .background(Color("BgPrimary"))
            // Hidden from VoiceOver while the menu is over it (useInert).
            .accessibilityHidden(isSidebarOpen)
            // Left-edge pan opens the menu (useDrawerEdgeOpen, 24pt strip).
            .overlay(alignment: .leading) {
                if !isSidebarOpen {
                    Color.clear
                        .frame(width: 24)
                        .contentShape(Rectangle())
                        .ignoresSafeArea()
                        .gesture(drawerGesture)
                }
            }

            // Scrim over the visible strip (Arc contained overlay, 16% black).
            Color.black
                .opacity(0.16 * sidebarProgress)
                .ignoresSafeArea()
                .allowsHitTesting(isSidebarOpen)
                .onTapGesture { setSidebar(false) }
                .gesture(drawerGesture, including: isSidebarOpen ? .all : .none)
                .accessibilityLabel("Close menu")
                .accessibilityAddTraits(.isButton)
                .accessibilityHidden(!isSidebarOpen)

            // The drawer panel: rounded trailing corners, slides from the edge.
            HistorySidebarView(
                selectedSessionId: $selectedSessionId,
                isSidebarOpen: $isSidebarOpen,
                width: sidebarWidth
            )
            .background(
                // In Bots mode the rail wears the colour of the bot on screen
                // (picked in To:, or whose chat or page is open); in Bots and
                // Code it also carries the open screen's background, frosted.
                ZStack {
                    BotWashedBackground(color: .some(modeStore.shellMode == .bots ? botColors.activeHex : nil))
                    if let look = railBackdrop.look, look.mode == modeStore.shellMode {
                        RailBackdropView(look: look)
                    }
                }
                .clipShape(UnevenRoundedRectangle(bottomTrailingRadius: 26, topTrailingRadius: 26, style: .continuous))
                .ignoresSafeArea()
            )
            .shadow(color: .black.opacity(sidebarProgress > 0 ? 0.12 : 0), radius: 16, x: 4, y: 0)
            .offset(x: (sidebarProgress - 1) * (sidebarWidth + 24))
            .simultaneousGesture(drawerGesture, including: isSidebarOpen ? .all : .subviews)
            .accessibilityHidden(!isSidebarOpen)
            .accessibilityAddTraits(.isModal)
            .accessibilityLabel("\(modeStore.shellMode.label) menu")
        }
        .background(
            GeometryReader { proxy in
                Color.clear
                    .onAppear { screenWidth = proxy.size.width }
                    .onChange(of: proxy.size.width) { _, width in screenWidth = width }
            }
        )
        // The waiting games window, over every mode (WaitingGamesLayer).
        .overlay { WaitingGamesLayer() }
        // Pet offers: the bot offers help when a run fails off-screen.
        .overlay { PetOfferBannerLayer() }
        .overlay { sessionLookupCover }
        // Recents and the overviews open an artifact session in its studio,
        // with its editor or pane, not as a plain chat.
        .onChange(of: selectedSessionId) { _, id in
            guard let id else {
                sessionLookup = nil
                return
            }
            openEditorSession(id: id)
        }
        .fullScreenCover(item: $artifactSession) { session in
            ArtifactStudioView(studio: session)
                // Nothing carries over from another session's view.
                .id(session.sessionId)
        }
        .fullScreenCover(isPresented: $isOverviewPresented) {
            SessionOverviewView(
                mode: modeStore.shellMode,
                onOpen: { session in
                    isOverviewPresented = false
                    switch session.originSurface ?? "" {
                    case "code": modeStore.selectBarItem(.code)
                    case "browser": modeStore.selectBarItem(.aci)
                    default: modeStore.selectBarItem(.chats)
                    }
                    selectedSessionId = session.id
                    setSidebar(false)
                },
                onNew: {
                    isOverviewPresented = false
                    selectedSessionId = nil
                    modeStore.selectBarItem(modeStore.shellMode == .code ? .code : modeStore.shellMode == .aci ? .aci : .chats)
                    if modeStore.shellMode == .home || modeStore.shellMode == .bots {
                        NotificationCenter.default.post(name: .shellNewChat, object: nil)
                    }
                    setSidebar(false)
                },
                onMenu: {
                    isOverviewPresented = false
                    setSidebar(true)
                }
            )
            .environmentObject(modeStore)
        }
        .onReceive(NotificationCenter.default.publisher(for: .shellOpenOverview)) { _ in
            Task { await ShellSessionsStore.shared.load() }
            isOverviewPresented = true
        }
        .onReceive(NotificationCenter.default.publisher(for: .shellOpenDrawer)) { _ in
            setSidebar(true)
        }
        // Universal Links + push taps (DeepLinkRouter) and the Meet your bots intro.
        .modifier(DeepLinkHost())
        // Allternit calls: ring polling + the incoming / live call screens.
        .modifier(InappCallHost())
        .modifier(MeetYourBotsHost())
        .onReceive(NotificationCenter.default.publisher(for: .openChatSession)) { note in
            // Bot Home / quick task post this: land on Chats with that
            // session loaded, from whichever section is showing.
            guard let sessionId = note.openChatSessionId else { return }
            selectedSessionId = sessionId
            modeStore.selectBarItem(.chats)
            setSidebar(false)
        }
        .onAppear {
            // Projects feed the cowork top deck and the Projects section;
            // load once per launch (ProjectStore dedupes).
            ProjectStore.shared.fetchProjectsIfNeeded()
            // Usage meter (Phase 5): one fetch per launch; streams refresh
            // it on completion (ChatViewModel.finishStreaming).
            UsageStore.shared.fetchUsageIfNeeded()
            // Response-style preferences: loaded here so the very first
            // send can already inject the directive (plan Phase 6).
            PreferencesStore.shared.fetchIfNeeded()
            if launchArgumentEnabled("sidebar") {
                isSidebarOpen = true
            }
            #if DEBUG
            // `-mode <chat|cowork|code|browser>` (DEBUG only): pre-select a
            // mode for UI testing — launch args land in UserDefaults.
            if let modeArg = UserDefaults.standard.string(forKey: "mode"),
               let mode = AppMode(rawValue: modeArg) {
                modeStore.selectBarItem(ModeBarItem.tab(for: mode))
                modeStore.mode = mode
            }
            // `-shell-mode <home|bots|code|aci>` (DEBUG only): pick the dial mode.
            if let shellArg = UserDefaults.standard.string(forKey: "shell-mode"),
               let shell = ShellMode(rawValue: shellArg) {
                modeStore.setShellMode(shell)
            }
            // `-open-<section>` (DEBUG only): land on a section directly
            // (no tap injection in simctl). `-open-settings` and friends are
            // handled by the drawer, which owns the Settings sheet.
            if launchArgumentEnabled("open-projects") {
                modeStore.selectBarItem(.projects)
            }
            if launchArgumentEnabled("open-artifacts") {
                modeStore.selectBarItem(.artifacts)
            }
            if launchArgumentEnabled("open-automation") {
                modeStore.selectBarItem(.automation)
            }
            if launchArgumentEnabled("open-customize") {
                modeStore.selectBarItem(.customize)
            }
            if launchArgumentEnabled("open-tasks") {
                modeStore.selectBarItem(.tasks)
            }
            if launchArgumentEnabled("open-models") {
                modeStore.selectBarItem(.models)
            }
            if launchArgumentEnabled("open-agent-hub") || launchArgumentEnabled("open-bots") {
                modeStore.selectBarItem(.agents)
            }
            if launchArgumentEnabled("open-aci") {
                modeStore.selectBarItem(.aci)
            }
            // `-open-aci-teach`: ACI with Teach open over the Agent.
            if launchArgumentEnabled("open-aci-teach") {
                modeStore.selectBarItem(.aci)
                ACINavStore.shared.openTeach()
            }
            // `-open-code-filter` (DEBUG only): land on Code — CodeModeView
            // presents the Phase-8 status filter sheet itself.
            if launchArgumentEnabled("open-code-filter") {
                modeStore.selectBarItem(.code)
            }
            #endif
        }
    }

    private enum SessionLookupState: Equatable {
        case loading(String)
        case unlinked(id: String, noun: String)
    }

    /// Opens an artifact session in its studio. One from another install is
    /// looked up behind a cover, so a plain chat doesn't flash first.
    private func openEditorSession(id: String) {
        let launcher = ArtifactStudioLauncher.shared
        if let session = launcher.session(for: id) {
            selectedSessionId = nil
            artifactSession = session
            return
        }
        guard launcher.needsLookup(id: id) else { return }
        sessionLookup = .loading(id)
        Task {
            // A slow server gets the chat rather than a spinner.
            try? await Task.sleep(for: .seconds(6))
            if sessionLookup == .loading(id) { sessionLookup = nil }
        }
        Task {
            let result = await launcher.lookUpSession(id: id)
            guard selectedSessionId == id else { return }
            switch result {
            case .editor(let session):
                sessionLookup = nil
                selectedSessionId = nil
                artifactSession = session
            case .unlinked(let noun):
                sessionLookup = .unlinked(id: id, noun: noun)
            case .chat:
                sessionLookup = nil
            }
        }
    }

    @ViewBuilder
    private var sessionLookupCover: some View {
        switch sessionLookup {
        case .loading?:
            ZStack {
                Color("BgPrimary").ignoresSafeArea()
                ProgressView("Opening…")
            }
        case .unlinked(let id, let noun)?:
            ZStack {
                Color("BgPrimary").ignoresSafeArea()
                VStack(spacing: 8) {
                    FriendlyStateView(
                        style: .empty,
                        icon: "doc.questionmark",
                        title: "Couldn't find this \(noun)",
                        message: "This chat was started on another install before chats were linked to their files. The \(noun) may still be in Artifacts.",
                        actionTitle: "Open as chat",
                        action: {
                            ArtifactStudioLauncher.shared.treatAsChat(id: id)
                            sessionLookup = nil
                        }
                    )
                    Button("Back") {
                        sessionLookup = nil
                        selectedSessionId = nil
                    }
                    .font(.system(size: 15, weight: .medium))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(minHeight: 44)
                }
            }
        case nil:
            EmptyView()
        }
    }

    @ViewBuilder
    private var activeSection: some View {
        switch modeStore.activeTab {
        case .chats:
            ChatView(
                selectedSessionId: $selectedSessionId,
                isSidebarOpen: $isSidebarOpen
            )
        case .projects:
            ProjectsListView(
                onNewChatInProject: { project in
                    // "+ New chat" in a project: select it (the next session
                    // create is stamped via SessionContext.projectId) and
                    // start a fresh Home conversation.
                    ProjectStore.shared.selectedProjectId = project.id
                    selectedSessionId = nil
                    modeStore.selectBarItem(.chats)
                    NotificationCenter.default.post(name: .shellNewChat, object: nil)
                },
                onOpenChat: { session in
                    selectedSessionId = session.id
                    modeStore.selectBarItem(.chats)
                },
                onOpenSidebar: { setSidebar(true) }
            )
        case .artifacts:
            ArtifactsLibraryView(isSidebarOpen: $isSidebarOpen)
        case .agents:
            BotsModeRootView(onOpenSidebar: { setSidebar(true) })
        case .automation:
            AutomationTasksPhoneView(isSidebarOpen: $isSidebarOpen)
        case .customize:
            CustomizeScreen(isSidebarOpen: $isSidebarOpen)
        case .tasks:
            TasksBoardScreen(isSidebarOpen: $isSidebarOpen)
        case .models:
            ModelsTabView(isSidebarOpen: $isSidebarOpen)
        case .code:
            CodeModeView(isSidebarOpen: $isSidebarOpen, selectedSessionId: $selectedSessionId)
        case .aci:
            ACITabView(isSidebarOpen: $isSidebarOpen, selectedSessionId: $selectedSessionId)
        }
    }

    private func setSidebar(_ open: Bool) {
        withAnimation(drawerAnimation) {
            isSidebarOpen = open
            dragOffset = 0
        }
    }

    /// Drawer drag: a pan from the left edge opens (past a quarter of the
    /// panel or a quick flick); dragging an open drawer left closes it past
    /// a third, or on a flick toward the edge, else it springs back.
    private var drawerGesture: some Gesture {
        DragGesture(minimumDistance: 10)
            .onChanged { gesture in
                // Horizontal pans only: vertical scrolls stay with the lists.
                guard abs(gesture.translation.width) > abs(gesture.translation.height) else { return }
                if isSidebarOpen {
                    dragOffset = min(0, gesture.translation.width)
                } else if gesture.translation.width > 0 {
                    dragOffset = gesture.translation.width
                }
            }
            .onEnded { gesture in
                let velocity = gesture.predictedEndTranslation.width - gesture.translation.width
                let open: Bool
                if isSidebarOpen {
                    open = !(-gesture.translation.width > sidebarWidth / 3 || velocity < -120)
                } else {
                    open = gesture.translation.width > sidebarWidth * 0.25 || velocity > 120
                }
                setSidebar(open)
            }
    }
}

// MARK: - Mode placeholders (UX-4 fills these)

/// Intentional placeholder for surfaces not yet built on iOS — centered
/// mode icon + label, tinted with the mode accent.
struct ModePlaceholderView: View {
    let mode: AppMode

    var body: some View {
        FriendlyStateView(
            style: .empty,
            icon: mode.theme.icon,
            title: "\(mode.label) mode — coming in UX-4",
            message: "This surface isn't part of the iOS app yet."
        )
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color("BgSecondary"))
    }
}

