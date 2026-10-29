import SwiftUI

// MARK: - Mode host shell

/// Top-level shell (the PWA phone shell, allternit-ai
/// src/fabric-session/phone/PhoneShell.tsx): one section fills the screen,
/// no bottom bar. The left drawer (HistorySidebarView, 92% wide) carries the
/// mode dial — Home · Bots · Code · ACI — plus the mode's rail, pins and
/// recents. Cowork is NOT a mode; it's a composer-level toggle inside Home.
struct MainWorkspaceView: View {
    @EnvironmentObject private var modeStore: AppModeStore
    @State private var isSidebarOpen = false
    @State private var dragOffset: CGFloat = 0
    @State private var selectedSessionId: String? = nil
    @State private var screenWidth: CGFloat = 390

    /// The drawer covers 92% of the screen (PhoneShell's Arc drawer width).
    private var sidebarWidth: CGFloat { max(screenWidth * 0.92, 260) }

    /// Drawer position from 0 (closed) to 1 (open), tracking live drags.
    private var sidebarProgress: CGFloat {
        let base = isSidebarOpen ? sidebarWidth : 0
        return min(max(base + dragOffset, 0), sidebarWidth) / sidebarWidth
    }

    /// Chats paints BgSecondary edge to edge; the other sections lead with
    /// BgPrimary chrome.
    private var paneBackground: Color {
        Color(modeStore.activeTab == .chats ? "BgSecondary" : "BgPrimary")
    }

    var body: some View {
        ZStack {
            // Root backdrop — also the strip behind the status bar / home
            // indicator (the content pane is clipped to the safe area and
            // can't paint up there itself).
            paneBackground
                .edgesIgnoringSafeArea(.all)

            ZStack(alignment: .leading) {
                // The drawer — full-height, edge-to-edge behind the pane.
                HistorySidebarView(
                    selectedSessionId: $selectedSessionId,
                    isSidebarOpen: $isSidebarOpen,
                    width: sidebarWidth
                )

                activeSection
                // Opaque backdrop for the whole pane, bleeding edge-to-edge
                // (it fully hides the drawer behind). Sliding right reveals
                // the drawer; the pane stays full-size.
                .background(paneBackground)
                // Rounded pane while the drawer is showing: a full-screen
                // mask (NOT .cornerRadius, which clips at the safe-area
                // bounds) so the corners round from the physical screen edges.
                .mask(
                    RoundedRectangle(cornerRadius: 28 * sidebarProgress, style: .continuous)
                        .ignoresSafeArea()
                )
                .offset(x: sidebarProgress * sidebarWidth)
                .shadow(color: Color.black.opacity(0.4 * sidebarProgress), radius: 10, x: -5, y: 0)
                .animation(.spring(response: 0.35, dampingFraction: 0.86, blendDuration: 0), value: isSidebarOpen)
                // Pane-wide drag only while the drawer is open (to drag it
                // closed). When closed, the gesture gets the LOWEST
                // precedence (.none) so every ScrollView inside wins its
                // drags — opening is handled by the left-edge strip below.
                .gesture(drawerGesture, including: isSidebarOpen ? .all : .none)
                .overlay(alignment: .leading) {
                    if isSidebarOpen {
                        // The visible 8% sliver of the pane: tap to close.
                        Color.clear
                            .contentShape(Rectangle())
                            .ignoresSafeArea()
                            .onTapGesture {
                                withAnimation(.spring(response: 0.35, dampingFraction: 0.86, blendDuration: 0)) {
                                    isSidebarOpen = false
                                }
                            }
                            .gesture(drawerGesture)
                            .accessibilityLabel("Close menu")
                            .accessibilityAddTraits(.isButton)
                    } else {
                        Color.clear
                            .frame(width: 28)
                            .contentShape(Rectangle())
                            .ignoresSafeArea()
                            .gesture(drawerGesture)
                    }
                }
            }
        }
        .background(
            GeometryReader { proxy in
                Color.clear
                    .onAppear { screenWidth = proxy.size.width }
                    .onChange(of: proxy.size.width) { _, width in screenWidth = width }
            }
        )
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
            withAnimation(.spring(response: 0.35, dampingFraction: 0.86, blendDuration: 0)) {
                isSidebarOpen = false
            }
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
            if launchArgumentEnabled("open-models") {
                modeStore.selectBarItem(.models)
            }
            if launchArgumentEnabled("open-agent-hub") || launchArgumentEnabled("open-bots") {
                modeStore.selectBarItem(.agents)
            }
            if launchArgumentEnabled("open-aci") {
                modeStore.selectBarItem(.aci)
            }
            // `-open-code-filter` (DEBUG only): land on Code — CodeModeView
            // presents the Phase-8 status filter sheet itself.
            if launchArgumentEnabled("open-code-filter") {
                modeStore.selectBarItem(.code)
            }
            #endif
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
                onOpenSidebar: {
                    withAnimation(.spring(response: 0.35, dampingFraction: 0.86, blendDuration: 0)) {
                        isSidebarOpen = true
                    }
                }
            )
        case .artifacts:
            ArtifactsLibraryView(isSidebarOpen: $isSidebarOpen)
        case .agents:
            BotsModeRootView(onOpenSidebar: {
                withAnimation(.spring(response: 0.35, dampingFraction: 0.86, blendDuration: 0)) {
                    isSidebarOpen = true
                }
            })
        case .automation:
            switch modeStore.automationKind {
            case .overview:
                AutomationOverviewRoot(isSidebarOpen: $isSidebarOpen)
            case .cron:
                AutomationTasksListView(isSidebarOpen: $isSidebarOpen)
            case .routines:
                RoutinesListView(isSidebarOpen: $isSidebarOpen)
            case .loops:
                LoopsListView(isSidebarOpen: $isSidebarOpen)
            }
        case .models:
            ModelsTabView(isSidebarOpen: $isSidebarOpen)
        case .code:
            CodeModeView(isSidebarOpen: $isSidebarOpen, selectedSessionId: $selectedSessionId)
        case .aci:
            ACITabView(isSidebarOpen: $isSidebarOpen, selectedSessionId: $selectedSessionId)
        }
    }

    /// Drawer drag: swiping right from the left edge opens, dragging left
    /// on an open drawer closes.
    private var drawerGesture: some Gesture {
        DragGesture()
            .onChanged { gesture in
                if isSidebarOpen {
                    // Dragging left pushes the open drawer closed.
                    if gesture.translation.width < 0 {
                        dragOffset = gesture.translation.width
                    }
                } else if gesture.startLocation.x < 50 && gesture.translation.width > 0 {
                    // Swiping right from the left margin exposes the drawer.
                    dragOffset = gesture.translation.width
                }
            }
            .onEnded { gesture in
                let base = isSidebarOpen ? sidebarWidth : 0
                let projected = base + gesture.predictedEndTranslation.width
                withAnimation(.spring(response: 0.35, dampingFraction: 0.86, blendDuration: 0)) {
                    isSidebarOpen = projected > sidebarWidth / 2
                    dragOffset = 0
                }
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

