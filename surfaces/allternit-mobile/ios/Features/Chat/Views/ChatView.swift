import SwiftUI

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
        // The drawer's New chat / New pill: start fresh even when the
        // selection was already nil (a just-sent chat has no selection).
        .onReceive(NotificationCenter.default.publisher(for: .shellNewChat)) { _ in
            selectedSessionId = nil
            viewModel.startNewSession()
            viewModel.isTemporaryChat = false
        }
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
            },
            sessionId: viewModel.currentSessionId,
            onSendPrompt: { prompt in
                viewModel.sendMessage(prompt, runtimeModelId: modelStore.selectedModelId, effort: modelStore.effortForSend)
            }
        )
        .id(message.id)
    }

    /// Terminal-styled feed row for code threads — see TerminalMessageRow.
    private func terminalRow(_ message: MessageRecord) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            TerminalMessageRow(
                message: message,
                isStreamingTail: viewModel.isStreaming && message.id == viewModel.messages.last?.id
            )
            if message.role == "assistant" {
                TerminalQuestionCards(
                    message: message,
                    sessionId: viewModel.currentSessionId,
                    isLast: message.id == viewModel.messages.last(where: { $0.role == "assistant" })?.id
                )
            }
        }
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
                        // Shown on the US storefront only (StorefrontGate).
                        usageSafariURL = IdentifiableURL(url: AccountPlanStore.shared.upgradeURL)
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
                                // Buttons show on the US storefront only
                                // (StorefrontGate); elsewhere the card
                                // shows the reset time alone.
                                onAddCredits: {
                                    usageSafariURL = IdentifiableURL(url: StorefrontGate.creditsURL)
                                },
                                onGetPro: {
                                    usageSafariURL = IdentifiableURL(url: AccountPlanStore.shared.upgradeURL)
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
        // A prompt stashed while this chat is already on screen (the Meet
        // your bots intro's first job) fills the composer too.
        .onReceive(OnboardingStore.shared.$pendingPrompt.compactMap { $0 }) { prompt in
            inputText = prompt
            DispatchQueue.main.async { OnboardingStore.shared.pendingPrompt = nil }
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
struct GizziMascotPill: View {
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

/// Questions the agent asked in a code thread, answered in place under the
/// terminal row (the chat feed's MessageRow does the same): the reply's own
/// questions, plus on the latest reply any the session still lists.
private struct TerminalQuestionCards: View {
    let message: MessageRecord
    let sessionId: String?
    let isLast: Bool

    @ObservedObject private var asks = AgentAskStore.shared

    var body: some View {
        ForEach(questions) { request in
            QuestionCardView(request: request)
        }
        .task(id: "\(sessionId ?? "")|\(isLast)|\(message.isStreaming)") {
            guard isLast, !message.isStreaming, let sessionId else { return }
            await asks.refresh(sessionId: sessionId)
        }
    }

    private var questions: [AgentQuestionRequest] {
        var list = message.questions
        if isLast {
            for pending in asks.questions(for: sessionId) where !list.contains(where: { $0.id == pending.id }) {
                list.append(pending)
            }
        }
        return list.filter { !asks.isResolved($0.id) || message.questions.contains($0) }
    }
}
