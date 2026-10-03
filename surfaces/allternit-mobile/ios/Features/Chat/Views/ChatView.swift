import SwiftUI

// MARK: - Mode host shell

/// Top-level tab host: a per-tab content area, no persistent bottom bar —
/// neither ChatGPT nor Claude's iOS apps put a surface switcher there, so
/// the [Chats | Projects | Artifacts Library | Agents | Automation Tasks |
/// Code | ACI] tab list lives in the sidebar header instead
/// (HistorySidebarView). Cowork is NOT a tab destination; it's a
/// composer-level toggle inside Chats (BottomDock.tsx ChatCoworkToggle).
struct MainWorkspaceView: View {
    @EnvironmentObject private var modeStore: AppModeStore
    @State private var isSidebarOpen = false
    @State private var dragOffset: CGFloat = 0
    @State private var selectedSessionId: String? = nil

    private let sidebarWidth: CGFloat = 280

    /// Drawer position from 0 (closed) to 1 (open), tracking live drags.
    private var sidebarProgress: CGFloat {
        let base = isSidebarOpen ? sidebarWidth : 0
        return min(max(base + dragOffset, 0), sidebarWidth) / sidebarWidth
    }

    var body: some View {
        ZStack {
            // Root backdrop — also the strip behind the status bar / home
            // indicator (the content pane is clipped to the safe area and
            // can't paint up there itself), so it tracks the active
            // surface's top color: Chats is the BgSecondary feed edge-to-
            // edge, the other tabs lead with BgPrimary chrome.
            Color(modeStore.activeTab == .chats ? "BgSecondary" : "BgPrimary")
                .edgesIgnoringSafeArea(.all)

            ZStack(alignment: .leading) {
                // Left Sidebar Drawer (mode-aware unified sidebar) —
                // full-height, edge-to-edge behind the content pane.
                HistorySidebarView(
                    selectedSessionId: $selectedSessionId,
                    isSidebarOpen: $isSidebarOpen
                )

                    // Active content container
                    Group {
                        switch modeStore.activeTab {
                        case .chats:
                            ChatView(
                                selectedSessionId: $selectedSessionId,
                                isSidebarOpen: $isSidebarOpen
                            )
                        case .projects:
                            ProjectsListView(
                                onNewChatInProject: { project in
                                    // "+ New chat" in a project: select it
                                    // (the next session create is stamped via
                                    // SessionContext.projectId) and start a
                                    // fresh Chats conversation.
                                    ProjectStore.shared.selectedProjectId = project.id
                                    selectedSessionId = nil
                                    modeStore.selectBarItem(.chats)
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
                            AgentHubView(isSidebarOpen: $isSidebarOpen)
                        case .automation:
                            switch modeStore.automationKind {
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
                // Opaque backdrop for the whole pane, bleeding edge-to-edge
                // (it fully hides the sidebar behind and lets translucent
                // hairlines inside the surfaces composite against it, not
                // the sidebar — the accent tab row once showed through as a
                // red sliver). Sliding right reveals the full-height drawer;
                // the pane stays full-size — no scale-down card effect.
                .background(Color(modeStore.activeTab == .chats ? "BgSecondary" : "BgPrimary"))
                // Rounded pane while the drawer is showing: a full-screen
                // mask (NOT .cornerRadius, which clips at the safe-area
                // bounds and would cut the edge-to-edge backdrop) so the
                // corners round from the physical screen edges as it slides.
                .mask(
                    RoundedRectangle(cornerRadius: 28 * sidebarProgress, style: .continuous)
                        .ignoresSafeArea()
                )
                .offset(x: sidebarProgress * sidebarWidth)
                .shadow(color: Color.black.opacity(0.4 * sidebarProgress), radius: 10, x: -5, y: 0)
                .animation(.spring(response: 0.35, dampingFraction: 0.86, blendDuration: 0), value: isSidebarOpen)
                // Pane-wide drag only while the drawer is open (to drag it
                // closed). When closed, the gesture gets the LOWEST
                // precedence (.none) so every ScrollView inside (agent deck
                // templates, composer strips) wins its drags — opening is
                // handled by the left-edge strip below instead.
                .gesture(drawerGesture, including: isSidebarOpen ? .all : .none)
                .overlay(alignment: .leading) {
                    if !isSidebarOpen {
                        Color.clear
                            .frame(width: 28)
                            .contentShape(Rectangle())
                            .ignoresSafeArea()
                            .gesture(drawerGesture)
                    }
                }
            }
        }
        .onAppear {
            // Projects feed the cowork top deck and the Projects tab; load
            // once per launch (ProjectStore dedupes).
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
                modeStore.mode = mode
                modeStore.activeTab = ModeBarItem.tab(for: mode)
            }
            // `-open-projects` / `-open-artifacts` (DEBUG only): land on a
            // tab surface directly (no tap injection in simctl).
            if launchArgumentEnabled("open-projects") {
                modeStore.selectBarItem(.projects)
            }
            if launchArgumentEnabled("open-artifacts") {
                modeStore.selectBarItem(.artifacts)
            }
            // `-open-agent-hub` (DEBUG only): land on the Agents tab
            // directly (no tap injection in simctl).
            if launchArgumentEnabled("open-agent-hub") {
                modeStore.selectBarItem(.agents)
            }
            // `-open-code-filter` (DEBUG only): land on the Code tab —
            // CodeModeView presents the Phase-8 status filter sheet itself.
            if launchArgumentEnabled("open-code-filter") {
                modeStore.selectBarItem(.code)
            }
            #endif
        }
    }

    /// Drawer drag: swiping right from the left margin opens, dragging left
    /// on an open drawer closes. Home-only — code/ACI have no drawer.
    private var drawerGesture: some Gesture {
        DragGesture()
            .onChanged { gesture in
                if isSidebarOpen {
                    // Dragging left pushes the open drawer closed.
                    if gesture.translation.width < 0 {
                        dragOffset = gesture.translation.width
                    }
                } else if gesture.startLocation.x < 50 && gesture.translation.width > 0 {
                    // Swiping right from the left margin exposes the sidebar.
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

// MARK: - Chat surface (Home)

/// Home chrome: the sidebar toggle + new-chat header above the shared chat
/// content. The Code tab hosts the same `ChatContentView` inside its own
/// navigation chrome (CodeModeView), so code threads reuse the chat UI
/// instead of duplicating it.
struct ChatView: View {
    @Binding var selectedSessionId: String?
    @Binding var isSidebarOpen: Bool

    @EnvironmentObject private var modeStore: AppModeStore
    @StateObject private var viewModel = ChatViewModel()
    /// Intelli-Schedule panel sheet — opened from the floating chrome in Cowork mode.
    @State private var isIntelliSchedulePresented = false

    /// Incognito affordance glyph. "ghost" (Claude parity) only exists in
    /// newer SF Symbols — on iOS 18 it renders blank, so fall back to
    /// "eye.slash" when the name doesn't resolve.
    fileprivate static let incognitoSymbolName: String =
        UIImage(systemName: "ghost") != nil ? "ghost" : "eye.slash"

    /// A conversation is on screen (opened from history or created by
    /// sending) — the back button returns to the main empty-chat screen.
    private var isInChat: Bool {
        selectedSessionId != nil || viewModel.currentSessionId != nil || !viewModel.messages.isEmpty
    }

    var body: some View {
        ZStack(alignment: .top) {
            ChatContentView(
                sessionId: selectedSessionId,
                viewModel: viewModel,
                // Extra room for the "Temporary chat" caption pill under
                // the icon row while the toggle is on.
                topContentInset: viewModel.isTemporaryChat ? 88 : 52
            )

            // Floating chrome: circular icon buttons hovering over the feed —
            // no header bar, no separator; content scrolls beneath them.
            VStack(spacing: 8) {
                HStack(spacing: 8) {
                    if isInChat {
                        floatingIcon("chevron.left", accessibilityLabel: "Back") {
                            selectedSessionId = nil
                            viewModel.startNewSession()
                            viewModel.isTemporaryChat = false
                        }
                    }

                    floatingIcon("line.3.horizontal", accessibilityLabel: "Open sidebar") {
                        isSidebarOpen.toggle()
                    }

                    Spacer()

                    // Incognito chat (Phase 6, Claude parity): starts an
                    // ephemeral session stamped `metadata.ephemeral` —
                    // excluded from history, purged on abort server-side.
                    floatingIcon(Self.incognitoSymbolName, isActive: viewModel.isIncognito, accessibilityLabel: "Incognito chat") {
                        selectedSessionId = nil
                        viewModel.startNewSession(ephemeral: true)
                    }

                    // Intelli-Schedule panel — only offered in Cowork mode.
                    if modeStore.mode == .cowork {
                        floatingIcon("calendar.badge.clock", accessibilityLabel: "Open Intelli-Schedule") {
                            isIntelliSchedulePresented = true
                        }
                    }
                }

            }
            .padding(.horizontal, 12)
            .padding(.top, 6)

            // Incognito mode title (Claude parity): small, top-center,
            // non-interactive — the floating icon row owns the corners.
            if viewModel.isIncognito {
                Text("Incognito chat")
                    .font(.subheadline.weight(.medium))
                    .foregroundColor(Color("TextSecondary"))
                    .padding(.top, 18)
                    .allowsHitTesting(false)
            }
        }
        .background(Color("BgSecondary"))
        .sheet(isPresented: $isIntelliSchedulePresented) {
            IntelliSchedulePanel()
        }
        #if DEBUG
        // `-temporary-chat` (DEBUG only): start in temporary mode for
        // screenshot verification (no tap injection in simctl).
        // `-open-incognito` (DEBUG only): start an incognito chat on launch
        // so the explainer empty state can be screenshot-verified.
        .onAppear {
            if launchArgumentEnabled("temporary-chat"), !viewModel.isTemporaryChat {
                viewModel.toggleTemporaryChat()
            }
            if launchArgumentEnabled("open-incognito"), !viewModel.isIncognito {
                viewModel.startNewSession(ephemeral: true)
            }
        }
        #endif
    }

    private func floatingIcon(
        _ systemName: String,
        isActive: Bool = false,
        accessibilityLabel: String? = nil,
        action: @escaping () -> Void
    ) -> some View {
        Button(action: {
            let generator = UIImpactFeedbackGenerator(style: .medium)
            generator.impactOccurred()
            action()
        }) {
            Image(systemName: systemName)
                .font(.system(size: 16, weight: .semibold))
                .foregroundColor(isActive ? Color("AccentPrimary") : Color("TextPrimary"))
                .frame(width: 40, height: 40)
                // Apple glass look: frosted blur material with a light
                // rim catching the "light" on top and a soft drop shadow —
                // content scrolling beneath blurs through. Active toggles
                // tint the glyph and rim with the accent.
                .background(.ultraThinMaterial, in: Circle())
                .overlay(
                    Circle()
                        .strokeBorder(
                            isActive
                                ? LinearGradient(
                                    colors: [Color("AccentPrimary").opacity(0.7), Color("AccentPrimary").opacity(0.25)],
                                    startPoint: .top,
                                    endPoint: .bottom
                                )
                                : LinearGradient(
                                    colors: [Color.white.opacity(0.6), Color.white.opacity(0.08)],
                                    startPoint: .top,
                                    endPoint: .bottom
                                ),
                            lineWidth: 1
                        )
                )
                .shadow(color: isActive ? Color("AccentPrimary").opacity(0.25) : Color.black.opacity(0.10), radius: 10, y: 3)
        }
        .accessibilityLabel(accessibilityLabel ?? "")
    }
}

// MARK: - Shared chat content

/// The chat experience itself — error banner, message feed, composer + decks
/// — with no host chrome. The host owns the `ChatViewModel` (as @StateObject)
/// so its chrome buttons can drive it, and supplies the session id to load.
/// Session-create context derives from the current app mode, so hosting this
/// under `.code` stamps new sessions with `origin_surface='code'`.
struct ChatContentView: View {
    let sessionId: String?
    @ObservedObject var viewModel: ChatViewModel
    /// Extra top scroll margin for hosts whose chrome FLOATS over the feed
    /// (ChatView's circular icons) so the first row starts below the icons;
    /// hosts with a real bar (CodeThreadChatView's nav chrome) keep 0.
    var topContentInset: CGFloat = 0

    @EnvironmentObject private var modeStore: AppModeStore
    @EnvironmentObject private var agentModeStore: AgentModeStore
    @StateObject private var modelStore = ModelStore.shared
    /// Selected cowork project — stamped into the next session's create
    /// metadata (`metadata.projectId`) so the Projects feature can group it.
    @StateObject private var projectStore = ProjectStore.shared
    /// Staged composer attachments — owned here (next to the view model that
    /// uploads them on send) and handed to the composer for the pickers
    /// and thumbnail strip.
    @StateObject private var attachmentStore = AttachmentStore()
    /// Weekly usage meter (Phase 5) — drives the ≥80% banner and the 100%
    /// composer wall.
    @StateObject private var usageStore = UsageStore.shared
    @State private var inputText: String = ""
    /// Full voice-mode takeover (Phase 7b), presented from the composer's
    /// waveform button (or the `-open-voice-mode` DEBUG arg).
    @StateObject private var voiceHost = VoiceHost()
    /// Phase 8 edit-resend: id of the last user bubble whose "Edit" filled
    /// the composer; non-nil routes the next send through
    /// `ChatViewModel.resendEditedMessage` (truncate + re-send).
    @State private var editingMessageId: String? = nil
    @State private var activeArtifact: ArtifactRecord? = nil
    @State private var isModelPickerPresented = false
    /// Upgrade/credits links from the usage banner and wall open in
    /// SFSafariViewController.
    @State private var usageSafariURL: IdentifiableURL? = nil
    /// The incognito explainer's "Learn more" link (Phase 6) — same
    /// SFSafariViewController presentation as the usage links.
    @State private var incognitoSafariURL: IdentifiableURL? = nil
    /// One-time "Turn On Response Notifications" card (Claude parity) —
    /// the X flips this flag forever.
    @State private var isNotificationsCardDismissed =
        UserDefaults.standard.bool(forKey: Self.notificationsCardDismissedKey)
    /// App-owned notification priming sheet, shown from the card's Continue
    /// before the system prompt (once per install via
    /// AppPermission.notifications).
    @State private var isNotificationPrimingPresented = false
    /// Cowork workspace launchpad sheet — opened from the composer toggle.
    @State private var isCoworkWorkspacePresented = false
    @Environment(\.scenePhase) private var scenePhase

    private static let notificationsCardDismissedKey = "allternit-notifications-card-dismissed"

    /// The notifications opt-in card shows once, at the top of an
    /// empty/new chat feed. `-open-notifications-card` (DEBUG only) forces
    /// it visible for screenshots regardless of the dismissed flag.
    private var showsNotificationsCard: Bool {
        guard viewModel.messages.isEmpty else { return false }
        // The incognito empty state leads with its own explainer (Phase 6),
        // and the code terminal keeps its boot header clean.
        guard !viewModel.isIncognito, !isTerminal else { return false }
        #if DEBUG
        if launchArgumentEnabled("open-notifications-card") {
            return true
        }
        #endif
        return !isNotificationsCardDismissed
    }

    /// The mode + agent state the NEXT session create should be stamped
    /// with. Composer-level controls only apply pre-session, so pushing
    /// this into the view model can never mutate a live session.
    private var sessionContext: SessionContext {
        let mode = modeStore.mode
        let agentOn = agentModeStore.isAgentEnabled(for: mode)
        return SessionContext(
            originSurface: mode.originSurface,
            sessionMode: agentModeStore.sessionMode(for: mode),
            agentId: agentOn ? agentModeStore.selectedAgentId(for: mode) : nil,
            agentName: agentOn ? agentModeStore.selectedAgent(for: mode)?.name : nil,
            agentModeId: agentOn ? agentModeStore.selectedTile(for: mode).rawValue : nil,
            projectId: projectStore.selectedProjectId,
            ephemeral: viewModel.isIncognito,
            // Phase 10: the onboarding work-profile answer rides along as
            // `metadata.persona` on the next session create.
            persona: OnboardingStore.shared.persona?.rawValue
        )
    }

    /// Model for the next send: the user's manual picker choice always
    /// wins; a selected agent defers to the SERVER (the bridge applies the
    /// agent's own model when runtimeModelId is absent); otherwise the
    /// persisted/auto-selected model goes out as before.
    private var modelForSend: String? {
        if modelStore.didManuallySelectModel { return modelStore.selectedModelId }
        return sessionContext.agentId == nil ? modelStore.selectedModelId : nil
    }

    private var agentOn: Bool { agentModeStore.isAgentEnabled(for: modeStore.mode) }

    /// Code threads render as a terminal session (dark, monospace, `❯`
    /// prompts) instead of chat bubbles.
    private var isTerminal: Bool { modeStore.mode == .code }

    /// One feed row — extracted from `body` so the view stays under the
    /// type-checker's expression budget (the Phase-8 action-bar / edit
    /// closures pushed the feed's ForEach over it).
    private func messageRow(_ message: MessageRecord) -> some View {
        MessageRow(
            message: message,
            onArtifactTap: { artifact in
                activeArtifact = artifact
            },
            onChooseModel: {
                isModelPickerPresented = true
            },
            onRetry: { failedMessageId in
                viewModel.retryFailedMessage(failedMessageId, runtimeModelId: modelStore.selectedModelId, effort: modelStore.effortForSend)
            },
            isLastAssistant: message.id == viewModel.lastAssistantMessageId,
            isLastUser: message.id == viewModel.lastUserMessageId,
            onRegenerate: {
                viewModel.regenerateLastResponse(runtimeModelId: modelStore.selectedModelId, effort: modelStore.effortForSend)
            },
            onEdit: {
                // "Edit" on the last user bubble: the composer loads the
                // text; sending truncates + re-sends (onSend below).
                editingMessageId = message.id
                inputText = message.content
            }
        )
        .id(message.id)
    }

    /// Terminal-styled feed row for code threads — see TerminalMessageRow.
    private func terminalRow(_ message: MessageRecord) -> some View {
        TerminalMessageRow(
            message: message,
            isStreamingTail: viewModel.isStreaming && message.id == viewModel.messages.last?.id
        )
        .id(message.id)
    }

    /// The "editing" strip above the composer (Phase 8) — extracted with
    /// `messageRow` for the same type-checker budget reason.
    private var editingBanner: some View {
        HStack(spacing: 8) {
            Image(systemName: "pencil")
                .font(.caption)
            Text("Editing message — sending replaces everything after it")
                .font(.caption)
                .lineLimit(1)
            Spacer()
            Button(action: {
                editingMessageId = nil
                inputText = ""
            }) {
                Image(systemName: "xmark")
                    .font(.caption)
            }
        }
        .foregroundColor(Color("TextSecondary"))
        .padding(.horizontal, 16)
        .padding(.vertical, 6)
        .background(Color("BgSecondary"))
    }

    var body: some View {
        VStack(spacing: 0) {
            // Transient load/send failure banner (dismissible). Pushed below
            // any floating host chrome so the icons never cover its text.
            if let transientError = viewModel.transientError {
                TransientErrorBanner(message: transientError, onDismiss: {
                    viewModel.transientError = nil
                })
                .padding(.top, topContentInset)
            }

            // ≥80% weekly-usage nudge (Phase 5) — below the error banner,
            // dismissed for the day on X; at 100% the wall card in the feed
            // takes over instead.
            if usageStore.shouldShowBanner, let percentText = usageStore.percentText {
                UsageLimitBanner(
                    percentText: percentText,
                    resetsLabel: usageStore.resetsLabel,
                    onUpgrade: {
                        // Placeholder — the real upgrade flow is TBD.
                        usageSafariURL = IdentifiableURL(url: URL(string: "https://allternit.com/upgrade")!)
                    },
                    onDismiss: {
                        usageStore.dismissBannerForToday()
                    }
                )
                .padding(.top, viewModel.transientError == nil ? topContentInset : 0)
            }

            // Conversation Scroll Area
            ScrollViewReader { scrollProxy in
                ScrollView {
                    LazyVStack(spacing: 20) {
                        if viewModel.messages.isEmpty {
                            // One-time notifications opt-in (Claude parity)
                            // at the top of the empty feed.
                            if showsNotificationsCard {
                                ResponseNotificationsCard(
                                    onContinue: {
                                        isNotificationsCardDismissed = true
                                        UserDefaults.standard.set(true, forKey: Self.notificationsCardDismissedKey)
                                        isNotificationPrimingPresented = true
                                    },
                                    onDismiss: {
                                        isNotificationsCardDismissed = true
                                        UserDefaults.standard.set(true, forKey: Self.notificationsCardDismissedKey)
                                    }
                                )
                                .padding(.horizontal, 12)
                            }
                            // The usage wall replaces the greeting on an
                            // empty locked chat — showing both pushes the
                            // wall's buttons below the fold.
                            if !usageStore.isAtLimit {
                                if viewModel.isIncognito {
                                    // Incognito empty state (Phase 6, Claude
                                    // parity): ghost glyph + privacy explainer
                                    // instead of the greeting.
                                    IncognitoEmptyStateView(onLearnMore: {
                                        // Placeholder URL until the real
                                        // privacy page exists.
                                        incognitoSafariURL = IdentifiableURL(url: URL(string: "https://allternit.com/privacy")!)
                                    })
                                    .padding(.top, 60)
                                } else if isTerminal {
                                    // Code thread empty state: terminal boot
                                    // header, not the Home greeting.
                                    TerminalEmptyState()
                                        .padding(.top, 48)
                                } else {
                                    // Empty chat keeps the centered wordmark;
                                    // suggestion rows removed to keep the feed
                                    // uncluttered — greeting/suggested-prompt
                                    // chips are the one exception, and only
                                    // when a specific agent is explicitly
                                    // selected (not the default backend
                                    // agent), per Eoj.
                                    EmptyChatStateView()
                                        .padding(.top, 60)
                                    if agentOn, let agentId = sessionContext.agentId,
                                       let agent = AgentHubStore.shared.agents.first(where: { $0.id == agentId }),
                                       agent.greeting != nil || !agent.suggestedPrompts.isEmpty {
                                        AgentGreetingView(agent: agent) { prompt in
                                            inputText = prompt
                                        }
                                        .padding(.top, 16)
                                        .padding(.horizontal, 16)
                                    }
                                }
                            }
                        } else {
                            ForEach(viewModel.messages) { message in
                                if isTerminal {
                                    terminalRow(message)
                                } else {
                                    messageRow(message)
                                }
                            }
                        }

                        // Hard usage wall (Phase 5): at/above 100% the
                        // composer locks and this card offers the way out.
                        if usageStore.isAtLimit {
                            UsageWallCard(
                                resetsLabel: usageStore.resetsLabel,
                                onAddCredits: {
                                    // Placeholder — the real credits
                                    // purchase flow is TBD.
                                    usageSafariURL = IdentifiableURL(url: URL(string: "https://allternit.com/credits")!)
                                },
                                onGetPro: {
                                    // Placeholder — the real upgrade flow
                                    // is TBD.
                                    usageSafariURL = IdentifiableURL(url: URL(string: "https://allternit.com/upgrade")!)
                                }
                            )
                            .padding(.horizontal, 12)
                        }
                    }
                    .padding(.vertical, 20)
                }
                .background(Color("BgSecondary"))
                .contentMargins(.top, (viewModel.transientError == nil && !usageStore.shouldShowBanner) ? topContentInset : 0, for: .scrollContent)
                .scrollDismissesKeyboard(.interactively)
                .onChange(of: viewModel.messages.last) { _, _ in
                    // Fires on new messages AND on streamed content changes;
                    // the view model's ~50ms flush coalescing bounds the rate.
                    if let lastId = viewModel.messages.last?.id {
                        // Animated scroll lags behind rapid streaming deltas,
                        // so use an instant scroll while a stream is in flight.
                        if viewModel.isStreaming {
                            scrollProxy.scrollTo(lastId, anchor: .bottom)
                        } else {
                            withAnimation {
                                scrollProxy.scrollTo(lastId, anchor: .bottom)
                            }
                        }
                    }
                }
            }

            // Editing indicator (Phase 8): the last user bubble's "Edit"
            // loaded its text into the composer — sending truncates the
            // conversation after it and re-sends; X cancels back to a
            // normal draft.
            if editingMessageId != nil {
                editingBanner
            }

            // Quick voice bar: orb + status + mute / expand / end while a
            // voice conversation runs (full-screen mode is the expand button).
            if voiceHost.presentation == .bar, let voiceModel = voiceHost.viewModel {
                VoiceQuickBar(
                    viewModel: voiceModel,
                    onExpand: { voiceHost.presentation = .full },
                    onEnd: {
                        if let seconds = voiceHost.end() {
                            viewModel.appendVoiceSummary(durationSeconds: seconds)
                        }
                    }
                )
                .transition(.move(edge: .bottom).combined(with: .opacity))
            }

            // Composer card + decks (top deck in cowork, bottom deck when
            // the agent pill is on) — floating over the feed background,
            // no separator; the card's own border defines its edge.
            ComposerView(
                inputText: $inputText,
                attachmentStore: attachmentStore,
                isStreaming: viewModel.isStreaming,
                isSendDisabled: viewModel.isCreatingSession,
                hasActiveSession: viewModel.currentSessionId != nil,
                // Hard usage wall: at/above 100% the composer locks until
                // the window resets or the user adds credits / upgrades.
                isUsageLocked: usageStore.isAtLimit,
                isVoiceModeEnabled: !viewModel.isStreaming && !usageStore.isAtLimit,
                onSend: { attachments in
                    if let editingMessageId {
                        self.editingMessageId = nil
                        viewModel.resendEditedMessage(editingMessageId, newText: inputText, attachments: attachments, runtimeModelId: modelForSend, effort: modelStore.effortForSend)
                    } else {
                        viewModel.sendMessage(inputText, attachments: attachments, runtimeModelId: modelForSend, effort: modelStore.effortForSend)
                    }
                    inputText = ""
                },
                onStop: {
                    viewModel.stopStreaming()
                },
                onDictationError: { message in
                    viewModel.transientError = message
                },
                onVoiceMode: {
                    voiceHost.start(chat: viewModel, runtimeModelId: modelStore.selectedModelId,
                                    effort: modelStore.effortForSend, presentation: .bar)
                },
                onAgentModeChanged: { isOn in
                    if isOn, viewModel.currentSessionId == nil, viewModel.messages.isEmpty {
                        // Mount a fresh bot session context when there is no
                        // active conversation so the first message creates a
                        // session bound to the selected/default bot.
                        viewModel.startNewSession()
                    }
                }
            )
            .background(Color("BgSecondary"))
        }
        .animation(.easeOut(duration: 0.2), value: voiceHost.presentation)
        .voiceHostWiring(voiceHost, chat: viewModel)
        .fullScreenCover(isPresented: Binding(
            get: { voiceHost.presentation == .full && voiceHost.viewModel != nil },
            set: { if !$0, voiceHost.presentation == .full { voiceHost.presentation = voiceHost.isActive ? .bar : .none } }
        )) {
            if let voiceModel = voiceHost.viewModel {
                VoiceModeView(host: voiceHost, viewModel: voiceModel, onEnd: { durationSeconds in
                    // Files the "Voice chat ended · Ns" card into the feed; the
                    // conversation itself is already in the thread (it went
                    // through sendMessage).
                    viewModel.appendVoiceSummary(durationSeconds: durationSeconds)
                })
            }
        }
        .onDisappear {
            // Leaving the thread ends the call (the engine owns the mic) — but
            // not when the full-screen cover is what covered this view.
            if voiceHost.presentation != .full { voiceHost.end() }
        }
        .sheet(item: $activeArtifact) { artifact in
            ArtifactDetailsView(artifact: artifact)
        }
        .sheet(isPresented: $isModelPickerPresented) {
            ModelPickerSheet(modelStore: modelStore)
        }
        .sheet(item: $usageSafariURL) { item in
            SafariView(url: item.url)
        }
        .sheet(item: $incognitoSafariURL) { item in
            SafariView(url: item.url)
        }
        .sheet(isPresented: $isNotificationPrimingPresented) {
            PermissionPrimingSheet(permission: .notifications) {
                Task { _ = await NotificationService.requestAuthorization() }
            }
        }
        .onAppear {
            viewModel.sessionContext = sessionContext
            if let sessionId = sessionId {
                viewModel.loadSession(sessionId)
            }
            // Phase 10: an onboarding starter-task card stashes its prompt
            // in OnboardingStore.pendingPrompt — fill the composer with it
            // ONCE (fill-not-send, same contract as the suggestion rows).
            if let prompt = OnboardingStore.shared.pendingPrompt {
                inputText = prompt
                OnboardingStore.shared.pendingPrompt = nil
            }
            #if DEBUG
            // `-stage-test-attachment` (DEBUG only): stages a generated
            // 400x300 colored image so the composer thumbnail strip and the
            // attachment send path can be screenshot-verified without
            // tap-injection or photo-library fixtures. Runs before the
            // `-autosend` branch below so combining both args exercises the
            // upload path end-to-end.
            if launchArgumentEnabled("stage-test-attachment") {
                stageTestAttachment()
            }
            // `-open-voice-mode` / `-open-voice-settings` (DEBUG only):
            // presents the Phase 7b voice-mode takeover (idle) — the
            // settings variant lands on the voice settings sheet. Combine
            // with `-voice-state listening|thinking|speaking` to pin a
            // gradient state for screenshots.
            if launchArgumentEnabled("open-voice-mode")
                || launchArgumentEnabled("open-voice-settings") {
                voiceHost.start(chat: viewModel, runtimeModelId: modelStore.selectedModelId,
                                effort: modelStore.effortForSend, presentation: .full)
            }
            // `-voice-summary <seconds>` (DEBUG only): files a "Voice chat
            // ended · Ns" card into the feed for screenshot verification.
            if let raw = UserDefaults.standard.string(forKey: "voice-summary"),
               let seconds = Int(raw), viewModel.messages.isEmpty {
                viewModel.appendVoiceSummary(durationSeconds: seconds)
            }
            // `-autosend <text>` (DEBUG only): sends one message on appear —
            // exercises the full session-create + stream path without UI
            // automation. Model comes from the persisted ModelStore selection
            // (overridable the same way: `-allternit-runtime-model-id <id>`).
            if let text = UserDefaults.standard.string(forKey: "autosend"),
               sessionId == nil, viewModel.messages.isEmpty {
                viewModel.sendMessage(text, attachments: attachmentStore.attachments, runtimeModelId: modelForSend, effort: modelStore.effortForSend)
                attachmentStore.clear()
            }
            #endif
        }
        .onChange(of: sessionContext) { _, newContext in
            // Mode / agent pill changed pre-session: the next create is
            // stamped with the new context.
            viewModel.sessionContext = newContext
        }
        .onChange(of: sessionId) { _, newSessionId in
            // Tapping a history item swaps the session under this view.
            editingMessageId = nil
            if let newSessionId = newSessionId {
                viewModel.loadSession(newSessionId)
            } else {
                viewModel.startNewSession()
            }
        }
        .onChange(of: scenePhase) { _, newPhase in
            viewModel.handleScenePhase(newPhase)
        }
        .onChange(of: viewModel.draftToRestore) { _, draft in
            // A send that never landed hands its text back so the user can
            // retry without retyping.
            guard let draft else { return }
            inputText = draft
            viewModel.draftToRestore = nil
        }
    }

    #if DEBUG
    /// `-stage-test-attachment`: programmatically generate a 400x300 accent
    /// image and stage it — no photo-library fixture needed.
    private func stageTestAttachment() {
        let size = CGSize(width: 400, height: 300)
        let image = UIGraphicsImageRenderer(size: size).image { context in
            UIColor(named: "AccentPrimary")?.setFill()
            context.fill(CGRect(origin: .zero, size: size))
            "TEST".draw(at: CGPoint(x: 170, y: 140), withAttributes: [
                .font: UIFont.boldSystemFont(ofSize: 28),
                .foregroundColor: UIColor.white,
            ])
        }
        guard let data = image.pngData() else { return }
        attachmentStore.add(StagedAttachment(
            thumbnail: image,
            data: data,
            filename: "test-image.png",
            mediaType: "image/png"
        ))
    }
    #endif
}

// MARK: - Composer

/// Deck slide motion — matches the web's deck-rise / deck-fall keyframes:
/// 0.35s cubic-bezier(0.22, 1, 0.36, 1)
/// (surfaces/ai.allternit.com/tailwind.config.ts:89-116).
private enum DeckMotion {
    static let animation = Animation.timingCurve(0.22, 1, 0.36, 1, duration: 0.35)
}

/// Plain circular toolbar icon — no bubble border around the mic/waveform
/// or agent toggle, keeping the composer row clean.
@MainActor
private func toolbarIconButton(
    _ systemName: String,
    tint: Color? = nil,
    action: @escaping () -> Void
) -> some View {
    Button(action: {
        let generator = UIImpactFeedbackGenerator(style: .light)
        generator.impactOccurred()
        action()
    }) {
        Image(systemName: systemName)
            .font(.system(size: 16, weight: .semibold))
            .foregroundColor(tint ?? Color("TextPrimary"))
            .frame(width: 32, height: 32)
            .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
}

/// The platform composer: one card holding the editor and a toolbar row,
/// with the cowork top deck tucked behind its top edge and the agent-mode
/// bottom deck tucked behind its bottom edge (CoworkTopDeck.tsx:115-144,
/// ChatComposer.tsx:2549-2567). Toolbar order mirrors the web:
/// + attach, Chat/Cowork toggle (pre-session only), Agent pill + caret,
/// spacer, dictation mic, model selector, send/stop.
struct ComposerView: View {
    @Binding var inputText: String
    /// Staged attachments from the "+" sheet's pickers — rendered as the
    /// thumbnail strip above the text field, uploaded on send. Owned by
    /// ChatContentView (next to the view model that uploads them).
    @ObservedObject var attachmentStore: AttachmentStore
    let isStreaming: Bool
    /// True while the first message's session is being created — blocks
    /// double-sends into the same unbacked draft.
    let isSendDisabled: Bool
    /// The Chat/Cowork toggle is pre-session only (web `showModeToggle`).
    let hasActiveSession: Bool
    /// Hard usage wall (Phase 5): at/above 100% of the weekly window the
    /// composer locks — the field is not editable and send stays grayed.
    var isUsageLocked: Bool = false
    /// Waveform button is disabled while a stream is in flight or usage is
    /// locked, matching the send button's gating.
    var isVoiceModeEnabled: Bool = true
    /// Send tapped; carries the composer's staged attachments (empty when
    /// none) alongside the bound inputText.
    let onSend: ([StagedAttachment]) -> Void
    let onStop: () -> Void
    /// Dictation failures (permissions, engine) bubble up to the feed banner.
    let onDictationError: (String) -> Void
    /// Waveform button (Phase 7b): presents the full-screen voice mode.
    let onVoiceMode: () -> Void
    /// Called whenever the agent/bot toggle changes state so the host can
    /// mount or unmount a bot session context.
    var onAgentModeChanged: ((Bool) -> Void)? = nil

    @EnvironmentObject private var modeStore: AppModeStore
    @EnvironmentObject private var agentModeStore: AgentModeStore
    @StateObject private var modelStore = ModelStore.shared
    @StateObject private var dictation = DictationController()
    /// View-side dictation session flag: set on mic tap, cleared once the
    /// controller reports recording ended (silence, end tap, or error).
    @State private var isDictating = false
    /// Text in the field when dictation began (plus a trailing space); live
    /// partials render as `dictationBase + transcript` so Speech's word
    /// corrections replace earlier words instead of appending.
    @State private var dictationBase = ""
    /// The "+" button's sheet — attachments, tool toggles, and the
    /// Connectors entry (Claude iOS "Add to Chat" parity).
    @State private var isPlusSheetPresented = false
    /// Direct route to the connector browser (kept for the `-open-connectors`
    /// DEBUG launch arg; in-app it's reached via the "+" sheet's row).
    @State private var isConnectorsPresented = false
    /// First-run dictation onboarding (Phase 7a), shown once before the mic
    /// priming sheet (DictationOnboardingSheet.hasShown flag).
    @State private var isDictationOnboardingPresented = false
    /// App-owned mic priming sheet, shown before the first dictation's
    /// system prompts (once per install via AppPermission.microphone).
    @State private var isMicPrimingPresented = false
    /// When the mic priming sheet was raised by the VOICE-MODE button rather
    /// than the dictation mic, Continue presents voice mode instead of
    /// starting dictation (same shared mic permission, Phase 7b).
    @State private var micPrimingTargetsVoiceMode = false
    @State private var isModelPickerPresented = false
    /// Cowork workspace launchpad sheet — opens CoworkWorkspaceView in Cowork mode.
    @State private var isCoworkWorkspacePresented = false

    private var mode: AppMode { modeStore.mode }
    private var theme: ModeTheme { mode.theme }
    private var agentOn: Bool { agentModeStore.isAgentEnabled(for: mode) }

    /// Code mode gives the composer a code-accent border cue and a
    /// monospace, terminal-flavored editor — and NEVER shows agent mode
    /// (no pill, no deck, no mention chip, no Gizzi): a code thread is a
    /// plain terminal session.
    private var isTerminal: Bool { mode == .code }

    /// Agent-mode UI is shown only on surfaces that offer it.
    private var agentUI: Bool { agentOn && !isTerminal }

    private var canSend: Bool {
        !inputText.isEmpty && !isSendDisabled && !isUsageLocked
    }

    /// Mode-aware placeholder: the web cowork launchpad's prompt
    /// (CoworkLaunchpad.tsx:93) vs the chat composer's default; code mode
    /// reads like a shell prompt.
    private var placeholder: String {
        switch mode {
        case .cowork: return "What should we coordinate, build, or review?"
        case .code: return "describe the task…"
        default: return "Message Allternit..."
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            // Cowork top deck — tray tucked behind the card's top edge
            // (CoworkTopDeck.tsx: deck-rise), holding the Project and
            // Permissions selectors.
            if mode == .cowork {
                CoworkTopDeck()
                    .zIndex(0)
                    .transition(.move(edge: .bottom).combined(with: .opacity))
            }

            // Agent-mode top deck — the agent selector (the platform's
            // Agent Hub), tucked behind the card's top edge like the
            // cowork deck. Code mode never shows it; in cowork mode the
            // selector rides inside CoworkTopDeck instead, so agent-on
            // cowork gets ONE deck, not two stacked trays.
            if agentUI, mode != .cowork {
                AgentTopDeck()
                    .zIndex(0)
                    .transition(.move(edge: .bottom).combined(with: .opacity))
            }

            composerCard
                .zIndex(1)

            // Agent-mode bottom deck — collapsed to the selected tile by
            // default; tapping the tile expands the full grid again.
            // Code mode never shows it (no agent mode in the terminal).
            if agentUI {
                AgentModeBottomDeck(inputText: $inputText)
                    .zIndex(0)
                    .transition(.move(edge: .top).combined(with: .opacity))
            }
        }
        .animation(DeckMotion.animation, value: mode)
        .animation(DeckMotion.animation, value: agentUI)
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .sheet(isPresented: $isPlusSheetPresented) {
            ComposerPlusSheet(attachmentStore: attachmentStore)
        }
        .sheet(isPresented: $isConnectorsPresented) {
            ConnectorsListView()
        }
        .sheet(isPresented: $isDictationOnboardingPresented) {
            DictationOnboardingSheet {
                // Continue runs the existing flow: permission priming if the
                // system prompts are still pending, else dictation starts.
                if DictationController.systemPromptsPending(), !AppPermission.microphone.hasPrimed {
                    isMicPrimingPresented = true
                } else {
                    beginDictation()
                }
            }
        }
        .sheet(isPresented: $isMicPrimingPresented) {
            PermissionPrimingSheet(permission: .microphone) {
                if micPrimingTargetsVoiceMode {
                    micPrimingTargetsVoiceMode = false
                    onVoiceMode()
                } else {
                    beginDictation()
                }
            }
        }
        .sheet(isPresented: $isModelPickerPresented) {
            ModelPickerSheet(modelStore: modelStore)
        }
        .sheet(isPresented: $isCoworkWorkspacePresented) {
            CoworkWorkspaceView()
        }
        .onAppear {
            if agentOn { agentModeStore.fetchAgentsIfNeeded() }
            modelStore.fetchModelsIfNeeded()
            #if DEBUG
            // `-reset-onboarding` (DEBUG only): clears the dictation
            // onboarding flag and every AppPermission priming flag so the
            // first-run sheets show again. Runs before the `-open-*` args.
            // (The Phase-10 onboarding gate itself is cleared earlier, in
            // AllternitApp.init, so this launch lands on its page 1.)
            if launchArgumentEnabled("reset-onboarding") {
                DictationOnboardingSheet.resetShown()
                AppPermission.resetAllPriming()
            }
            // `-open-plus-sheet` / `-open-connectors` / `-open-model-picker`
            // (DEBUG only): jump straight to a composer sheet for UI
            // testing/screenshots — simctl has no tap-injection, so this is
            // the way to reach sheet content without a real touch.
            if launchArgumentEnabled("open-plus-sheet") {
                isPlusSheetPresented = true
            }
            if launchArgumentEnabled("open-connectors") {
                isConnectorsPresented = true
            }
            if launchArgumentEnabled("open-model-picker") {
                isModelPickerPresented = true
            }
            // `-open-dictation-onboarding` (DEBUG only): shows the first-run
            // dictation onboarding sheet for screenshot verification. Does
            // NOT mark it shown (only Continue does).
            if launchArgumentEnabled("open-dictation-onboarding") {
                isDictationOnboardingPresented = true
            }
            // `-open-mic-priming` (DEBUG only): shows the mic permission-
            // priming sheet for screenshot verification. NOTE: presenting it
            // this way marks the mic permission primed (shows-once flag).
            if launchArgumentEnabled("open-mic-priming") {
                isMicPrimingPresented = true
            }
            // `-enable-agent-mode` (DEBUG only): turn on agent mode at launch
            // so the agent pill, bottom deck, and Gizzi mascot can be
            // screenshot-verified without simctl tap injection.
            if launchArgumentEnabled("enable-agent-mode"), !agentOn {
                agentModeStore.toggleAgent(for: mode)
            }
            // `-select-website-mode` (DEBUG only): pre-select the Websites tile
            // and fill the composer so the collapsed agent-mode UX can be
            // screenshot-verified.
            if launchArgumentEnabled("select-website-mode") {
                agentModeStore.selectTile(.website, for: mode)
                inputText = AgentModeTile.website.taskPrompt
            }
            #endif
        }
        .onChange(of: agentOn) { _, on in
            // The agent selector lists the registry on first use (the web
            // fetches when agent mode activates, ChatComposer.tsx:779-800).
            if on {
                agentModeStore.fetchAgentsIfNeeded()
            } else {
                // Agent off: the composer goes back to default — any
                // mode-injected starter text leaves with the chip.
                inputText = ""
            }
            onAgentModeChanged?(on)
        }
        .onChange(of: dictation.transcript) { _, transcript in
            // Live partials replace the tail of the field; the base keeps
            // whatever the user typed before starting dictation.
            guard isDictating else { return }
            inputText = dictationBase + transcript
        }
        .onChange(of: dictation.isRecording) { _, isRecording in
            // Session over (silence timeout or error): fold the final
            // transcript in once, then detach.
            guard !isRecording, isDictating else { return }
            inputText = dictationBase + dictation.transcript
            isDictating = false
        }
        .onChange(of: dictation.errorMessage) { _, errorMessage in
            guard let errorMessage else { return }
            isDictating = false
            onDictationError(errorMessage)
        }
    }

    // MARK: Composer card

    private var composerCard: some View {
        VStack(alignment: .leading, spacing: 6) {
            // Staged attachments strip (Claude iOS parity: thumbnails above
            // the editor, X to remove) — visible only when picks exist.
            if !attachmentStore.attachments.isEmpty {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 8) {
                        ForEach(attachmentStore.attachments) { attachment in
                            attachmentThumb(attachment)
                        }
                    }
                }
            }

            // Text input — when agent mode is on, the selected mode lives
            // INSIDE the text as a real @-mention chip (NSTextAttachment),
            // not a view floating next to the field. Code mode renders the
            // editor as a shell input line: `❯` prompt + monospace text.
            HStack(alignment: .top, spacing: 6) {
                if isTerminal {
                    Text("❯")
                        .font(.system(size: 15, weight: .semibold, design: .monospaced))
                        .foregroundColor(Theme.accentCode)
                        .padding(.top, 8) // align with the editor's first line
                }

                ComposerTextView(
                    text: $inputText,
                    placeholder: placeholder,
                    isEnabled: !isUsageLocked,
                    mention: agentUI ? agentModeStore.selectedTile(for: mode) : nil,
                    terminal: isTerminal
                )
            }
            .padding(.horizontal, 1)

            // Toolbar row — clean icon-only controls so the composer never
            // feels crowded. No scrolling: every control stays fully visible.
            // Leading: + attach, Chat/Cowork segments (icon-only once agent
            // mode claims the row), Agent toggle.
            // Trailing: dictation mic, voice mode, model, send/stop.
            HStack(spacing: 2) {
                // Opens the "+" sheet (ComposerPlusSheet): attachments
                // (camera/photos/files), tool toggles, tool access, and
                // the Connectors entry — Claude iOS "Add to Chat" parity.
                toolbarIconButton("plus") {
                    isPlusSheetPresented = true
                }
                .accessibilityLabel("Add attachment")

                // Chat/Cowork toggle is a Home composer control
                // (pre-session only); the Code surface never offers it.
                if !hasActiveSession, mode == .chat || mode == .cowork {
                    ChatCoworkToggle()
                }

                // Cowork workspace launchpad — opens the full workspace when
                // in Cowork mode and no session is active.
                if !hasActiveSession, mode == .cowork {
                    toolbarIconButton("arrow.up.forward.square") {
                        isCoworkWorkspacePresented = true
                    }
                    .accessibilityLabel("Open Cowork workspace")
                }

                // Agent | Bot toggle chip. Not offered in code mode: the
                // terminal session has no agent deck.
                if !isTerminal {
                    AgentBotChip()
                }

                Spacer(minLength: 2)

                // Dictation mic: plain icon, red while recording.
                toolbarIconButton(dictation.isRecording ? "mic.fill" : "mic", tint: dictation.isRecording ? Theme.statusError : nil) {
                    toggleDictation()
                }
                .symbolEffect(.pulse, isActive: dictation.isRecording)

                // Voice mode (Phase 7b): plain waveform icon next to the mic.
                toolbarIconButton("waveform", tint: isVoiceModeEnabled ? nil : Color("TextSecondary").opacity(0.4)) {
                    voiceModeTapped()
                }
                .accessibilityLabel("Voice mode")
                .disabled(!isVoiceModeEnabled)

                // Model selector — compact pill with the current model label.
                Button(action: { isModelPickerPresented = true }) {
                    HStack(spacing: 3) {
                        Text(modelStore.pillLabel)
                            .font(.system(size: 11, weight: .medium))
                            .lineLimit(1)
                            .truncationMode(.tail)
                            .layoutPriority(-1)
                        Image(systemName: "chevron.down")
                            .font(.system(size: 8, weight: .bold))
                    }
                    .foregroundColor(isTerminal ? TerminalTheme.dim : Color("TextSecondary"))
                    .frame(height: 26)
                    .frame(maxWidth: 78)
                }

                if isStreaming {
                    Button(action: onStop) {
                        if isTerminal {
                            Image(systemName: "xmark")
                                .font(.system(size: 15, weight: .bold))
                                .foregroundColor(Theme.accentCode)
                                .frame(width: 32, height: 32)
                        } else {
                            Image(systemName: "stop.fill")
                                .font(.system(size: 13))
                                .foregroundColor(.black)
                                .frame(width: 32, height: 32)
                                .background(Color("AccentPrimary"))
                                .clipShape(Circle())
                        }
                    }
                } else if canSend {
                    Button(action: sendTapped) {
                        if isTerminal {
                            // Terminal send: a return key, not a chat bubble arrow.
                            Image(systemName: "return")
                                .font(.system(size: 15, weight: .bold))
                                .foregroundColor(Theme.accentCode)
                                .frame(width: 32, height: 32)
                        } else {
                            Image(systemName: "arrow.up")
                                .font(.system(size: 14, weight: .semibold))
                                .foregroundColor(.black)
                                .frame(width: 32, height: 32)
                                .background(Color("AccentPrimary"))
                                .clipShape(Circle())
                        }
                    }
                }
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 10)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusLG)
                .stroke(agentUI ? theme.accentGlow : (isTerminal ? Theme.accentCode.opacity(0.25) : Theme.borderWarmDefault), lineWidth: 1)
        )
        // Agent-on glow: accent border above + a soft halo shadow
        // (web: box-shadow 0 0 10px <glow>, BottomDock.tsx:194-207).
        .shadow(color: agentUI ? theme.accentGlow : .clear, radius: 5)
        // Gizzi mascot perches on the top edge of the input bar when agent
        // mode is active, mirroring the Allternit web platform behavior.
        .overlay(alignment: .top) {
            if agentUI {
                GizziMascotPill()
            }
        }
    }

    private func voiceModeTapped() {
        guard isVoiceModeEnabled else { return }

        let generator = UIImpactFeedbackGenerator(style: .light)
        generator.impactOccurred()

        if DictationController.systemPromptsPending(), !AppPermission.microphone.hasPrimed {
            // Same priming gate as dictation (Claude iOS parity: app-owned
            // explainer before the system prompts, once per install) —
            // Continue presents voice mode instead of starting dictation.
            micPrimingTargetsVoiceMode = true
            isMicPrimingPresented = true
        } else {
            onVoiceMode()
        }
    }

    private func toggleDictation() {
        let generator = UIImpactFeedbackGenerator(style: .light)
        generator.impactOccurred()

        if dictation.isRecording {
            dictation.stop()
            // isDictating is cleared by the isRecording onChange, which
            // folds the final transcript into the field.
        } else if !DictationOnboardingSheet.hasShown {
            // First-ever dictation: the onboarding sheet goes up BEFORE
            // everything else (Claude iOS parity). Its Continue then runs
            // the permission-priming / start flow.
            isDictationOnboardingPresented = true
        } else if DictationController.systemPromptsPending(), !AppPermission.microphone.hasPrimed {
            // Prime once: the app-owned explainer sheet goes up BEFORE the
            // Speech/mic system prompts (Claude iOS parity). Continue runs
            // beginDictation(), which triggers the real prompts.
            isMicPrimingPresented = true
        } else {
            beginDictation()
        }
    }

    private func beginDictation() {
        var base = inputText
        if !base.isEmpty, !base.hasSuffix(" ") {
            base += " "
        }
        dictationBase = base
        isDictating = true
        Task { await dictation.start() }
    }

    /// One staged attachment in the composer strip: image thumbnail or a
    /// file icon chip, with an X to unstage.
    private func attachmentThumb(_ attachment: StagedAttachment) -> some View {
        ZStack(alignment: .topTrailing) {
            Group {
                if let thumbnail = attachment.thumbnail {
                    Image(uiImage: thumbnail)
                        .resizable()
                        .scaledToFill()
                } else {
                    VStack(spacing: 2) {
                        Image(systemName: attachment.fileIcon)
                            .font(.system(size: 16, weight: .medium))
                        Text(attachment.filename)
                            .font(.system(size: 8))
                            .lineLimit(1)
                            .truncationMode(.middle)
                    }
                    .foregroundColor(Color("TextSecondary"))
                    .padding(6)
                }
            }
            .frame(width: 56, height: 56)
            .background(Color("BgSecondary"))
            .clipShape(RoundedRectangle(cornerRadius: Theme.radiusSM))
            .overlay(
                RoundedRectangle(cornerRadius: Theme.radiusSM)
                    .stroke(Theme.borderWarmDefault, lineWidth: 1)
            )

            Button(action: {
                let generator = UIImpactFeedbackGenerator(style: .light)
                generator.impactOccurred()
                attachmentStore.remove(id: attachment.id)
            }) {
                Image(systemName: "xmark")
                    .font(.system(size: 8, weight: .bold))
                    .foregroundColor(.white)
                    .frame(width: 18, height: 18)
                    .background(Color.black.opacity(0.7))
                    .clipShape(Circle())
            }
            .offset(x: 4, y: -4)
        }
        .padding(.top, 4)
        .padding(.trailing, 4)
    }

    private func sendTapped() {
        // Sending ends any live dictation: fold the latest transcript in
        // first so nothing the user said is lost.
        if isDictating {
            dictation.stop()
            inputText = dictationBase + dictation.transcript
            isDictating = false
        }
        onSend(attachmentStore.attachments)
        attachmentStore.clear()
    }
}

// MARK: - Chat / Cowork toggle

/// Icon-only 28pt segmented pair [Chat | Cowork] (BottomDock.tsx
/// ChatCoworkToggle, lines 18-63). Cowork is a composer-level mode, not a
/// tab: selecting it sets the app mode (accent turns purple, the top deck
/// appears); selecting Chat returns. Pre-session only — the caller hides it
/// once a session is active.
struct ChatCoworkToggle: View {
    @EnvironmentObject private var modeStore: AppModeStore

    var body: some View {
        HStack(spacing: 0) {
            segment(mode: .chat, icon: "message", label: "Chat")
            Rectangle()
                .fill(Theme.borderWarmDefault)
                .frame(width: 1, height: 14)
            segment(mode: .cowork, icon: "person.3", label: "Cowork")
        }
        .frame(height: 26)
        .clipShape(RoundedRectangle(cornerRadius: 7))
        .overlay(
            RoundedRectangle(cornerRadius: 7)
                .stroke(Theme.borderWarmDefault, lineWidth: 1)
        )
    }

    private func segment(mode: AppMode, icon: String, label: String) -> some View {
        let isActive = modeStore.mode == mode
        return Button(action: {
            let generator = UIImpactFeedbackGenerator(style: .light)
            generator.impactOccurred()
            modeStore.mode = mode
        }) {
            Image(systemName: icon)
                .font(.system(size: 11, weight: .semibold))
            // Active = soft bg + mode accent (web bg-composer-soft);
            // inactive = muted.
            .foregroundColor(isActive ? mode.theme.accent : Color("TextSecondary"))
            .padding(.horizontal, 10)
            .frame(height: 26)
            .background(isActive ? mode.theme.accentSoft : Color.clear)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }
}

// MARK: - Agent | Bot toggle chip

/// Bot Off / Bot On chip. Tapping toggles agent mode for the current surface.
/// When on, the chip shows the selected agent's avatar (or the default sparkles
/// glyph) and opens the agent/bot selection sheet on a secondary tap.
struct AgentBotChip: View {
    @EnvironmentObject private var modeStore: AppModeStore
    @EnvironmentObject private var agentModeStore: AgentModeStore
    @State private var isSelectionSheetPresented = false

    private var mode: AppMode { modeStore.mode }
    private var theme: ModeTheme { mode.theme }
    private var agentOn: Bool { agentModeStore.isAgentEnabled(for: mode) }
    private var selectedAgent: AgentRecord? { agentModeStore.selectedAgent(for: mode) }

    var body: some View {
        Button(action: {
            let generator = UIImpactFeedbackGenerator(style: .light)
            generator.impactOccurred()
            if agentOn {
                isSelectionSheetPresented = true
            } else {
                agentModeStore.toggleAgent(for: mode)
            }
        }) {
            HStack(spacing: 6) {
                if agentOn {
                    if let selectedAgent {
                        AgentAvatarView(agent: selectedAgent, size: 18)
                    } else {
                        Image(systemName: "sparkles")
                            .font(.system(size: 11, weight: .semibold))
                            .foregroundColor(theme.accent)
                    }
                    Text("Bot on")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundColor(theme.accent)
                } else {
                    Image(systemName: "cpu")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundColor(Color("TextSecondary"))
                    Text("Bot off")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundColor(Color("TextSecondary"))
                }
            }
            .padding(.horizontal, 10)
            .frame(height: 26)
            .background(
                Capsule()
                    .fill(agentOn ? theme.accentSoft : Color("BgPanel").opacity(0.55))
                    .background(.ultraThinMaterial, in: Capsule())
            )
            .overlay(
                Capsule()
                    .stroke(agentOn ? theme.accentGlow : Color("BorderSubtle").opacity(0.55), lineWidth: 1)
            )
        }
        .buttonStyle(.plain)
        .accessibilityLabel(agentOn ? "Bot on" : "Bot off")
        .sheet(isPresented: $isSelectionSheetPresented) {
            AgentSelectionSheet()
        }
        .onLongPressGesture {
            let generator = UIImpactFeedbackGenerator(style: .light)
            generator.impactOccurred()
            agentModeStore.setAgentEnabled(false, for: mode)
        }
    }
}

/// UITextView-backed composer editor so the selected agent mode can live
/// INSIDE the text as a real @-mention chip (NSTextAttachment) instead of a
/// view floating next to the field. The chip plus its trailing space form a
/// protected prefix: they render in the view but never leak into the bound
/// text, and they can't be deleted or typed before.
private struct ComposerTextView: UIViewRepresentable {
    @Binding var text: String
    var placeholder: String
    var isEnabled: Bool
    /// Selected agent-mode tile → mention chip; nil = plain editor.
    var mention: AgentModeTile?
    /// Code mode: monospace font on the terminal palette.
    var terminal: Bool = false

    /// Editor font for the current style (body vs terminal monospace).
    private var uiFont: UIFont {
        terminal
            ? UIFont.monospacedSystemFont(ofSize: 15, weight: .regular)
            : UIFont.preferredFont(forTextStyle: .body)
    }

    private var uiTextColor: UIColor {
        terminal ? UIColor(TerminalTheme.text) : (UIColor(named: "TextPrimary") ?? .label)
    }

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeUIView(context: Context) -> UITextView {
        let textView = UITextView()
        textView.delegate = context.coordinator
        textView.font = uiFont
        textView.textColor = uiTextColor
        textView.backgroundColor = .clear
        textView.textContainerInset = UIEdgeInsets(top: 8, left: 5, bottom: 8, right: 5)
        // Kill the 5pt fragment padding so typed text and the caret start at
        // exactly the placeholder's x — otherwise the caret lands on top of
        // the placeholder's first glyphs.
        textView.textContainer.lineFragmentPadding = 0
        textView.autocapitalizationType = terminal ? .none : .sentences
        textView.autocorrectionType = terminal ? .no : .default
        textView.returnKeyType = .default
        textView.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        let placeholderLabel = context.coordinator.placeholderLabel
        placeholderLabel.font = uiFont
        placeholderLabel.textColor = terminal
            ? UIColor(TerminalTheme.dim)
            : UIColor(named: "TextSecondary")?.withAlphaComponent(0.6)
        placeholderLabel.lineBreakMode = .byTruncatingTail
        placeholderLabel.translatesAutoresizingMaskIntoConstraints = false
        textView.addSubview(placeholderLabel)
        context.coordinator.placeholderLeading =
            placeholderLabel.leadingAnchor.constraint(equalTo: textView.leadingAnchor, constant: 5)
        NSLayoutConstraint.activate([
            context.coordinator.placeholderLeading!,
            placeholderLabel.topAnchor.constraint(equalTo: textView.topAnchor, constant: 8),
            placeholderLabel.trailingAnchor.constraint(lessThanOrEqualTo: textView.trailingAnchor, constant: -5),
        ])

        applyContent(to: textView, coordinator: context.coordinator, moveCursorToEnd: false)
        context.coordinator.updatePlaceholder(textView)
        return textView
    }

    func updateUIView(_ textView: UITextView, context: Context) {
        context.coordinator.parent = self
        textView.isEditable = isEnabled
        context.coordinator.placeholderLabel.text = placeholder
        // Rebuild only when the visible raw text diverges from the binding
        // (external fills: dictation, templates, edit-resend) or the mention
        // changed — never mid-keystroke, so the cursor stays put.
        if context.coordinator.rawText(in: textView) != text || context.coordinator.mention != mention {
            applyContent(to: textView, coordinator: context.coordinator, moveCursorToEnd: true)
        }
        context.coordinator.updatePlaceholder(textView)
        #if DEBUG
        // `-focus-composer` (DEBUG only): focus the editor on launch so the
        // caret's alignment can be screenshot-verified (no tap injection in
        // simctl).
        if launchArgumentEnabled("focus-composer"), !textView.isFirstResponder {
            DispatchQueue.main.async { textView.becomeFirstResponder() }
        }
        #endif
    }

    func sizeThatFits(_ proposal: ProposedViewSize, uiView: UITextView, context: Context) -> CGSize? {
        let width = proposal.width ?? UIScreen.main.bounds.width
        let fit = uiView.sizeThatFits(CGSize(width: width, height: .greatestFiniteMagnitude))
        let line = uiView.font?.lineHeight ?? 22
        let maxHeight = line * 6 + uiView.textContainerInset.top + uiView.textContainerInset.bottom
        uiView.isScrollEnabled = fit.height > maxHeight + 1
        return CGSize(width: width, height: min(fit.height, maxHeight))
    }

    // MARK: Mention chip

    private func applyContent(to textView: UITextView, coordinator: Coordinator, moveCursorToEnd: Bool) {
        let composed = NSMutableAttributedString()
        if let mention {
            composed.append(Self.mentionChunk(for: mention, font: uiFont))
            coordinator.prefixLength = composed.length // attachment char + space
        } else {
            coordinator.prefixLength = 0
        }
        composed.append(Self.bodyChunk(text, font: uiFont, color: uiTextColor))
        coordinator.mention = mention
        textView.attributedText = composed
        if moveCursorToEnd {
            textView.selectedRange = NSRange(location: composed.length, length: 0)
        }
    }

    fileprivate static func bodyChunk(_ string: String, font: UIFont, color: UIColor) -> NSAttributedString {
        NSAttributedString(string: string, attributes: [
            .font: font,
            .foregroundColor: color,
        ])
    }

    fileprivate static func mentionChunk(for tile: AgentModeTile, font: UIFont) -> NSAttributedString {
        let image = chipImage(for: tile)
        let attachment = NSTextAttachment()
        attachment.image = image
        // Vertically center the chip on the font's cap-height band so the
        // text baseline — and the caret — stay exactly where an un-chipped
        // line puts them.
        let y = (font.capHeight - image.size.height) / 2
        attachment.bounds = CGRect(x: 0, y: y, width: image.size.width, height: image.size.height)
        let chunk = NSMutableAttributedString(attachment: attachment)
        chunk.append(NSAttributedString(string: " ", attributes: [
            .font: font,
        ]))
        return chunk
    }

    /// Icon-only circular token, tile-colored — rendered as an image so it
    /// can sit inside the text like a real mention chip. Sized to sit inside
    /// a normal text line without inflating it.
    private static func chipImage(for tile: AgentModeTile) -> UIImage {
        let color = UIColor(tile.color)
        let side: CGFloat = 18
        let icon = UIImage(systemName: tile.icon, withConfiguration:
            UIImage.SymbolConfiguration(pointSize: 9, weight: .semibold))?
            .withTintColor(color, renderingMode: .alwaysOriginal)
        return UIGraphicsImageRenderer(size: CGSize(width: side, height: side)).image { _ in
            let rect = CGRect(origin: .zero, size: CGSize(width: side, height: side))
            let path = UIBezierPath(ovalIn: rect.insetBy(dx: 0.5, dy: 0.5))
            color.withAlphaComponent(0.14).setFill()
            path.fill()
            color.withAlphaComponent(0.45).setStroke()
            path.lineWidth = 1
            path.stroke()
            if let icon {
                icon.draw(at: CGPoint(x: (side - icon.size.width) / 2,
                                      y: (side - icon.size.height) / 2))
            }
        }
    }

    // MARK: Coordinator

    final class Coordinator: NSObject, UITextViewDelegate {
        var parent: ComposerTextView
        /// UTF-16 length of the protected mention prefix (0 = no chip).
        var prefixLength = 0
        var mention: AgentModeTile?
        let placeholderLabel = UILabel()
        var placeholderLeading: NSLayoutConstraint?

        init(_ parent: ComposerTextView) { self.parent = parent }

        /// The user-owned text: everything after the protected chip prefix.
        func rawText(in textView: UITextView) -> String {
            let full = textView.attributedText.string as NSString
            guard full.length >= prefixLength else { return "" }
            return full.substring(from: prefixLength)
        }

        func updatePlaceholder(_ textView: UITextView) {
            placeholderLabel.isHidden = !rawText(in: textView).isEmpty
            // Keep the placeholder clear of the chip when one is present.
            placeholderLeading?.constant = 5 + (mention != nil ? 23 : 0)
        }

        func textViewDidChange(_ textView: UITextView) {
            parent.text = rawText(in: textView)
            updatePlaceholder(textView)
        }

        func textView(_ textView: UITextView, shouldChangeTextIn range: NSRange, replacementText replacement: String) -> Bool {
            guard prefixLength > 0, range.location < prefixLength else { return true }
            // The edit touches the protected chip (select-all + type/delete,
            // backspace at the boundary): re-apply it AFTER the chip instead.
            let raw = rawText(in: textView) as NSString
            let cut = max(0, range.location - prefixLength)
            let end = max(cut, range.location + range.length - prefixLength)
            let newRaw = raw.replacingCharacters(in: NSRange(location: cut, length: end - cut), with: replacement)
            parent.text = newRaw

            let composed = NSMutableAttributedString()
            if let mention {
                composed.append(ComposerTextView.mentionChunk(for: mention, font: parent.uiFont))
            }
            composed.append(ComposerTextView.bodyChunk(newRaw, font: parent.uiFont, color: parent.uiTextColor))
            textView.attributedText = composed
            textView.selectedRange = NSRange(
                location: prefixLength + cut + (replacement as NSString).length, length: 0)
            updatePlaceholder(textView)
            return false
        }

        func textViewDidChangeSelection(_ textView: UITextView) {
            // Never let the cursor sit before (or inside) the chip.
            guard prefixLength > 0, textView.selectedRange.location < prefixLength else { return }
            let spill = max(0, textView.selectedRange.location + textView.selectedRange.length - prefixLength)
            textView.selectedRange = NSRange(location: prefixLength, length: spill)
        }
    }
}

// MARK: - Deck pill chrome

/// Shared deck-pill chrome (project / permission / agent selectors): a
/// compact glass capsule so the tray stays a sliver above the composer card.
/// Text uses the primary color for legibility against the blurred backing.
private func deckPill(icon: String, iconColor: Color, text: String) -> some View {
    HStack(spacing: 4) {
        Image(systemName: icon)
            .font(.system(size: 9, weight: .semibold))
            .foregroundColor(iconColor)
        Text(text)
            .font(.system(size: 10, weight: .semibold))
            .foregroundColor(Color("TextPrimary"))
            .lineLimit(1)
            .frame(maxWidth: 88)
        Image(systemName: "chevron.down")
            .font(.system(size: 7, weight: .bold))
            .foregroundColor(Color("TextSecondary"))
            .opacity(0.7)
    }
    .padding(.horizontal, 8)
    .frame(height: 24)
    .background(
        Capsule()
            .fill(Color("BgSecondary").opacity(0.45))
            .background(.ultraThinMaterial, in: Capsule())
    )
    .overlay(Capsule().stroke(Color("BorderSubtle").opacity(0.55), lineWidth: 1))
}

// MARK: - Agent selection menu

/// The platform's Agent Hub as a deck pill (AgentSelectorDropdown): taps
/// open `AgentSelectionSheet`, the quick-switcher listing the registry
/// agents valid for this surface; the selection rides on the next session
/// create as `agent_id`/`agent_name` (ChatContentView.sessionContext) and
/// its persona/model are injected at send time (ChatViewModel
/// composedSystemPrompt). Shared by AgentTopDeck (chat) and CoworkTopDeck
/// (cowork), so agent-on never needs a second tray.
private struct AgentSelectionMenu: View {
    @EnvironmentObject private var modeStore: AppModeStore
    @EnvironmentObject private var agentModeStore: AgentModeStore
    @State private var isSheetPresented = false

    private var surface: AppMode { modeStore.mode }

    var body: some View {
        Button(action: { isSheetPresented = true }) {
            deckPill(
                icon: "cpu",
                iconColor: Color("AccentPrimary"),
                text: agentModeStore.selectedAgent(for: surface)?.name ?? "Default agent"
            )
        }
        .buttonStyle(.plain)
        .sheet(isPresented: $isSheetPresented) {
            AgentSelectionSheet()
        }
        .onAppear {
            #if DEBUG
            // `-open-agent-sheet` (DEBUG only): open the picker on launch
            // for screenshot verification (no tap injection in simctl).
            if launchArgumentEnabled("open-agent-sheet") {
                isSheetPresented = true
            }
            #endif
        }
    }
}

// MARK: - Cowork top deck

/// Cowork tray tucked behind the composer card's TOP edge
/// (CoworkTopDeck.tsx): 56pt total with the bottom 12pt hidden under the
/// card, rounded top corners, holding the Project and Permissions dropdown
/// pills — plus the Agent selector when agent mode is on (one deck, not
/// two stacked trays). Backing state: ProjectStore (project) +
/// AgentModeStore (coworkPermission, agent selection), all persisted.
struct CoworkTopDeck: View {
    @EnvironmentObject private var modeStore: AppModeStore
    @EnvironmentObject private var agentModeStore: AgentModeStore
    @StateObject private var projectStore = ProjectStore.shared

    private var agentOn: Bool { agentModeStore.isAgentEnabled(for: modeStore.mode) }

    var body: some View {
        HStack(spacing: 8) {
            projectMenu
            permissionMenu
            if agentOn {
                AgentSelectionMenu()
            }
            Spacer()
        }
        .padding(.horizontal, 12)
        // Keep the pills clear of the Gizzi mascot perched on the card's
        // top-right corner when agent mode is on (mascot spans ~48pt).
        .padding(.trailing, agentOn ? 60 : 0)
        .padding(.bottom, 12) // tucked portion hidden under the card
        .frame(height: 56)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            UnevenRoundedRectangle(topLeadingRadius: Theme.radiusLG, topTrailingRadius: Theme.radiusLG)
                .fill(Color("BgPanel").opacity(0.72))
                .background(.ultraThinMaterial, in:
                    UnevenRoundedRectangle(topLeadingRadius: Theme.radiusLG, topTrailingRadius: Theme.radiusLG))
        )
        .overlay(
            UnevenRoundedRectangle(topLeadingRadius: Theme.radiusLG, topTrailingRadius: Theme.radiusLG)
                .stroke(Color("BorderSubtle").opacity(0.55), lineWidth: 1)
        )
        .padding(.bottom, -12) // the card overlaps the deck's bottom
        .onAppear {
            projectStore.fetchProjectsIfNeeded()
            if agentOn { agentModeStore.fetchAgentsIfNeeded() }
        }
    }

    private var projectMenu: some View {
        Menu {
            Button(action: { projectStore.selectedProjectId = nil }) {
                HStack {
                    if projectStore.selectedProjectId == nil {
                        Image(systemName: "checkmark")
                    }
                    Text("No project")
                }
            }
            ForEach(projectStore.projects) { project in
                Button(action: { projectStore.selectedProjectId = project.id }) {
                    HStack {
                        if projectStore.selectedProjectId == project.id {
                            Image(systemName: "checkmark")
                        }
                        Text(project.title)
                    }
                }
            }
        } label: {
            deckPill(
                icon: "folder",
                iconColor: Color("AccentPrimary"),
                text: projectStore.selectedProject?.title ?? "Select project"
            )
        }
    }

    private var permissionMenu: some View {
        Menu {
            ForEach(CoworkPermission.allCases, id: \.self) { permission in
                Button(action: { agentModeStore.coworkPermission = permission }) {
                    HStack {
                        if agentModeStore.coworkPermission == permission {
                            Image(systemName: "checkmark")
                        }
                        Text(permission.label)
                    }
                }
            }
        } label: {
            deckPill(
                icon: "checkmark.shield",
                iconColor: Theme.statusWarning,
                text: agentModeStore.coworkPermission.label
            )
        }
    }

}

// MARK: - Agent-mode top deck

/// Agent-on tray tucked behind the composer card's TOP edge (same tuck as
/// CoworkTopDeck): the agent selector — the platform's Agent Hub
/// (AgentSelectorDropdown). Chat surface only; in cowork the selector
/// rides inside CoworkTopDeck.
struct AgentTopDeck: View {
    @EnvironmentObject private var agentModeStore: AgentModeStore

    var body: some View {
        HStack(spacing: 8) {
            AgentSelectionMenu()
            Spacer()
        }
        .padding(.horizontal, 12)
        .padding(.bottom, 12) // tucked portion hidden under the card
        .frame(height: 56)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            UnevenRoundedRectangle(topLeadingRadius: Theme.radiusLG, topTrailingRadius: Theme.radiusLG)
                .fill(Color("BgPanel").opacity(0.72))
                .background(.ultraThinMaterial, in:
                    UnevenRoundedRectangle(topLeadingRadius: Theme.radiusLG, topTrailingRadius: Theme.radiusLG))
        )
        .overlay(
            UnevenRoundedRectangle(topLeadingRadius: Theme.radiusLG, topTrailingRadius: Theme.radiusLG)
                .stroke(Color("BorderSubtle").opacity(0.55), lineWidth: 1)
        )
        .padding(.bottom, -12) // the card overlaps the deck's bottom
        .onAppear { agentModeStore.fetchAgentsIfNeeded() }
    }
}

// MARK: - Agent-mode bottom deck

/// Agent-on tray tucked behind the composer card's bottom edge.
///
/// Expanded: a horizontally scrolling row of every mode tile so all 9 modes
/// are reachable without guessing.
///
/// Collapsed (a mode is selected): the deck stays full-width but shows ONLY
/// the selected mode's template chips — the mode itself is already shown as
/// the icon chip inside the input area. Tapping a chip fills the composer.
struct AgentModeBottomDeck: View {
    @Binding var inputText: String
    @EnvironmentObject private var modeStore: AppModeStore
    @EnvironmentObject private var agentModeStore: AgentModeStore

    private var surface: AppMode { modeStore.mode }
    private var tile: AgentModeTile { agentModeStore.selectedTile(for: surface) }
    private var expanded: Bool { agentModeStore.isAgentModeDeckExpanded(for: surface) }
    /// Code mode sets template rows monospace (terminal flavor).
    private var isTerminal: Bool { surface == .code }

    var body: some View {
        VStack(spacing: 0) {
            if expanded {
                expandedTiles
            } else {
                collapsedTemplates
            }
        }
        .background(
            UnevenRoundedRectangle(bottomLeadingRadius: Theme.radiusLG, bottomTrailingRadius: Theme.radiusLG)
                .fill(Color("BgPanel").opacity(0.72))
                .background(.ultraThinMaterial, in:
                    UnevenRoundedRectangle(bottomLeadingRadius: Theme.radiusLG, bottomTrailingRadius: Theme.radiusLG))
        )
        .overlay(
            UnevenRoundedRectangle(bottomLeadingRadius: Theme.radiusLG, bottomTrailingRadius: Theme.radiusLG)
                .stroke(Color("BorderSubtle").opacity(0.55), lineWidth: 1)
        )
        .padding(.top, -12) // the card overlaps the deck's top
        .zIndex(0)
        .animation(DeckMotion.animation, value: expanded)
    }

    /// Expanded: a horizontally scrolling row of mode tabs separated by a
    /// pipe character, matching the original web mode selector.
    private var expandedTiles: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 0) {
                let tiles = AgentModeTile.visibleTiles(for: surface)
                ForEach(Array(tiles.enumerated()), id: \.element) { index, tile in
                    modeTab(tile)
                    if index < tiles.count - 1 {
                        Text("|")
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundColor(Color("BorderSubtle"))
                            .padding(.horizontal, 8)
                    }
                }
            }
            .padding(.horizontal, 12)
        }
        .frame(height: 38)
        .padding(.top, 16) // tucked portion hidden under the card
        .padding(.bottom, 10)
    }

    /// Collapsed: full-width deck listing the selected mode's templates as
    /// horizontally scrolling chips. Tapping a chip fills the composer.
    private var collapsedTemplates: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                ForEach(tile.templates, id: \.self) { template in
                    Button(action: {
                        let generator = UIImpactFeedbackGenerator(style: .light)
                        generator.impactOccurred()
                        inputText = template
                    }) {
                        HStack(spacing: 6) {
                            Image(systemName: "sparkles")
                                .font(.system(size: 10, weight: .semibold))
                                .foregroundColor(tile.color)
                            Text(template)
                                .font(.system(size: 12, weight: .medium, design: isTerminal ? .monospaced : .default))
                                .foregroundColor(Color("TextPrimary"))
                                .lineLimit(1)
                        }
                        .padding(.horizontal, 12)
                        .frame(height: 32)
                        .background(Color("BgSecondary").opacity(0.55))
                        .clipShape(Capsule())
                        .overlay(
                            Capsule()
                                .stroke(Theme.borderWarmDefault, lineWidth: 1)
                        )
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(.horizontal, 12)
        }
        .frame(height: 44)
        .padding(.top, 16) // tucked portion hidden under the card
        .padding(.bottom, 12)
    }

    /// One mode tab: icon + label, tinted when selected.
    @ViewBuilder
    private func modeTab(_ tile: AgentModeTile) -> some View {
        let isSelected = agentModeStore.selectedTile(for: surface) == tile
        Button(action: {
            let generator = UIImpactFeedbackGenerator(style: .light)
            generator.impactOccurred()
            agentModeStore.selectTile(tile, for: surface)
            inputText = tile.taskPrompt
        }) {
            HStack(spacing: 5) {
                Image(systemName: tile.icon)
                    .font(.system(size: 12, weight: .semibold))
                Text(tile.label)
                    .font(.system(size: 12, weight: .semibold))
                    .lineLimit(1)
            }
            .foregroundColor(isSelected ? tile.color : Color("TextSecondary"))
            .padding(.horizontal, 10)
            .frame(height: 30)
            .background(isSelected ? tile.color.opacity(0.14) : Color.clear)
            .clipShape(Capsule())
        }
        .buttonStyle(.plain)
    }
}

// MARK: - Empty state & banners

// Greeting data matching surfaces/ai.allternit.com/src/views/chat/main/launchGreeting.ts.
private let launchTitles = [
    "Allternit & Coffee",
    "Ready to Build?",
    "The Architect's Den",
    "Good to see you, Architect",
    "Creative Control",
    "Allternit",
]

private let launchTaglines = [
    "The Intelligent Workspace",
    "Your Architecture, Amplified",
    "Coffee, Code, and Creativity",
    "Where Logic Meets Elegance",
    "Precision in Every Interaction",
    "Stay curious, stay creative.",
]

/// Agent-specific greeting + suggested-prompt chips on a fresh chat
/// (PocketPal Pal-inspired). Deliberately narrower than the generic
/// "suggestion rows on every empty chat" pattern that was removed elsewhere
/// to keep the feed uncluttered — this only renders when the composer has a
/// specific agent selected (not the default backend agent), and only when
/// that agent actually has a greeting/prompts set.
struct AgentGreetingView: View {
    let agent: AgentRecord
    /// Tapping a chip fills the composer — never auto-sends.
    let onSelectPrompt: (String) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if let greeting = agent.greeting {
                Text(greeting)
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
                    .multilineTextAlignment(.leading)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            if !agent.suggestedPrompts.isEmpty {
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(agent.suggestedPrompts, id: \.self) { prompt in
                        Button(action: {
                            let generator = UISelectionFeedbackGenerator()
                            generator.selectionChanged()
                            onSelectPrompt(prompt)
                        }) {
                            Text(prompt)
                                .font(.subheadline)
                                .foregroundColor(Color("TextPrimary"))
                                .padding(.horizontal, 14)
                                .padding(.vertical, 10)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .background(Color("BgPanel"))
                                .clipShape(RoundedRectangle(cornerRadius: 14))
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
        }
    }
}

struct EmptyChatStateView: View {
    // Pick a random greeting once per view lifetime (matches web behavior
    // where the greeting is cached per renderer session).
    @State private var greeting: (title: String, tagline: String) = {
        let title = launchTitles[Int.random(in: 0..<launchTitles.count)]
        let tagline = launchTaglines[Int.random(in: 0..<launchTaglines.count)]
        return (title, tagline)
    }()

    /// Logo glow animation state.
    @State private var logoGlowing = false
    /// Staggered reveal for the greeting title.
    @State private var titleRevealed = false
    @State private var taglineRevealed = false

    var body: some View {
        VStack(spacing: 0) {
            // ── Brand mark with accent glow (LaunchHeader.tsx:40-51) ──
            // The matrix logo (01_brand/logos/matrix), replacing the old
            // "A://" tile per Eoj; GizziMascot is also in the catalog.
            ZStack {
                // Ambient glow behind the logo
                Circle()
                    .fill(Color("AccentPrimary").opacity(logoGlowing ? 0.12 : 0.04))
                    .frame(width: 120, height: 120)
                    .blur(radius: 30)
                    .animation(.easeInOut(duration: 2.5).repeatForever(autoreverses: true), value: logoGlowing)

                // The source art carries generous transparent padding, so
                // the frame is larger than the old 72pt tile for a similar
                // visual footprint.
                Image("MatrixLogo")
                    .resizable()
                    .scaledToFit()
                    .frame(width: 160, height: 160)
                    .shadow(color: Color("AccentPrimary").opacity(0.15), radius: 12, y: 4)
            }
            .padding(.bottom, 24)

            // ── Greeting title (5xl, medium, serif — LaunchHeader.tsx:53-61) ──
            Text(greeting.title)
                .font(.system(size: 32, weight: .medium, design: .serif))
                .foregroundColor(Color("TextPrimary"))
                .multilineTextAlignment(.center)
                .opacity(titleRevealed ? 1 : 0)
                .offset(y: titleRevealed ? 0 : 12)
                .animation(.easeOut(duration: 0.7), value: titleRevealed)
                .padding(.bottom, 16)

            // ── Tagline with decorative lines (LaunchHeader.tsx:63-75) ──
            HStack(spacing: 12) {
                Rectangle()
                    .fill(Color("BorderSubtle"))
                    .frame(width: 28, height: 1)

                Text(greeting.tagline.uppercased())
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
                    .tracking(2.5)
                    .lineLimit(1)
                    .minimumScaleFactor(0.8)

                Rectangle()
                    .fill(Color("BorderSubtle"))
                    .frame(width: 28, height: 1)
            }
            .opacity(taglineRevealed ? 1 : 0)
            .offset(y: taglineRevealed ? 0 : 8)
            .animation(.easeOut(duration: 0.6).delay(0.3), value: taglineRevealed)
        }
        .onAppear {
            logoGlowing = true
            // Stagger the reveal animation like the web's typing/reveal effect.
            withAnimation { titleRevealed = true }
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) {
                withAnimation { taglineRevealed = true }
            }
        }
    }
}

/// Incognito empty state (Phase 6, Claude iOS parity): a ghost glyph with
/// the privacy explainer in place of the greeting. Shown by ChatContentView
/// whenever the feed is empty and `ChatViewModel.isIncognito` is on.
struct IncognitoEmptyStateView: View {
    /// Opens the "Learn more" link (SFSafariViewController via the host).
    let onLearnMore: () -> Void

    var body: some View {
        FriendlyStateView(
            style: .empty,
            icon: ChatView.incognitoSymbolName,
            title: "Incognito chat",
            message: "Incognito chats can't access memory. They aren't saved to history, added to memory, or used to train models.",
            actionTitle: "Learn more about how your data is used.",
            action: onLearnMore
        )
    }
}

/// A single suggestion card matching the web's prompt-card style.
/// Dismissible one-line banner for transient load/send failures
/// (ChatViewModel.transientError), pinned to the top of the feed.
struct TransientErrorBanner: View {
    let message: String
    let onDismiss: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.subheadline)
                .foregroundColor(Theme.statusError)

            Text(message)
                .font(.subheadline)
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(2)

            Spacer(minLength: 8)

            Button(action: onDismiss) {
                Image(systemName: "xmark")
                    .font(.caption)
                    .fontWeight(.semibold)
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 28, height: 28)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .background(Color("BgSecondary"))
    }
}

// MARK: - Terminal session (code threads)

/// Terminal-styled feed row for code threads: `❯` prompt line for the user;
/// for the assistant a dim thought stream (`✻ thinking`), `$`-prefixed
/// tool-call status lines, then the streamed output with a blinking block
/// cursor on the tail. No bubbles — a terminal session, not a chat.
private struct TerminalMessageRow: View {
    let message: MessageRecord
    let isStreamingTail: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if message.role == "user" {
                HStack(alignment: .top, spacing: 8) {
                    Text("❯")
                        .foregroundColor(TerminalTheme.accent)
                    Text(message.content)
                        .foregroundColor(TerminalTheme.text)
                }
            } else {
                // Thought stream — the agent's reasoning, dimmed like
                // commented terminal output.
                if !message.reasoning.isEmpty {
                    VStack(alignment: .leading, spacing: 3) {
                        Text("✻ thinking")
                            .foregroundColor(TerminalTheme.dim)
                        Text(message.reasoning)
                            .font(.system(size: 12, design: .monospaced))
                            .foregroundColor(TerminalTheme.dim)
                    }
                }

                // Tool-call status, shell-command style.
                if let tool = message.toolStatus {
                    HStack(spacing: 6) {
                        Text("$")
                            .foregroundColor(TerminalTheme.accent)
                        Text(tool.text)
                            .foregroundColor(TerminalTheme.text.opacity(0.85))
                            .lineLimit(2)
                        Spacer(minLength: 4)
                        Text(toolGlyph(tool.state))
                            .foregroundColor(tool.state == .failed ? Theme.statusWarning : TerminalTheme.accent)
                    }
                    .font(.system(size: 12, design: .monospaced))
                }

                if !message.content.isEmpty {
                    Text(message.content)
                        .foregroundColor(TerminalTheme.text.opacity(0.85))
                }
                if isStreamingTail {
                    TerminalCursor()
                }
            }
        }
        .font(.system(size: 14, design: .monospaced))
        .textSelection(.enabled)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 16)
    }

    private func toolGlyph(_ state: MessageRecord.ToolStatus.State) -> String {
        switch state {
        case .running: return "…"
        case .done: return "✓"
        case .failed: return "✗"
        }
    }
}

/// Boot-style empty state for code threads — a terminal session header in
/// place of the Home greeting. Read-only output lines: no dangling prompt,
/// so it never looks like a field waiting for input.
private struct TerminalEmptyState: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Image(systemName: "terminal")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundColor(TerminalTheme.accent)
                Text("allternit code — agent terminal")
                    .foregroundColor(TerminalTheme.accent)
            }
            Text("session ready. describe the task below and the agent gets to work.")
                .foregroundColor(TerminalTheme.dim)
        }
        .font(.system(size: 14, design: .monospaced))
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 16)
    }
}

/// Blinking block cursor (terminal `_`-at-rest feel, block style).
private struct TerminalCursor: View {
    @State private var lit = true

    var body: some View {
        Text("▌")
            .foregroundColor(TerminalTheme.text)
            .opacity(lit ? 1 : 0)
            .onAppear {
                withAnimation(.easeInOut(duration: 0.8).repeatForever(autoreverses: true)) {
                    lit = false
                }
            }
    }
}

// MARK: - Gizzi mascot

/// Agent/bot mascot perched on the top edge of the composer bar while agent
/// mode is active. It follows the user's finger: touch and drag along the
/// bar and it walks that way (leaning into the direction), staying where
/// it's left — no autonomous movement.
///
/// When a default bot is selected, the mascot shows that bot's avatar;
/// otherwise it falls back to the classic Gizzi image.
private struct GizziMascotPill: View {
    @EnvironmentObject private var modeStore: AppModeStore
    @EnvironmentObject private var agentModeStore: AgentModeStore

    /// Committed walk position along the bar (points from center).
    @State private var walkOffset: CGFloat = 0
    /// Live drag translation while a finger is down.
    @GestureState private var dragOffset: CGFloat = 0

    /// Farthest the mascot may roam from center — keeps it on the composer card.
    private let maxWalk: CGFloat = 150
    /// Resting perch, near the card's right corner: clear of the top-deck
    /// pills that hug the card's leading edge when agent mode is on.
    private let restX: CGFloat = 140

    private var totalOffset: CGFloat {
        restX + min(max(walkOffset + dragOffset, -maxWalk - restX), maxWalk - restX)
    }

    private var selectedAgent: AgentRecord? {
        agentModeStore.selectedAgent(for: modeStore.mode)
    }

    var body: some View {
        Group {
            if let agent = selectedAgent {
                AgentAvatarView(agent: agent, size: 44)
                    .shadow(color: .black.opacity(0.1), radius: 4, x: 0, y: 2)
            } else {
                Image("GizziMascot")
                    .resizable()
                    .scaledToFit()
                    .frame(width: 44, height: 44)
            }
        }
        // Lean into the walk direction while being dragged.
        .rotationEffect(.degrees(dragOffset == 0 ? 0 : (dragOffset > 0 ? 7 : -7)))
        // Feet sit on the top edge of the card (a few points inside so it
        // looks grounded, not hovering).
        .offset(x: totalOffset, y: -38)
        .contentShape(Rectangle())
        .gesture(
            DragGesture(minimumDistance: 3)
                .updating($dragOffset) { value, state, _ in
                    state = value.translation.width
                }
                .onEnded { value in
                    withAnimation(.spring(response: 0.4, dampingFraction: 0.72)) {
                        walkOffset = min(max(walkOffset + value.translation.width, -maxWalk - restX), maxWalk - restX)
                    }
                }
        )
        .transition(.scale.combined(with: .opacity))
        .accessibilityLabel(selectedAgent?.name ?? "Gizzi")
    }
}
