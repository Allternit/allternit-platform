import SwiftUI

// MARK: - Chat surface (Home)

/// Home chrome (allternit-ai src/fabric-session/phone/screens/ChatsScreen.tsx):
/// the phone header — ☰, the open chat's name (else "Allternit"), the
/// open-chats count button and New chat — above the shared chat content.
/// The Code tab hosts the same `ChatContentView` inside its own navigation
/// chrome (CodeModeView), so code threads reuse the chat UI.
struct ChatView: View {
    @Binding var selectedSessionId: String?
    @Binding var isSidebarOpen: Bool

    @StateObject private var viewModel = ChatViewModel()
    @ObservedObject private var sessions = ShellSessionsStore.shared

    /// The open chat's name, else "Allternit" (ChatsScreen `activeChat?.name`).
    private var title: String {
        let id = viewModel.currentSessionId ?? selectedSessionId
        if let name = sessions.session(id: id)?.name, !name.isEmpty { return name }
        return "Allternit"
    }

    var body: some View {
        VStack(spacing: 0) {
            PhoneHeader(
                title: title,
                wideSides: true,
                onMenu: { isSidebarOpen = true }
            ) {
                PhoneIconButton(systemName: "square.and.pencil", label: "New chat") {
                    startNewChat()
                }
            }
            .zIndex(1)

            ChatContentView(
                sessionId: selectedSessionId,
                viewModel: viewModel,
                launchStyle: true
            )
        }
        .background(Color("BgPrimary"))
        // The drawer's New chat / New pill: start fresh even when the
        // selection was already nil (a just-sent chat has no selection).
        .onReceive(NotificationCenter.default.publisher(for: .shellNewChat)) { _ in
            startNewChat()
        }
        // A sent first message names the chat (after its first reply): pick
        // up the new title when a turn settles.
        .task(id: "\(viewModel.currentSessionId ?? "")|\(viewModel.isStreaming)") {
            // Drawer rows: this chat shows "Working" while it streams and is never unread.
            sessions.setActive(viewModel.currentSessionId, streaming: viewModel.isStreaming)
            guard !viewModel.isStreaming else { return }
            if sessions.session(id: viewModel.currentSessionId) == nil || viewModel.currentSessionId != nil {
                await sessions.load()
                if let id = viewModel.currentSessionId { sessions.markSeen(id) }
            }
        }
        .onDisappear { sessions.setActive(nil, streaming: viewModel.isStreaming) }
    }

    private func startNewChat() {
        selectedSessionId = nil
        viewModel.startNewSession()
    }
}

// MARK: - Shared chat content

/// The chat experience itself — the launch screen or the message feed, and
/// the composer — with no host chrome. The host owns the `ChatViewModel`
/// (as @StateObject) so its chrome can drive it, and supplies the session id
/// to load. Session-create context derives from the current app mode, so
/// hosting this under `.code` stamps new sessions with `origin_surface='code'`.
struct ChatContentView: View {
    let sessionId: String?
    @ObservedObject var viewModel: ChatViewModel
    /// Home: the web chat launch screen (greeting, composer with its full
    /// deck, the template gallery) and the compact deck in a session. Other
    /// hosts get the plain composer.
    var launchStyle: Bool = false
    /// Code's Terminal choice: this same thread drawn as a terminal session
    /// (dark surface, `❯` prompts, `●` tool lines) with the Code prompt and
    /// its status row in place of the chat composer.
    var terminalLook: Bool = false
    /// The working folder and branch for the terminal's header and status row.
    var codeFolder: CodeFolderInfo? = nil
    /// The status row's "Chat" item on the terminal view.
    var onLeaveTerminal: (() -> Void)? = nil
    /// The status row's "Terminal" item in a Code thread's chat view.
    var onEnterTerminal: (() -> Void)? = nil
    /// A bot thread's tabs, in the tray above the composer card
    /// (BotChatSessionView's BotThreadTopDeck); the compact deck rides under it.
    var composerTopDeck: AnyView? = nil
    /// The composer's hint in place of "Reply…" ("Message Goldie…").
    var composerPlaceholder: String? = nil

    @EnvironmentObject private var modeStore: AppModeStore
    @EnvironmentObject private var agentModeStore: AgentModeStore
    @Environment(\.chatBot) private var chatBot
    /// A background (StageBackdropView) is drawn behind this chat: the page fills step aside.
    @Environment(\.backdropUnderneath) private var backdropUnderneath
    @StateObject private var modelStore = ModelStore.shared
    /// Selected project — stamped into the next session's create metadata
    /// (`metadata.projectId`) so the Projects feature can group it.
    @StateObject private var projectStore = ProjectStore.shared
    @ObservedObject private var permissionStore = ChatPermissionStore.shared
    /// Staged composer attachments — owned here (next to the view model that
    /// uploads them) and handed to the composer for the pickers and previews.
    @StateObject private var attachmentStore = AttachmentStore()
    @ObservedObject private var usageStore = UsageStore.shared
    @State private var inputText: String = ""
    @StateObject private var voiceHost = VoiceHost()
    /// Edit-resend: id of the last user bubble whose "Edit" filled the
    /// composer; non-nil routes the next send through
    /// `ChatViewModel.resendEditedMessage` (truncate + re-send).
    @State private var editingMessageId: String? = nil
    @State private var isCodePlusSheetPresented = false
    @State private var activeArtifact: ArtifactRecord? = nil
    @State private var isModelPickerPresented = false
    @Environment(\.scenePhase) private var scenePhase

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
            persona: OnboardingStore.shared.persona?.rawValue,
            permissionMode: (mode == .chat || mode == .cowork) ? permissionStore.pendingMode.rawValue : nil
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

    /// Code's terminal view renders the thread as a terminal session (dark,
    /// monospace, `❯` prompts) instead of chat messages.
    private var isTerminal: Bool { terminalLook }

    /// A Code thread (chat view): it keeps the Code prompt.
    private var isCodeThread: Bool { modeStore.mode == .code && onEnterTerminal != nil }

    private var showsLaunch: Bool {
        launchStyle && viewModel.messages.isEmpty && !isTerminal
    }

    var body: some View {
        Group {
            if showsLaunch {
                launchLayout
            } else {
                sessionLayout
            }
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
                    // conversation itself is already in the thread.
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
            // An onboarding starter-task card stashes its prompt in
            // OnboardingStore.pendingPrompt — fill the composer with it once.
            if let prompt = OnboardingStore.shared.pendingPrompt {
                inputText = prompt
                OnboardingStore.shared.pendingPrompt = nil
            }
            #if DEBUG
            // `-stage-test-attachment` (DEBUG only): stages a generated image
            // so the composer previews and the upload path can be verified.
            if launchArgumentEnabled("stage-test-attachment") {
                stageTestAttachment()
            }
            // `-open-voice-mode` (DEBUG only): presents voice mode (idle).
            if launchArgumentEnabled("open-voice-mode") || launchArgumentEnabled("open-voice-settings") {
                voiceHost.start(chat: viewModel, runtimeModelId: modelStore.selectedModelId,
                                effort: modelStore.effortForSend, presentation: .full)
            }
            // `-voice-summary <seconds>` (DEBUG only): files a "Voice chat
            // ended · Ns" card into the feed.
            if let raw = UserDefaults.standard.string(forKey: "voice-summary"),
               let seconds = Int(raw), viewModel.messages.isEmpty {
                viewModel.appendVoiceSummary(durationSeconds: seconds)
            }
            // `-autosend <text>` (DEBUG only): sends one message on appear.
            if let text = UserDefaults.standard.string(forKey: "autosend"),
               sessionId == nil, viewModel.messages.isEmpty {
                viewModel.sendMessage(text, attachments: attachmentStore.attachments, runtimeModelId: modelForSend, effort: modelStore.effortForSend)
                attachmentStore.clear()
            }
            #endif
        }
        .onChange(of: sessionContext) { _, newContext in
            // Composer controls changed pre-session: the next create is
            // stamped with the new context.
            viewModel.sessionContext = newContext
        }
        .onChange(of: viewModel.currentSessionId) { _, newId in
            // A chat this composer just started took the pending permission
            // choice on its create; remember it for the pill.
            guard let newId, newId != sessionId else { return }
            permissionStore.bind(sessionId: newId, mode: permissionStore.pendingMode)
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

    // MARK: Launch screen

    /// Web chat launch on the phone: the greeting sits in the middle of the
    /// space above the composer; the composer with its full deck rests at
    /// the bottom, and the template gallery (hidden once there's a draft)
    /// scrolls below it.
    private var launchLayout: some View {
        GeometryReader { proxy in
            ScrollView {
                VStack(spacing: 0) {
                    if let transientError = viewModel.transientError {
                        TransientErrorBanner(message: transientError, onDismiss: {
                            viewModel.transientError = nil
                        })
                    }
                    Spacer(minLength: 16)
                    ChatLaunchGreeting()
                    Spacer(minLength: 16)
                    composer(deck: .full)
                    // Templates stay hidden until a Docs, Slides, Sites or
                    // Sheets mode is picked in Agent mode; then that mode's
                    // templates show.
                    if inputText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                       agentModeStore.isAgentEnabled(for: modeStore.mode),
                       let tile = agentModeStore.selectedTile(for: modeStore.mode) as AgentModeTile?,
                       ArtifactModeGallery.tabs.contains(where: { $0.mode == tile }) {
                        ArtifactModeGallery(
                            preferredMode: tile,
                            showsTabs: false,
                            onSelectTemplate: { template in
                                agentModeStore.selectTile(template.mode, for: modeStore.mode)
                                agentModeStore.setAgentEnabled(true, for: modeStore.mode)
                                inputText = template.prompt
                            }
                        )
                        .padding(.top, 24)
                        .padding(.bottom, 16)
                        .transition(.opacity)
                    }
                }
                .padding(.horizontal, 14)
                .padding(.bottom, 8)
                .frame(minHeight: proxy.size.height)
                .animation(.easeOut(duration: 0.2), value: inputText.isEmpty)
                .animation(.easeOut(duration: 0.2), value: agentModeStore.isAgentEnabled(for: modeStore.mode))
            }
            .scrollDismissesKeyboard(.interactively)
            .background { BotWashedBackground().ignoresSafeArea() }
        }
    }

    // MARK: Session

    private var sessionLayout: some View {
        VStack(spacing: 0) {
            // Transient load/send failure banner (dismissible).
            if let transientError = viewModel.transientError {
                TransientErrorBanner(message: transientError, onDismiss: {
                    viewModel.transientError = nil
                })
            }

            ScrollViewReader { scrollProxy in
                ScrollView {
                    LazyVStack(spacing: 20) {
                        if viewModel.messages.isEmpty {
                            if isTerminal {
                                // Code thread empty state: terminal boot header.
                                TerminalEmptyState(folder: codeFolder)
                                    .padding(.top, 24)
                            } else {
                                ChatLaunchGreeting()
                                    .padding(.top, 60)
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
                    }
                    .padding(.vertical, 16)
                }
                .scrollDismissesKeyboard(.interactively)
                .onChange(of: viewModel.messages.last) { _, _ in
                    // Fires on new messages AND on streamed content changes;
                    // the view model's ~50ms flush coalescing bounds the rate.
                    guard let lastId = viewModel.messages.last?.id else { return }
                    // Animated scroll lags behind rapid streaming deltas.
                    if viewModel.isStreaming {
                        scrollProxy.scrollTo(lastId, anchor: .bottom)
                    } else {
                        withAnimation { scrollProxy.scrollTo(lastId, anchor: .bottom) }
                    }
                }
            }

            if editingMessageId != nil {
                editingBanner
            }

            // Quick voice bar while a voice conversation runs.
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

            if isTerminal {
                codeComposer(terminal: true)
            } else if isCodeThread {
                // Code threads keep the Code prompt (the same one as the Code
                // home: `❯` line, dashed rules, the full status row).
                codeComposer(terminal: false)
            } else {
                composer(deck: launchStyle || composerTopDeck != nil ? .compact : .none)
                    .padding(.horizontal, 14)
                    .padding(.top, 6)
                    .padding(.bottom, 8)
                    .background(backdropUnderneath ? Color.clear : Color("BgPrimary"))
            }
        }
        // A bot's chat wears its colour wash at the top (BotColorWash);
        // elsewhere this is the plain page.
        .background {
            if isTerminal { (backdropUnderneath ? Color.clear : TerminalPalette.background).ignoresSafeArea() } else { BotWashedBackground().ignoresSafeArea() }
        }
    }

    /// The Code prompt: the `❯` line and its status row, sending into this
    /// thread with any staged files; Stop while a turn runs. On the terminal
    /// view it is drawn on the dark surface.
    private func codeComposer(terminal: Bool) -> some View {
        VStack(spacing: 0) {
            if !attachmentStore.attachments.isEmpty {
                CodeStagedFiles(store: attachmentStore, terminal: terminal)
            }
            CodeLaunchComposer(
                text: $inputText,
                folder: codeFolder,
                terminal: terminal,
                isRunning: viewModel.isStreaming,
                onStop: { viewModel.stopStreaming() },
                onSend: { text in
                    viewModel.sendMessage(text, attachments: attachmentStore.attachments, runtimeModelId: modelForSend, effort: modelStore.effortForSend)
                    attachmentStore.clear()
                },
                onOpenTerminal: { terminal ? onLeaveTerminal?() : onEnterTerminal?() },
                onAttach: { isCodePlusSheetPresented = true },
                sessionId: viewModel.currentSessionId,
                // A landed turn (count moves, streaming stops) refreshes the meter.
                contextRefresh: viewModel.messages.count * 2 + (viewModel.isStreaming ? 1 : 0)
            )
        }
        .padding(.bottom, 4)
        .background(backdropUnderneath ? Color.clear : (terminal ? TerminalPalette.background : Color("BgPrimary")))
        .sheet(isPresented: $isCodePlusSheetPresented) {
            ComposerPlusSheet(attachmentStore: attachmentStore, sessionId: viewModel.currentSessionId)
        }
    }

    private func composer(deck: ComposerDeckVariant) -> some View {
        ComposerView(
            inputText: $inputText,
            attachmentStore: attachmentStore,
            isStreaming: viewModel.isStreaming,
            isSendDisabled: viewModel.isCreatingSession,
            hasActiveSession: viewModel.currentSessionId != nil,
            sessionId: viewModel.currentSessionId,
            // Hard usage wall: at/above 100% the composer locks until the
            // window resets or the user adds credits / upgrades.
            isUsageLocked: usageStore.isAtLimit,
            isVoiceModeEnabled: !viewModel.isStreaming && !usageStore.isAtLimit,
            deck: deck,
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
            onVoiceMode: {
                voiceHost.start(chat: viewModel, runtimeModelId: modelStore.selectedModelId,
                                effort: modelStore.effortForSend, presentation: .bar)
            },
            onAgentModeChanged: { isOn in
                if isOn, viewModel.currentSessionId == nil, viewModel.messages.isEmpty {
                    // A fresh session context so the first message creates a
                    // session under the picked mode.
                    viewModel.startNewSession()
                }
            },
            placeholderOverride: composerPlaceholder,
            // A bot thread's composer (ThreadComposer) has no permission pill.
            showsPermission: chatBot == nil,
            topDeck: composerTopDeck
        )
    }

    /// One feed row — extracted from `body` so the view stays under the
    /// type-checker's expression budget.
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
                // "Edit" on the last user message: the composer loads the
                // text; sending truncates + re-sends (onSend above).
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

    /// The "editing" strip above the composer: the last user message's
    /// "Edit" loaded its text — sending truncates the conversation after it
    /// and re-sends; X cancels back to a normal draft.
    private var editingBanner: some View {
        HStack(spacing: 8) {
            Image(systemName: "pencil")
                .font(.system(size: 13))
            Text("Editing message — sending replaces everything after it")
                .font(.system(size: 13))
                .lineLimit(1)
            Spacer()
            Button(action: {
                editingMessageId = nil
                inputText = ""
            }) {
                Image(systemName: "xmark")
                    .font(.system(size: 12, weight: .semibold))
                    .frame(width: 32, height: 32)
            }
            .accessibilityLabel("Cancel edit")
        }
        .foregroundColor(Color("TextSecondary"))
        .padding(.leading, 16)
        .padding(.trailing, 8)
        .background(Color("BgPrimary"))
    }

    #if DEBUG
    /// `-stage-test-attachment`: programmatically generate a 400x300 image
    /// and stage it — no photo-library fixture needed.
    private func stageTestAttachment() {
        let size = CGSize(width: 400, height: 300)
        let image = UIGraphicsImageRenderer(size: size).image { context in
            UIColor(named: "AccentPrimary")?.setFill()
            context.fill(CGRect(origin: .zero, size: size))
            "TEST".draw(at: CGPoint(x: 170, y: 140), withAttributes: [
                .font: UIFont.boldSystemFont(ofSize: 28),
                .foregroundColor: UIColor(named: "OnAccent") ?? UIColor.white,
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

// MARK: - Launch greeting

/// The launch greeting (web LaunchHeader on the phone): the Gizzi mark over
/// a serif title, centered. One greeting per app session, from the web's
/// chat pool (launchGreeting.ts), with the signed-in first name.
struct ChatLaunchGreeting: View {
    /// In a bot's own chat the bot's face stands in for Gizzi (web
    /// ActiveAgentMascot on the launch header).
    @Environment(\.chatBot) private var bot
    /// The bot stands on its stage above this chat: no second face here.
    @Environment(\.chatBotOnStage) private var botOnStage
    @State private var revealed = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    @MainActor private static var cachedTitle: String?
    @MainActor private static var didAnimate = false

    @MainActor
    private static func sessionTitle() -> String {
        if let cachedTitle { return cachedTitle }
        let name = AuthManager.shared.firstName
        let now = Date()
        let weekday = now.formatted(.dateTime.weekday(.wide))
        let hour = Calendar.current.component(.hour, from: now)
        let timeOfDay = hour < 5 ? "Up late" : hour < 12 ? "Good morning" : hour < 18 ? "Good afternoon" : "Good evening"
        func withName(_ base: String) -> String { name.map { "\(base), \($0)" } ?? base }
        let pool = [
            withName("Happy \(weekday)"),
            withName(timeOfDay),
            name.map { "What's on your mind, \($0)?" } ?? "What's on your mind?",
            name.map { "How can I help, \($0)?" } ?? "How can I help you today?",
            withName("Welcome back"),
            "What should we get done?",
            name.map { "What are we working on, \($0)?" } ?? "What are we working on?",
        ]
        let picked = pool.randomElement() ?? "How can I help you today?"
        cachedTitle = picked
        return picked
    }

    var body: some View {
        if botOnStage, let bot {
            // On the stage the bot is already big above: a quiet line only
            // (BotChatSessionView's empty state, onStage).
            VStack(spacing: 4) {
                Text("Chat with \(bot.botDisplayName)")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                Text("Ask anything, or hand it a file.")
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextSecondary"))
            }
            .frame(maxWidth: .infinity)
            .padding(.top, -44)
        } else {
            greeting
        }
    }

    private var greeting: some View {
        VStack(spacing: 10) {
            Group {
                if let bot {
                    BotAvatarView(bot: bot, size: 64, live: true)
                } else {
                    Image("GizziMascot")
                        .resizable()
                        .scaledToFit()
                        .frame(width: 44, height: 44)
                }
            }
            .accessibilityHidden(true)
            Text(Self.sessionTitle())
                .font(.system(size: 32, design: .serif))
                .tracking(-0.64)
                .lineSpacing(2)
                .foregroundColor(Color("TextPrimary"))
                .multilineTextAlignment(.center)
                .opacity(revealed ? 1 : 0)
                .offset(y: revealed ? 0 : 8)
        }
        .frame(maxWidth: .infinity)
        // The web keeps the old tagline row's gap under the title.
        .padding(.bottom, 44)
        .onAppear {
            // The reveal plays only on the first launch-header mount.
            guard !revealed else { return }
            let firstMount = !ChatLaunchGreeting.didAnimate
            ChatLaunchGreeting.didAnimate = true
            if reduceMotion || !firstMount {
                revealed = true
            } else {
                withAnimation(.easeOut(duration: 0.6)) { revealed = true }
            }
        }
    }
}

private struct ChatBotKey: EnvironmentKey {
    static let defaultValue: AgentRecord? = nil
}

private struct ChatBotOnStageKey: EnvironmentKey {
    static let defaultValue = false
}

extension EnvironmentValues {
    /// The bot whose chat this is (nil outside Bots).
    var chatBot: AgentRecord? {
        get { self[ChatBotKey.self] }
        set { self[ChatBotKey.self] = newValue }
    }

    /// True while the bot stands on its live stage over its chat.
    var chatBotOnStage: Bool {
        get { self[ChatBotOnStageKey.self] }
        set { self[ChatBotOnStageKey.self] = newValue }
    }
}

/// Files staged for the next Code prompt: one line each, X to drop it.
struct CodeStagedFiles: View {
    @ObservedObject var store: AttachmentStore
    var terminal: Bool = false

    var body: some View {
        let ink = CodeInk(terminal: terminal)
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                ForEach(store.attachments) { file in
                    HStack(spacing: 6) {
                        Image(systemName: file.thumbnail == nil ? "doc" : "photo").font(.system(size: 12))
                        Text(file.filename).lineLimit(1).frame(maxWidth: 160, alignment: .leading)
                        Button(action: { store.remove(id: file.id) }) {
                            Image(systemName: "xmark").font(.system(size: 10, weight: .bold))
                                .frame(width: 22, height: 22)
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Remove \(file.filename)")
                    }
                    .font(.system(size: 13, design: .monospaced))
                    .foregroundColor(ink.secondary)
                    .padding(.leading, 8)
                    .background(ink.fill, in: RoundedRectangle(cornerRadius: 6))
                }
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 6)
        }
    }
}

/// Dismissible one-line banner for transient load/send failures
/// (ChatViewModel.transientError), pinned to the top of the feed.
struct TransientErrorBanner: View {
    let message: String
    let onDismiss: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: "exclamationmark.triangle")
                .font(.system(size: 14))
                .foregroundColor(Theme.statusError)

            Text(message)
                .font(.system(size: 14))
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(2)

            Spacer(minLength: 8)

            Button(action: onDismiss) {
                Image(systemName: "xmark")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 32, height: 32)
            }
            .accessibilityLabel("Dismiss")
        }
        .padding(.leading, 12)
        .padding(.trailing, 4)
        .padding(.vertical, 4)
        .background(Color("FillNeutral"), in: RoundedRectangle(cornerRadius: 12))
        .padding(.horizontal, 14)
        .padding(.top, 8)
    }
}

// MARK: - Terminal session (code threads)

/// Colours of the thread's terminal view: Terminal.app's dark profile (the
/// shell's palette) with the gizzi coral for prompts and live work.
@MainActor
private enum TerminalInk {
    static let text = TerminalPalette.foreground
    static let dim = TerminalPalette.secondary
    static let coral = GizziCodeWordmark.coral
    static let ok = Color(hex: "#7EC699")
    static let error = Color(hex: "#F2777A")
    static let font = Font.system(size: 15, design: .monospaced)
    static let small = Font.system(size: 13, design: .monospaced)
    /// 15pt text on an 18pt line (line height 1.2, like the shell).
    static let lineSpacing: CGFloat = 3
}

/// One turn of a code thread drawn the way gizzi code (and Claude Code)
/// prints it: `❯ prompt` for the user; for the agent a dim `✻ Thinking`
/// block, each tool call as `● Tool(arg)` with a `⎿` result line, the reply
/// text behind `●`, code fences indented and dimmed, errors in red, and a
/// `✻ Working… 12s` line while the turn runs.
private struct TerminalMessageRow: View {
    let message: MessageRecord
    let isStreamingTail: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if message.role == "user" {
                bullet("❯", color: TerminalInk.coral, weight: .bold) {
                    Text(message.content)
                        .foregroundColor(TerminalInk.text)
                }
            } else {
                if !message.reasoning.isEmpty {
                    bullet("✻", color: TerminalInk.dim) {
                        Text(message.reasoning.trimmingCharacters(in: .whitespacesAndNewlines))
                            .italic()
                            .foregroundColor(TerminalInk.dim)
                            .lineLimit(isStreamingTail && message.content.isEmpty ? nil : 4)
                    }
                }

                ForEach(message.toolCalls.filter { !$0.isTodo }) { call in
                    TerminalToolLines(call: call)
                }
                if message.toolCalls.isEmpty, let status = message.toolStatus {
                    TerminalToolLines(status: status)
                }

                if !message.content.isEmpty {
                    bullet("●", color: TerminalInk.text) {
                        TerminalReplyText(text: message.content)
                    }
                }

                if let error = message.error {
                    bullet("●", color: TerminalInk.error) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(error.title)
                                .foregroundColor(TerminalInk.error)
                            if !error.detail.isEmpty {
                                Text("⎿  \(error.detail)")
                                    .font(TerminalInk.small)
                                    .foregroundColor(TerminalInk.dim)
                            }
                        }
                    }
                }

                if isStreamingTail {
                    TerminalWorkingLine(since: message.toolCalls.first?.startedAt)
                }
            }
        }
        .font(TerminalInk.font)
        .lineSpacing(TerminalInk.lineSpacing)
        .textSelection(.enabled)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 14)
    }

    private func bullet<Content: View>(_ glyph: String, color: Color, weight: Font.Weight = .regular, @ViewBuilder content: () -> Content) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(glyph)
                .font(.system(size: 15, weight: weight, design: .monospaced))
                .foregroundColor(color)
                .accessibilityHidden(true)
            content()
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

/// `● Tool(arg)` and its `⎿` result line: coral dot while it runs, green
/// when done, red when it failed (with the error).
private struct TerminalToolLines: View {
    let name: String
    let arg: String
    let state: ToolCallRecord.State
    let result: String
    let startedAt: Date?

    init(call: ToolCallRecord) {
        let summary = call.summary
        name = Self.cliName(call)
        let detail = summary.detail.isEmpty ? summary.verb : summary.detail
        arg = detail
        state = call.state
        startedAt = call.startedAt
        switch call.state {
        case .failed:
            result = call.error.flatMap { $0.split(separator: "\n").first.map(String.init) } ?? "Failed"
        case .done:
            let elapsed = call.elapsed().map { " · \(Self.duration($0))" } ?? ""
            result = (summary.detail.isEmpty ? "Done" : summary.verb) + elapsed
        case .running:
            result = summary.verb
        }
    }

    init(status: MessageRecord.ToolStatus) {
        name = status.toolName.isEmpty ? "Tool" : status.toolName
        arg = status.text
        switch status.state {
        case .running: state = .running
        case .done: state = .done
        case .failed: state = .failed
        }
        result = status.state == .failed ? "Failed" : (status.elapsed.map { "Done · \(Self.duration($0))" } ?? (status.state == .running ? "Running" : "Done"))
        startedAt = status.startedAt
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text("●")
                    .foregroundColor(dotColor)
                    .accessibilityHidden(true)
                (Text(name).bold().foregroundColor(TerminalInk.text)
                 + Text(arg.isEmpty ? "" : "(\(arg))").foregroundColor(TerminalInk.text.opacity(0.85)))
                    .lineLimit(3)
            }
            HStack(alignment: .firstTextBaseline, spacing: 0) {
                Text("  ⎿  ")
                    .foregroundColor(TerminalInk.dim)
                if state == .running {
                    TimelineView(.periodic(from: .now, by: 1)) { context in
                        Text(runningText(now: context.date))
                            .foregroundColor(TerminalInk.dim)
                    }
                } else {
                    Text(result)
                        .foregroundColor(state == .failed ? TerminalInk.error : TerminalInk.dim)
                        .lineLimit(2)
                }
            }
            .font(TerminalInk.small)
        }
        .accessibilityElement(children: .combine)
    }

    private var dotColor: Color {
        switch state {
        case .running: return TerminalInk.coral
        case .done: return TerminalInk.ok
        case .failed: return TerminalInk.error
        }
    }

    private func runningText(now: Date) -> String {
        guard let startedAt else { return "\(result)…" }
        return "\(result)… \(Self.duration(now.timeIntervalSince(startedAt)))"
    }

    /// The CLI's tool names (Bash, Read, Edit, …) from the runtime's name.
    static func cliName(_ call: ToolCallRecord) -> String {
        let name = ToolDescriber.normalized(call.toolName)
        if call.isAgentStep { return "Task" }
        if name.contains("bash") || name.contains("shell") || name.contains("exec") || name.contains("terminal") { return "Bash" }
        if name.contains("multiedit") || name.contains("strreplace") || name.contains("editfile") || name == "edit" || name == "patch" { return "Edit" }
        if name.contains("writefile") || name.contains("createfile") || name == "write" { return "Write" }
        if name.contains("readfile") || name == "read" || name == "view" { return "Read" }
        if name.contains("grep") || name.contains("searchcode") { return "Grep" }
        if name.contains("glob") || name.contains("findfiles") || name.contains("listfiles") || name == "ls" || name == "list" { return "Glob" }
        if name.contains("websearch") || name.contains("googlesearch") || name == "search" { return "WebSearch" }
        if name.contains("fetch") || name.contains("readurl") || name.contains("webpage") { return "WebFetch" }
        var raw = call.toolName
        if raw.lowercased().hasPrefix("tool-") { raw.removeFirst(5) }
        if let range = raw.range(of: "__", options: .backwards) { raw = String(raw[range.upperBound...]) }
        return raw.isEmpty ? "Tool" : raw
    }

    static func duration(_ seconds: TimeInterval) -> String {
        if seconds < 10 { return String(format: "%.1fs", seconds) }
        if seconds < 60 { return "\(Int(seconds))s" }
        return "\(Int(seconds) / 60)m \(Int(seconds) % 60)s"
    }
}

/// The reply as terminal output: prose with inline markdown (bold, `code`),
/// fenced code blocks indented and dimmed behind a rule.
private struct TerminalReplyText: View {
    let text: String

    private struct Block: Identifiable {
        let id: Int
        let code: Bool
        let text: String
    }

    private var blocks: [Block] {
        var result: [Block] = []
        var current: [Substring] = []
        var inCode = false
        func flush() {
            let joined = current.joined(separator: "\n").trimmingCharacters(in: .newlines)
            if !joined.isEmpty { result.append(Block(id: result.count, code: inCode, text: joined)) }
            current = []
        }
        for line in text.split(separator: "\n", omittingEmptySubsequences: false) {
            if line.trimmingCharacters(in: .whitespaces).hasPrefix("```") {
                flush()
                inCode.toggle()
            } else {
                current.append(line)
            }
        }
        flush()
        return result
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            ForEach(blocks) { block in
                if block.code {
                    Text(block.text)
                        .font(TerminalInk.small)
                        .foregroundColor(TerminalInk.dim)
                        .padding(.leading, 10)
                        .overlay(alignment: .leading) {
                            Rectangle().fill(TerminalPalette.line).frame(width: 2)
                        }
                } else {
                    Text(Self.inline(block.text))
                        .foregroundColor(TerminalInk.text)
                }
            }
        }
    }

    private static func inline(_ text: String) -> AttributedString {
        let options = AttributedString.MarkdownParsingOptions(interpretedSyntax: .inlineOnlyPreservingWhitespace)
        return (try? AttributedString(markdown: text, options: options)) ?? AttributedString(text)
    }
}

/// `✻ Working… 12s` while the turn runs, the star pulsing (still under
/// Reduce Motion).
private struct TerminalWorkingLine: View {
    let since: Date?
    @State private var start = Date()
    @State private var lit = true
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        TimelineView(.periodic(from: .now, by: 1)) { context in
            HStack(spacing: 8) {
                Text("✻")
                    .foregroundColor(TerminalInk.coral)
                    .opacity(lit ? 1 : 0.35)
                Text("Working… \(TerminalToolLines.duration(context.date.timeIntervalSince(since ?? start)))")
                    .foregroundColor(TerminalInk.coral)
            }
        }
        .accessibilityLabel("Working")
        .onAppear {
            guard !reduceMotion else { return }
            withAnimation(.easeInOut(duration: 0.8).repeatForever(autoreverses: true)) { lit = false }
        }
    }
}

/// The terminal view of a thread with no turns yet: the GIZZI CODE mark, the
/// working folder and branch, and a hint, like the gizzi TUI's boot screen.
private struct TerminalEmptyState: View {
    let folder: CodeFolderInfo?

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            GizziCodeWordmark(height: 15)
                .environment(\.colorScheme, .dark)
                .accessibilityLabel("Gizzi Code")
            if let folder {
                Text(folder.branch.map { "\(folder.directory) · \($0)" } ?? folder.directory)
                    .foregroundColor(TerminalInk.dim)
                    .lineLimit(2)
                    .truncationMode(.head)
            }
            Text("Describe a task or ask about the code. The agent works in this thread.")
                .foregroundColor(TerminalInk.dim)
        }
        .font(TerminalInk.small)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 14)
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
