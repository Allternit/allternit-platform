import SwiftUI

import UIKit
// MARK: - Composer

/// Deck slide motion — matches the web's deck-rise / deck-fall keyframes:
/// 0.35s cubic-bezier(0.22, 1, 0.36, 1)
/// (surfaces/ai.allternit.com/tailwind.config.ts:89-116).
enum DeckMotion {
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
struct ComposerTextView: UIViewRepresentable {
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
struct AgentSelectionMenu: View {
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

