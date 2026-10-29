import SwiftUI

// MARK: - Chat surface (Home)

/// Home (web ChatsScreen): the phone header — ☰ menu, the chat's title (or
/// "Allternit"), the open-chats count and New chat — over the shared chat
/// content. The Code tab hosts the same `ChatContentView` inside its own
/// chrome (CodeModeView), so code threads reuse the chat UI.
struct ChatView: View {
    @Binding var selectedSessionId: String?
    @Binding var isSidebarOpen: Bool

    @StateObject private var viewModel = ChatViewModel()
    /// Chat sessions, for the header's title and open-chats count.
    @State private var sessions: [AgentSession] = []

    private var title: String {
        guard let id = viewModel.currentSessionId ?? selectedSessionId,
              let name = sessions.first(where: { $0.id == id })?.name,
              !name.isEmpty else { return "Allternit" }
        return name
    }

    /// Chats only (Cowork folded into Chat): no code, bot or browser threads.
    private var openChatCount: Int {
        sessions.filter { session in
            guard let surface = session.originSurface else { return true }
            return surface == "chat" || surface == "cowork"
        }.count
    }

    var body: some View {
        VStack(spacing: 0) {
            HomeHeader(
                title: title,
                onMenu: { isSidebarOpen.toggle() },
                trailing: {
                    HStack(spacing: 2) {
                        HomeOverviewButton(count: openChatCount, working: viewModel.isStreaming) {
                            // The app has no session overview yet; the drawer
                            // lists every chat.
                            isSidebarOpen = true
                        }
                        HomeIconButton(systemName: "square.and.pencil", label: "New chat") {
                            selectedSessionId = nil
                            viewModel.startNewSession()
                        }
                    }
                }
            )

            ChatContentView(
                sessionId: selectedSessionId,
                viewModel: viewModel,
                launchStyle: true
            )
        }
        .background(Color("BgPrimary"))
        // The drawer's New chat: start fresh even when the selection was
        // already nil (a just-sent chat has no selection).
        .onReceive(NotificationCenter.default.publisher(for: .shellNewChat)) { _ in
            selectedSessionId = nil
            viewModel.startNewSession()
        }
        .task(id: "\(viewModel.currentSessionId ?? "")|\(viewModel.isStreaming)") {
            // Refresh on open, on a new session, and when a turn settles
            // (the server names a chat after its first reply).
            guard !viewModel.isStreaming else { return }
            if let list = try? await AgentChatClient().listSessions() {
                sessions = list
            }
        }
    }
}

// MARK: - Phone header

/// Phone top bar (web PhoneHeader): menu on the left, a centered title, the
/// screen's actions on the right. No separator — content scrolls under it.
struct HomeHeader<Trailing: View>: View {
    let title: String
    let onMenu: () -> Void
    @ViewBuilder let trailing: () -> Trailing

    var body: some View {
        HStack(spacing: 0) {
            HStack {
                HomeIconButton(systemName: "line.3.horizontal", label: "Open menu", action: onMenu)
            }
            .frame(width: 96, alignment: .leading)

            Text(title)
                .font(.system(size: 16, weight: .semibold))
                .tracking(-0.2)
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(1)
                .truncationMode(.tail)
                .frame(maxWidth: .infinity)
                .accessibilityAddTraits(.isHeader)

            HStack { trailing() }
                .frame(width: 96, alignment: .trailing)
        }
        .padding(.horizontal, 6)
        .frame(height: 52)
        .background(Color("BgPrimary"))
    }
}

/// 44pt round icon button (web .phone-icon-btn).
struct HomeIconButton: View {
    let systemName: String
    let label: String
    let action: () -> Void

    var body: some View {
        Button(action: {
            UIImpactFeedbackGenerator(style: .light).impactOccurred()
            action()
        }) {
            Image(systemName: systemName)
                .font(.system(size: 20))
                .foregroundColor(Color("TextPrimary"))
                .frame(width: 44, height: 44)
                .contentShape(Circle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }
}

/// The open-chats count in a rounded square (web PhoneOverviewButton), with a
/// dot while one of them is working.
struct HomeOverviewButton: View {
    let count: Int
    let working: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Text(count > 99 ? "99+" : "\(count)")
                .font(.system(size: 11, weight: .bold).monospacedDigit())
                .foregroundColor(Color("TextPrimary"))
                .padding(.horizontal, 4)
                .frame(minWidth: 22, minHeight: 22)
                .overlay(RoundedRectangle(cornerRadius: 6).stroke(Color("TextPrimary"), lineWidth: 1.8))
                .overlay(alignment: .topTrailing) {
                    if working {
                        Circle()
                            .fill(Color(hex: "#D97757"))
                            .frame(width: 7, height: 7)
                            .overlay(Circle().stroke(Color("BgPrimary"), lineWidth: 2))
                            .offset(x: 4, y: -4)
                    }
                }
                .frame(width: 44, height: 44)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("All chats: \(count) open\(working ? ", some working" : "")")
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

    @EnvironmentObject private var modeStore: AppModeStore
    @EnvironmentObject private var agentModeStore: AgentModeStore
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

    /// Code threads render as a terminal session (dark, monospace, `❯`
    /// prompts) instead of chat messages.
    private var isTerminal: Bool { modeStore.mode == .code }

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
                    if inputText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                        ArtifactModeGallery(
                            preferredMode: agentModeStore.isAgentEnabled(for: modeStore.mode)
                                ? agentModeStore.selectedTile(for: modeStore.mode) : nil,
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
            }
            .scrollDismissesKeyboard(.interactively)
            .background(Color("BgPrimary"))
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
                                TerminalEmptyState()
                                    .padding(.top, 48)
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

            composer(deck: launchStyle ? .compact : .none)
                .padding(.horizontal, 14)
                .padding(.top, 6)
                .padding(.bottom, 8)
                .background(Color("BgPrimary"))
        }
        .background(Color("BgPrimary"))
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
            }
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
        VStack(spacing: 10) {
            Image("GizziMascot")
                .resizable()
                .scaledToFit()
                .frame(width: 44, height: 44)
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
