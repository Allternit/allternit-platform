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
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var isSidebarOpen = false
    @State private var dragOffset: CGFloat = 0
    @State private var selectedSessionId: String? = nil
    @State private var screenWidth: CGFloat = 390
    @State private var isOverviewPresented = false

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
                UnevenRoundedRectangle(bottomTrailingRadius: 26, topTrailingRadius: 26, style: .continuous)
                    .fill(Color("BgPrimary"))
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

