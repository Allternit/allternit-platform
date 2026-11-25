import SwiftUI
import Speech

import UIKit
// MARK: - Composer

/// Deck slide motion — matches the web's deck-rise / deck-fall keyframes:
/// 0.35s cubic-bezier(0.22, 1, 0.36, 1)
/// (surfaces/ai.allternit.com/tailwind.config.ts:89-116).
enum DeckMotion {
    static let animation = Animation.timingCurve(0.22, 1, 0.36, 1, duration: 0.35)
}

/// Which bottom deck rides under the composer card (web ComposerDeck):
/// `full` on the Home launch screen, `compact` in a chat session, `none` on
/// hosts with no deck (bots, browser, code).
enum ComposerDeckVariant: Equatable {
    case none, compact, full
}

/// The phone composer (web ChatComposer, phone branch): one quiet card — the
/// text line on top, then a hairline and the controls row (+ · permissions ·
/// model · voice/send) — with the Agent mode ▾ | Plugins ▾ | Artifacts ▾
/// band tucked behind its bottom edge.
struct ComposerView: View {
    @Binding var inputText: String
    /// Staged attachments from the "+" sheet — rendered as the preview strip
    /// above the text, uploaded on send. Owned by ChatContentView (next to the
    /// view model that uploads them).
    @ObservedObject var attachmentStore: AttachmentStore
    let isStreaming: Bool
    /// True while the first message's session is being created — blocks
    /// double-sends into the same unbacked draft.
    let isSendDisabled: Bool
    let hasActiveSession: Bool
    /// The chat on screen (nil before the first send) — the permission pill
    /// acts on it, or on the pending choice for a new chat.
    var sessionId: String? = nil
    /// Hard usage wall: at/above 100% the composer locks — the field is not
    /// editable and send stays grayed.
    var isUsageLocked: Bool = false
    /// Voice is disabled while a stream is in flight or usage is locked.
    var isVoiceModeEnabled: Bool = true
    /// Bot composers: Play shows from the moment the session opens.
    var playGamesAnytime: Bool = false
    var deck: ComposerDeckVariant = .none
    /// No brain picked for this send: Send opens the model picker and keeps
    /// the draft instead of sending.
    var requiresModelPick: Bool = false
    /// Send tapped; carries the composer's staged attachments (empty when
    /// none) alongside the bound inputText.
    let onSend: ([StagedAttachment]) -> Void
    let onStop: () -> Void
    /// Waveform button: starts voice mode.
    let onVoiceMode: () -> Void
    /// Called whenever agent mode turns on or off (a deck mode picked or
    /// cleared) so the host can mount a fresh session context.
    var onAgentModeChanged: ((Bool) -> Void)? = nil
    /// A fixed hint in place of the rotating suggestions (Bots home: "Ask
    /// Goldie to…").
    var placeholderOverride: String? = nil
    /// The picked bot's colour on the card's border (Bots home To: chip).
    var accentBorder: Color? = nil
    /// Content on the mode orb's row, right of the orb (Bots home To: chip).
    var orbRowTrailing: AnyView? = nil
    /// "Permission · Auto ›" in the + sheet (chat only; the Bots home and bot
    /// threads have none). The composer row has no permission pill (C2).
    var showsPermission: Bool = true
    /// The tray above the card (ThreadComposer `topDeck`): a bot thread's
    /// tabs — the open thread · Threads · Tasks · Bot.
    var topDeck: AnyView? = nil
    /// Voice mode in the send slot when there's no draft; off, the slot is
    /// always Send (group rooms).
    var showsVoice: Bool = true
    /// The + (files and photos); off where a send can't carry files.
    var showsAttach: Bool = true
    /// In an artifact session: always the chat composer, whichever section the
    /// app was on, with the session's own kind in the deck.
    var artifactKind: ArtifactStudioKind? = nil
    /// Artifacts ▾ → All artifacts, in an artifact session.
    var onAllArtifacts: (() -> Void)? = nil
    /// "Run on my phone" (chat ⋯ menu, or the + sheet when the host has no
    /// ⋯ menu), and the "On <phone>" chip while armed.
    var allowsPhoneRun: Bool = false
    /// The host's chat ⋯ menu holds Run on my phone and Capture to brain, so
    /// the + sheet leaves them out (ChatView's Home chat).
    var chatMenuHoldsExtras: Bool = false
    var armedPhoneName: String? = nil
    @ObservedObject private var attachedStore = AttachedArtifactStore.shared
    var onClearPhone: (() -> Void)? = nil
    /// The computer chip above the card (contract C4). On in every chat,
    /// Bots and bot thread composer.
    var showsComputerChip: Bool = true

    @EnvironmentObject private var modeStore: AppModeStore
    @EnvironmentObject private var agentModeStore: AgentModeStore
    /// A bot chat: the runtime, brain and bot chips take the model chip's place.
    @Environment(\.chatBot) private var chatBot
    @StateObject private var modelStore = ModelStore.shared
    @ObservedObject private var usageStore = UsageStore.shared
    @ObservedObject private var storefront = StorefrontGate.shared
    @State private var isPlusSheetPresented = false
    @State private var isModelPickerPresented = false
    /// App-owned mic priming sheet, shown before voice mode's first system
    /// prompt (once per install via AppPermission.microphone).
    @State private var isMicPrimingPresented = false
    @State private var isFocused = false
    @State private var keyboardFocused = false
    @State private var placeholderIndex = 0
    // Purchase pages go to the browser via the Open links in rule (step 30).
    @Environment(\.openURL) private var openURL
    /// The In-App Purchase sheet, where the App Store sells plans.
    @State private var isStoreSheetPresented = false
    /// Tools start folded; the sliders button opens the band.
    @State private var deckFolded = true
    /// The Runtime·Brain chip's width: the gap it leaves in the top border.
    @State private var chipWidth: CGFloat = 0
    /// One of the deck's sheets is up: keeps the band open under it.
    @State private var deckSheetActive = false
    /// Dictation (C3): the mic left of the voice button.
    @StateObject private var dictation = DictationController()
    @State private var dictationBase = ""
    /// The Undo dictation pill shows after dictation stops with words in.
    @State private var showsUndoDictation = false
    /// Keep mine ends dictation without offering the undo pill.
    @State private var suppressUndoPill = false
    @ObservedObject private var chipStore = ComputerChipStore.shared
    @ReducedMotion private var reduceMotion
    /// The + sheet grows from the + (iOS 18 zoom).
    @Namespace private var plusZoom

    private var mode: AppMode { artifactKind != nil ? .chat : modeStore.mode }
    private var agentOn: Bool { agentModeStore.isAgentEnabled(for: mode) }

    /// The artifact mode Send will start a session for: picked in the deck,
    /// with the deck showing and this not already an artifact session.
    private var launchMode: ChatDeckMode? {
        guard deck != .none, artifactKind == nil, !isTerminal, agentOn else { return nil }
        return ChatDeckMode.pendingLaunch(tile: agentModeStore.selectedTile(for: mode), surface: mode, bot: chatBot != nil)
    }

    /// Code mode keeps its terminal flavor: `❯` prompt, monospace text.
    private var isTerminal: Bool { mode == .code }

    private var hasDraft: Bool {
        !inputText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !attachmentStore.attachments.isEmpty
    }

    /// The deck band shows (it is folded at rest). Hosts with no deck never.
    private var deckBandOpen: Bool {
        deckOpen(folded: deckFolded)
    }

    private var forcedDeckOpen: Bool {
        #if DEBUG
        return launchArgumentEnabled("calm-deck-open")
        #else
        return false
        #endif
    }

    /// Dictation needs a speech recognizer for the current language.
    private var dictationAvailable: Bool {
        SFSpeechRecognizer()?.isAvailable ?? false
    }

    private var canSend: Bool {
        hasDraft && !isSendDisabled && !isUsageLocked
    }

    /// Launch screen: rotating suggestions (web useRotatingPlaceholder);
    /// a chat in session: "Reply…".
    private var placeholder: String {
        if isTerminal { return "describe the task…" }
        if let placeholderOverride { return placeholderOverride }
        if !isExpanded { return "Type or hold to talk" }
        // A picked artifact mode says what it will make ("Describe the slides…").
        if let launchMode { return launchMode.placeholder }
        guard deck == .full else { return "Reply…" }
        let list = ComposerPlaceholders.list(agentMode: agentOn, tile: agentOn ? agentModeStore.selectedTile(for: mode) : nil)
        return list[placeholderIndex % list.count]
    }

    var body: some View {
        VStack(spacing: 0) {
            limitStrip

            if let topDeck {
                ComposerTopDeckTray { topDeck }
                    .zIndex(0)
            } else if let launchMode, DeckLaunch.kind(for: launchMode.entry) != nil, deckBandOpen || forcedDeckOpen {
                // The "It can" row: only the toggles this kind can honour.
                ComposerTopDeckTray { ItCanRow(entry: launchMode.entry) }
                    .zIndex(0)
            }

            if let attachedArtifact = attachedStore.attached {
                HStack {
                    AttachedArtifactChip(artifact: attachedArtifact) { attachedStore.detach() }
                    Spacer()
                }
                .padding(.horizontal, 16)
                .padding(.bottom, 6)
            }

            if let armedPhoneName {
                HStack {
                    PhoneArmedChip(name: armedPhoneName) { onClearPhone?() }
                    Spacer()
                }
                .padding(.horizontal, 16)
                .padding(.bottom, 6)
            }

            // The computer chip (C4): no computer, can't reach it, waking, or a
            // compute gate notice. ✕ folds it into the dot on +.
            if showsComputerChip {
                ComputerChipSlot(dismissible: showsAttach)
                    .zIndex(1)
            }

            // One calm line when dictation can't start.
            if let dictationError = dictation.errorMessage {
                Text(plainErrorText(message: dictationError))
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 16)
                    .padding(.bottom, 6)
                    .accessibilityAddTraits(.updatesFrequently)
                    .task(id: dictationError) {
                        try? await Task.sleep(for: .seconds(5))
                        if !Task.isCancelled { dictation.errorMessage = nil }
                    }
            }

            if dictation.isRecording, !isTerminal {
                DictationPanel(
                    onCancel: cancelDictation,
                    onStop: { Haptics.light(); dictation.stop() },
                    onKeep: { Haptics.light(); suppressUndoPill = true; dictation.stop() }
                )
                .transition(reduceMotion ? .opacity : .move(edge: .bottom).combined(with: .opacity))
            } else if showsUndoDictation, !isTerminal {
                UndoDictationPill(
                    onUndo: undoDictation,
                    onExpire: { withAnimation(reduceMotion ? nil : .easeOut(duration: 0.2)) { showsUndoDictation = false } }
                )
                .transition(.opacity)
            }

            modeOrbRow

            card
                .zIndex(1)
                .simultaneousGesture(
                    DragGesture(minimumDistance: 12)
                        .onEnded { value in
                            guard deck != .none, !isTerminal else { return }
                            let next = nextSwipedOpen(current: !deckFolded, startY: value.startLocation.y, endY: value.location.y)
                            guard next == deckFolded else { return }
                            setDeckFolded(!next)
                        }
                )

            if deck != .none, !isTerminal, deckBandOpen || forcedDeckOpen {
                ComposerDeckBand(
                    variant: deck, inputText: $inputText, sessionKind: artifactKind,
                    onAllArtifacts: onAllArtifacts, onSheetActive: { deckSheetActive = $0 }
                )
                    .zIndex(0)
                    // With no tab while open, VoiceOver folds it from here.
                    .accessibilityAction(named: Text("Hide tools")) { setDeckFolded(true) }
                    .transition(reduceMotion ? .identity : .move(edge: .top).combined(with: .opacity))
            }
        }
        .animation(reduceMotion ? nil : Motion.deck, value: deckBandOpen || forcedDeckOpen)
        .animation(reduceMotion ? nil : DeckMotion.animation, value: deck)
        .animation(reduceMotion ? nil : .spring(response: 0.3, dampingFraction: 0.85), value: dictation.isRecording)
        .onChange(of: dictation.isRecording) { _, recording in
            if recording {
                showsUndoDictation = false
            } else if suppressUndoPill {
                suppressUndoPill = false
            } else if !dictation.transcript.isEmpty {
                showsUndoDictation = true
            }
        }
        .background(ComposerFrameReporter())
        .onChange(of: dictation.transcript) { _, transcript in
            guard dictation.isRecording || !transcript.isEmpty else { return }
            inputText = dictationBase + transcript
        }
        .sheet(isPresented: $isPlusSheetPresented) {
            ComposerPlusSheet(
                attachmentStore: attachmentStore,
                sessionId: sessionId,
                allowsPhoneRun: allowsPhoneRun,
                showsPermission: plusShowsPermission,
                chatMenuHoldsExtras: chatMenuHoldsExtras,
                fadesIn: reduceMotion
            )
            .composerPlusZoomDestination(plusZoom, enabled: !reduceMotion)
        }
        .sheet(isPresented: $isModelPickerPresented) {
            ModelPickerSheet(modelStore: modelStore)
        }
        .sheet(isPresented: $isMicPrimingPresented) {
            PermissionPrimingSheet(permission: .microphone) { onVoiceMode() }
        }
        .sheet(isPresented: $isStoreSheetPresented) {
            StorePurchaseSheet(focus: .plan)
        }
        .onAppear {
            modelStore.fetchModelsIfNeeded()
            usageStore.fetchUsageIfNeeded()
            #if DEBUG
            // `-open-plus-sheet` / `-open-model-picker` (DEBUG only): jump
            // straight to a composer sheet for screenshots (simctl has no
            // tap injection).
            if showsAttach,
               launchArgumentEnabled(LaunchArgument.openPlusSheet)
                || launchArgumentEnabled(LaunchArgument.openPlusPermission)
                || launchArgumentEnabled(LaunchArgument.openPlusComputer) {
                isPlusSheetPresented = true
            }
            if launchArgumentEnabled("open-model-picker") { isModelPickerPresented = true }
            if launchArgumentEnabled("open-mic-priming") { isMicPrimingPresented = true }
            // `-enable-agent-mode` (DEBUG only): arm the first deck mode.
            if launchArgumentEnabled("enable-agent-mode"), !agentOn {
                agentModeStore.setAgentEnabled(true, for: mode)
            }
            #endif
        }
        .onChange(of: agentOn) { _, on in
            if on { agentModeStore.fetchAgentsIfNeeded() }
            placeholderIndex = 0
            onAgentModeChanged?(on)
        }
        .task(id: "\(deck == .full)|\(isFocused)|\(inputText.isEmpty)|\(agentOn)") {
            // Rotate every 7s on the launch screen; paused while focused or
            // typing, so it never changes under the user.
            guard deck == .full, !isFocused, inputText.isEmpty else { return }
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(7))
                if Task.isCancelled { return }
                withAnimation(.easeInOut(duration: 0.25)) { placeholderIndex += 1 }
            }
        }
    }

    // MARK: Card

    /// Rest = one pill; focused, drafting, attached or recording = the open
    /// card (chips line, text, then the controls row).
    private var isExpanded: Bool {
        isFocused || hasDraft || dictation.isRecording || isStreaming || dictation.errorMessage != nil || ((deckBandOpen || forcedDeckOpen) && deck != .none && !isTerminal)
    }

    private var chipsGroup: some View {
        Group {
            if let chatBot {
                ComposerAgentChips(bot: chatBot, iconOnly: !isExpanded)
            } else {
                // Harness & Bots 2.0: Runtime + Brain in every chat.
                ComposerChatChips(iconOnly: !isExpanded, hasActiveSession: hasActiveSession) { isModelPickerPresented = true }
            }
        }
    }

    private var card: some View {
        VStack(alignment: .leading, spacing: 0) {
            // Phone: attachment previews above the text (no room for a card row).
            if !attachmentStore.attachments.isEmpty {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 8) {
                        ForEach(attachmentStore.attachments) { attachment in
                            attachmentThumb(attachment)
                        }
                    }
                    .padding(.horizontal, 16)
                }
                .padding(.top, 12)
            }

            // Focused: the usage meter, top right, below the chip on the border.
            if isExpanded {
                HStack(spacing: 8) {
                    Spacer(minLength: 0)
                    ComposerUsageMeter(sessionId: sessionId, refreshKey: isStreaming ? "streaming" : "idle")
                }
                .padding(.horizontal, 12)
                .padding(.top, 16)
            }

            HStack(alignment: .center, spacing: 6) {
                if !isExpanded, showsAttach { plusButton(size: 36) }
                if isTerminal {
                    Text("❯")
                        .font(.system(size: 15, weight: .semibold, design: .monospaced))
                        .foregroundColor(Theme.accentCode)
                        .padding(.top, 1)
                }
                ComposerTextView(
                    text: $inputText,
                    placeholder: placeholder,
                    isEnabled: !isUsageLocked,
                    terminal: isTerminal,
                    onFocusChange: { isFocused = $0; if !$0 { keyboardFocused = false } },
                    onKeyboardFocusChange: { keyboardFocused = $0 },
                    singleLineRest: !isExpanded
                )
                .accessibilityIdentifier("composerInput")
                if !isExpanded {
                    // Rest: + | text | mic | voice. Games stays mounted, hidden.
                    gamesButton
                        .opacity(0)
                        .allowsHitTesting(false)
                        .accessibilityHidden(true)
                    sendSlot
                }
            }
            .padding(.horizontal, isExpanded ? 16 : 8)
            .padding(.top, isExpanded ? 6 : 8)
            .padding(.bottom, isExpanded ? 4 : 8)

            if isExpanded {
                controlsRow
                    .padding(.vertical, 8)
                    .padding(.horizontal, 8)
            }
        }
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: isTerminal ? 0 : (isExpanded ? 22 : 26), style: .continuous))
        .overlay(
            // Touch keeps the quiet border. Hardware focus gets a visible cue.
            RoundedRectangle(cornerRadius: isTerminal ? 0 : (isExpanded ? 22 : 26), style: .continuous)
                .stroke(keyboardFocused ? Color("TextPrimary") : (accentBorder ?? Color("BorderSubtle")),
                        style: StrokeStyle(lineWidth: keyboardFocused ? 2 : 1, dash: isTerminal ? [4, 4] : []))
                .mask { borderGapMask }
        )
        .shadow(color: .black.opacity(trayOpen ? 0.10 : 0.04), radius: trayOpen ? 10 : 1, y: trayOpen ? 3 : 1)
        .shadow(color: .black.opacity(0.03), radius: 4, y: 2)
        // The chip sits on the top border, centred on the line, drawn after
        // the clip so it is not cut. No box behind it: the border leaves a gap
        // the chip's width (borderGapMask), like a fieldset legend and like
        // Code's prompt rule, so it reads the same over any page or backdrop.
        // The alignment guide sits outside the `if`: an overlay ignores a
        // guide set inside a conditional, which hung the chip's 44pt frame
        // down into the card (build 52).
        .overlay(alignment: .topTrailing) {
            Group {
                if !isTerminal {
                    chipsGroup
                        .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { chipWidth = $0 }
                }
            }
            .alignmentGuide(VerticalAlignment.top) { d in d[VerticalAlignment.center] }
            .padding(.trailing, chipInset)
        }
        // Room above the card so the chip clears the orb row.
        .padding(.top, isTerminal ? 0 : 10)
        .animation(reduceMotion ? nil : .easeOut(duration: 0.18), value: isExpanded)
    }

    /// The chip's distance from the card's right edge.
    private let chipInset: CGFloat = 18

    /// Cuts the chip's width out of the card's top border so the line stops
    /// either side of the chip. Padded out so the stroke's outer half stays.
    private var borderGapMask: some View {
        Rectangle()
            .padding(-2)
            .overlay(alignment: .topTrailing) {
                Rectangle()
                    .frame(width: isTerminal ? 0 : chipWidth, height: 6)
                    .alignmentGuide(VerticalAlignment.top) { d in d[VerticalAlignment.center] }
                    .padding(.trailing, chipInset)
                    .blendMode(.destinationOut)
            }
            .compositingGroup()
    }

    /// The Tools tray is open under the card.
    private var trayOpen: Bool {
        deck != .none && !isTerminal && (deckBandOpen || forcedDeckOpen)
    }

    /// The bare filled "+" (Add to chat).
    private func plusButton(size: CGFloat) -> some View {
        Button(action: openPlusSheet) {
            Image(systemName: "plus")
                .font(.system(size: size > 40 ? 20 : 17, weight: .medium))
                .foregroundColor(Color("TextSecondary"))
                // The plus turns into an x while its sheet is open (the
                // sheet zooms out of this same button).
                .rotationEffect(.degrees(isPlusSheetPresented ? 45 : 0))
                .animation(reduceMotion ? nil : .spring(response: 0.3, dampingFraction: 0.75), value: isPlusSheetPresented)
                .frame(width: size, height: size)
                .background(Color("FillNeutral"), in: Circle())
                // The folded computer chip.
                .overlay(alignment: .topTrailing) {
                    if showsComputerChip { ComputerChipDot().offset(x: -3, y: 3) }
                }
                .frame(minWidth: 44, minHeight: 44)
                .contentShape(Rectangle())
                .composerPlusZoomSource(plusZoom, enabled: !reduceMotion)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Add to chat")
    }

    private var modeOrbRow: some View {
        ComposerOrbRow(hasActiveSession: hasActiveSession,
                       toolsOpen: deck != .none && (deckBandOpen || forcedDeckOpen),
                       trailing: orbRowTrailing)
            .padding(.horizontal, 16)
            .padding(.bottom, 6)
            .zIndex(2)
    }

    /// The + sheet shows "Permission · Auto ›" where the pill used to show.
    private var plusShowsPermission: Bool {
        showsPermission && (mode == .chat || mode == .cowork)
    }

    /// + opens "Add to chat": it grows from the + (iOS 18+); under reduced
    /// motion it appears without the slide and fades in.
    private func openPlusSheet() {
        Haptics.light()
        if reduceMotion {
            var transaction = Transaction()
            transaction.disablesAnimations = true
            withTransaction(transaction) { isPlusSheetPresented = true }
        } else {
            isPlusSheetPresented = true
        }
    }

    private var controlsRow: some View {
        HStack(spacing: 6) {
            if showsAttach { plusButton(size: 44) }
            // Permission (Auto · Ask · Read-only) lives in the + sheet (C2).
            if deck != .none, !isTerminal {
                toolsButton
            }
            gamesButton

            Spacer(minLength: 0)

            sendSlot
        }
    }

    private var toolsButton: some View {
        Button { setDeckFolded(deckBandOpen) } label: {
            Image(systemName: "slider.horizontal.3")
                .font(.system(size: 17, weight: .medium))
                .foregroundColor(Color("TextSecondary"))
                .frame(width: 44, height: 44)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(deckBandOpen ? "Hide tools" : "Tools")
    }

    /// Play (the waiting games): beside Tools when expanded, hidden at rest.
    private var gamesButton: some View {
        WaitingGamesPlayButton(busy: isStreaming, anytime: playGamesAnytime, hidden: hasDraft)
    }

    /// Phone: voice and send share one slot (ChatComposer phoneSendSlotTaken).
    /// Play (the waiting games) sits beside Stop; it stays mounted so it can
    /// tell an open game the reply is ready.
    private var sendSlot: some View {
        HStack(spacing: 8) {
            // Dictation fills the draft, which swaps voice for Send: keep the
            // mic here while recording so it can be stopped.
            if dictation.isRecording, hasDraft, !isStreaming, !isTerminal { micButton }
            sendButton
        }
    }

    @ViewBuilder
    private var sendButton: some View {
        if isStreaming {
            HStack(spacing: 8) {
                StreamingBars()
                Button(action: onStop) {
                    Image(systemName: "square.fill")
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextPrimary"))
                        .frame(width: 44, height: 44)
                        .background(Color("FillNeutral"), in: Circle())
                        .overlay(Circle().stroke(Color("BorderSubtle"), lineWidth: 1))
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Stop")
            }
        } else if hasDraft || !showsVoice {
            Button(action: sendTapped) {
                Group {
                    if isTerminal {
                        Image(systemName: "return")
                            .font(.system(size: 16, weight: .bold))
                    } else {
                        Image(systemName: "arrow.up")
                            .font(.system(size: 18, weight: .semibold))
                    }
                }
                .foregroundColor(Color("OnAccent"))
                .frame(width: 44, height: 44)
                .background(Color("AccentPrimary"), in: Circle())
                .opacity(canSend ? 1 : 0.7)
            }
            .buttonStyle(.plain)
            .disabled(!canSend)
            .accessibilityLabel("Send message")
        } else {
            HStack(spacing: 0) {
                if showsMic { micButton }
                voiceButton
            }
        }
    }

    /// The mic shows beside voice mode (no draft) and while recording, so it
    /// can always be stopped. Hidden where dictation isn't available.
    private var showsMic: Bool {
        showsVoice && !isTerminal && (dictationAvailable || dictation.isRecording)
    }

    private var micButton: some View {
        Button(action: toggleDictation) {
            Image(systemName: dictation.isRecording ? "mic.fill" : "mic")
                .font(.system(size: 17, weight: .semibold))
                .foregroundColor(dictation.isRecording ? Color("AccentPrimary") : Color("TextSecondary"))
                .frame(width: 44, height: 44)
                .contentShape(Circle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Dictate")
        .accessibilityValue(dictation.isRecording ? "Recording" : "")
    }

    private var voiceButton: some View {
        Group {
            Button(action: voiceModeTapped) {
                Image(systemName: "waveform")
                    .font(.system(size: 17, weight: .bold))
                    .foregroundColor(Color("TextSecondary").opacity(isVoiceModeEnabled ? 1 : 0.4))
                    .frame(width: 44, height: 44)
                    .contentShape(Circle())
            }
            .buttonStyle(.plain)
            .disabled(!isVoiceModeEnabled)
            .accessibilityLabel("Start voice mode")
        }
    }

    // MARK: Usage-limit strip

    /// The composer's usage-limit strip (web ComposerLimitBar) for the
    /// account's allowance: approaching, then reached.
    @ViewBuilder
    private var limitStrip: some View {
        if usageStore.isAtLimit || usageStore.shouldShowBanner {
            HStack(spacing: 8) {
                Image(systemName: usageStore.isAtLimit ? "exclamationmark.triangle" : "circle.lefthalf.filled")
                    .font(.system(size: 13))
                    .foregroundColor(usageStore.isAtLimit ? Theme.statusWarning : Color("TextSecondary"))
                Text(limitText)
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(2)
                Spacer(minLength: 4)
                // The App Store sheet where it sells plans (it carries the web
                // link on the US storefront), else the web on the US storefront.
                if storefront.canBuy(.plan) {
                    Button("Upgrade") {
                        if storefront.showsStoreButtons(for: .plan) {
                            isStoreSheetPresented = true
                        } else {
                            openURL(AccountPlanStore.shared.upgradeURL)
                        }
                    }
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                }
                if !usageStore.isAtLimit {
                    Button(action: { usageStore.dismissBannerForToday() }) {
                        Image(systemName: "xmark")
                            .font(.system(size: 11, weight: .semibold))
                            .foregroundColor(Color("TextSecondary"))
                            .frame(width: 28, height: 28)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Dismiss")
                }
            }
            .padding(.leading, 12)
            .padding(.trailing, 6)
            .padding(.vertical, 6)
            .background(Color("FillNeutral"), in: RoundedRectangle(cornerRadius: 12))
            .padding(.bottom, 6)
        }
    }

    private var limitText: String {
        let resets = usageStore.resetsLabel.map { " · Resets \($0)" } ?? ""
        return (usageStore.isAtLimit ? "Usage limit reached" : "Approaching usage limit") + resets
    }

    // MARK: Actions

    private func voiceModeTapped() {
        guard isVoiceModeEnabled else { return }
        Haptics.light()
        if DictationController.systemPromptsPending(), !AppPermission.microphone.hasPrimed {
            // App-owned explainer before the system prompts, once per install.
            isMicPrimingPresented = true
        } else {
            onVoiceMode()
        }
    }

    private func sendTapped() {
        guard canSend else { return }
        if requiresModelPick {
            isModelPickerPresented = true
            return
        }
        suppressUndoPill = dictation.isRecording
        showsUndoDictation = false
        dictation.stop()
        inputText = attachedStore.consume(typed: inputText)
        onSend(attachmentStore.attachments)
        attachmentStore.clear()
    }

    /// Cancel: stop listening and put the field back as it was.
    private func cancelDictation() {
        Haptics.light()
        suppressUndoPill = true
        dictation.stop()
        inputText = dictationBase
    }

    /// Undo dictation: the field goes back to what it held before the mic.
    private func undoDictation() {
        Haptics.light()
        inputText = dictationBase.trimmingCharacters(in: .whitespaces).isEmpty ? "" : dictationBase
        showsUndoDictation = false
    }

    private func toggleDictation() {
        Haptics.light()
        if dictation.isRecording {
            dictation.stop()
        } else {
            dictationBase = inputText.isEmpty || inputText.hasSuffix(" ") ? inputText : inputText + " "
            Task { await dictation.start() }
        }
    }

    /// Fold or unfold the deck and remember it on this device.
    private func setDeckFolded(_ folded: Bool) {
        Haptics.selection()
        UserDefaults.standard.set(folded, forKey: deckFoldedDefaultsKey)
        withAnimation(reduceMotion ? nil : Motion.deck) { deckFolded = folded }
    }

    /// One staged attachment: image thumbnail or a file chip, X to unstage.
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
            .background(Color("FillNeutral"))
            .clipShape(RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).stroke(Color("BorderSubtle"), lineWidth: 1))

            Button(action: {
                Haptics.light()
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
            .accessibilityLabel("Remove \(attachment.filename)")
        }
        .padding(.top, 4)
        .padding(.trailing, 4)
    }
}

/// The composer model chip's text: the selected model's display name with
/// any trailing "(…)" dropped (the phone has no room for it), else its id.
enum ComposerModelLabel {
    @MainActor
    static func label(_ store: ModelStore) -> String {
        guard let id = store.selectedModelId, !id.isEmpty else { return "Pick a brain" }
        if let name = store.selectedModel?.name, !name.isEmpty {
            let trimmed = name.replacingOccurrences(of: #"\s*\([^)]*\)\s*$"#, with: "", options: .regularExpression)
            return trimmed.isEmpty ? name : trimmed
        }
        return id.split(separator: "/", maxSplits: 1).last.map(String.init) ?? id
    }
}

/// Launch-state placeholder suggestions (web useRotatingPlaceholder.ts):
/// plain, concrete prompts that follow the selected mode.
enum ComposerPlaceholders {
    static let chat = [
        "Ask anything, or paste something to work through",
        "Draft the email you’ve been putting off",
        "Explain this contract clause in plain terms",
        "Turn these notes into a clear plan",
        "Compare two options and tell me which fits",
        "Summarize a long article or PDF",
        "Think a decision through out loud with me",
    ]

    static let agentNoMode = [
        "Pick a mode below, or describe the outcome you want",
        "Type / for plugins and skills, @ for bots and files",
    ]

    static func byMode(_ tile: AgentModeTile) -> [String] {
        switch tile {
        case .research: return ["Ask for an in-depth, cited research report", "Compare five competitors with sources", "What does the latest data say about…"]
        case .slides: return ["Describe the deck. Audience, length, tone", "Turn a brief into a 10-slide pitch", "Make a workshop deck with exercises"]
        case .website: return ["Describe the site you want built", "Landing page for a bakery in Austin", "Rebuild a site from its URL, cleaner"]
        case .docs: return ["Describe the document you need", "Write a one-page proposal from these notes"]
        case .data: return ["Paste data or describe the sheet you need", "Chart monthly revenue from this CSV"]
        case .swarms: return ["Describe the outcome. A team of agents will split it up"]
        case .image: return ["Describe the image. Subject, style, format"]
        case .video: return ["Describe the clip. Length, shots, mood"]
        // No placeholders of its own on the web: the no-mode hints.
        case .artifact: return agentNoMode
        case .code: return ["Describe what to build or fix", "Paste the error and the code around it"]
        }
    }

    static func list(agentMode: Bool, tile: AgentModeTile?) -> [String] {
        guard agentMode else { return chat }
        if let tile { return byMode(tile) }
        return agentNoMode
    }
}

/// Three small bars beside Stop while a turn runs (web allternit-waveform-bar).
private struct StreamingBars: View {
    @State private var phase = false
    @ReducedMotion private var reduceMotion

    var body: some View {
        HStack(alignment: .bottom, spacing: 2) {
            ForEach(0..<3, id: \.self) { index in
                Capsule()
                    .fill(Color("TextPrimary"))
                    .frame(width: 2, height: phase ? 16 : 6)
                    .animation(
                        reduceMotion ? nil : .easeInOut(duration: 0.5).repeatForever().delay(Double(index) * 0.18),
                        value: phase
                    )
            }
        }
        .frame(height: 16)
        .onAppear { phase = true }
        .accessibilityHidden(true)
    }
}

/// UITextView-backed composer editor: grows to six lines, then scrolls; the
/// placeholder is a label laid over the text and wraps like the web's
/// textarea placeholder.
struct ComposerTextView: UIViewRepresentable {
    @Binding var text: String
    var placeholder: String
    var isEnabled: Bool
    /// Code mode: monospace font on the terminal palette.
    var terminal: Bool = false
    var onFocusChange: ((Bool) -> Void)? = nil
    var onKeyboardFocusChange: ((Bool) -> Void)? = nil
    var singleLineRest = false

    /// 16pt: iOS zooms fields under 16px, so the phone keeps every field there
    /// (phone.css), with the web's 24pt line.
    private var uiFont: UIFont {
        terminal
            ? UIFont.monospacedSystemFont(ofSize: 15, weight: .regular)
            : UIFont.systemFont(ofSize: 16)
    }

    private var uiTextColor: UIColor {
        terminal ? UIColor(TerminalTheme.text) : (UIColor(named: "TextPrimary") ?? .label)
    }

    private var paragraph: NSParagraphStyle {
        let style = NSMutableParagraphStyle()
        style.minimumLineHeight = 24
        style.maximumLineHeight = 24
        return style
    }

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeUIView(context: Context) -> UITextView {
        let textView = ComposerEditorView()
        textView.onKeyboardFocusChange = { focused in
            context.coordinator.parent.onKeyboardFocusChange?(focused)
        }
        textView.delegate = context.coordinator
        textView.font = uiFont
        textView.textColor = uiTextColor
        textView.typingAttributes = [.font: uiFont, .foregroundColor: uiTextColor, .paragraphStyle: paragraph]
        textView.backgroundColor = .clear
        textView.textContainerInset = .zero
        textView.textContainer.lineFragmentPadding = 0
        textView.autocapitalizationType = terminal ? .none : .sentences
        textView.autocorrectionType = terminal ? .no : .default
        textView.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        let placeholderLabel = context.coordinator.placeholderLabel
        placeholderLabel.font = uiFont
        placeholderLabel.textColor = terminal ? UIColor(TerminalTheme.dim) : UIColor(named: "TextTertiary")
        placeholderLabel.numberOfLines = 0
        placeholderLabel.translatesAutoresizingMaskIntoConstraints = false
        textView.addSubview(placeholderLabel)
        NSLayoutConstraint.activate([
            placeholderLabel.leadingAnchor.constraint(equalTo: textView.leadingAnchor),
            placeholderLabel.topAnchor.constraint(equalTo: textView.topAnchor),
            placeholderLabel.widthAnchor.constraint(equalTo: textView.frameLayoutGuide.widthAnchor),
        ])

        textView.attributedText = attributed(text)
        context.coordinator.updatePlaceholder(textView)
        return textView
    }

    func updateUIView(_ textView: UITextView, context: Context) {
        context.coordinator.parent = self
        context.coordinator.placeholderLabel.numberOfLines = singleLineRest ? 1 : 0
        textView.isEditable = isEnabled
        context.coordinator.placeholderLabel.attributedText = NSAttributedString(string: placeholder, attributes: [
            .font: uiFont,
            .foregroundColor: terminal ? UIColor(TerminalTheme.dim) : (UIColor(named: "TextTertiary") ?? .placeholderText),
            .paragraphStyle: paragraph,
        ])
        // Rebuild only for external fills (templates, edit-resend, voice) —
        // never mid-keystroke, so the cursor stays put.
        if textView.text != text {
            textView.attributedText = attributed(text)
            textView.selectedRange = NSRange(location: (text as NSString).length, length: 0)
        }
        context.coordinator.updatePlaceholder(textView)
        #if DEBUG
        // `-focus-composer` (DEBUG only): focus the editor on launch.
        if launchArgumentEnabled("focus-composer"), !textView.isFirstResponder {
            DispatchQueue.main.async { textView.becomeFirstResponder() }
        }
        #endif
    }

    func sizeThatFits(_ proposal: ProposedViewSize, uiView: UITextView, context: Context) -> CGSize? {
        let width = proposal.width ?? UIScreen.main.bounds.width
        var fit = uiView.sizeThatFits(CGSize(width: width, height: .greatestFiniteMagnitude))
        if uiView.text.isEmpty {
            // An empty field is as tall as its wrapped placeholder.
            let label = context.coordinator.placeholderLabel
            fit.height = max(fit.height, label.sizeThatFits(CGSize(width: width, height: .greatestFiniteMagnitude)).height)
        }
        if singleLineRest {
            uiView.isScrollEnabled = false
            return CGSize(width: width, height: 24)
        }
        let maxHeight: CGFloat = 24 * 6
        uiView.isScrollEnabled = fit.height > maxHeight + 1
        return CGSize(width: width, height: max(24, min(fit.height, maxHeight)))
    }

    private func attributed(_ string: String) -> NSAttributedString {
        NSAttributedString(string: string, attributes: [
            .font: uiFont,
            .foregroundColor: uiTextColor,
            .paragraphStyle: paragraph,
        ])
    }

    final class Coordinator: NSObject, UITextViewDelegate, UIGestureRecognizerDelegate {
        var parent: ComposerTextView
        let placeholderLabel = UILabel()

        init(_ parent: ComposerTextView) { self.parent = parent }

        func updatePlaceholder(_ textView: UITextView) {
            placeholderLabel.isHidden = !textView.text.isEmpty
        }

        func textViewDidChange(_ textView: UITextView) {
            parent.text = textView.text
            updatePlaceholder(textView)
        }

        // Tap outside the composer: resigns focus, so the keyboard goes away
        // and (with an empty draft) the deck folds. The recognizer sits on the
        // window, never cancels touches, and skips controls, text inputs and
        // anything inside a composer, so links, buttons and messages work as
        // before.
        private var outsideTap: UITapGestureRecognizer?
        private weak var editingView: UITextView?

        private func installOutsideTap(for textView: UITextView) {
            guard outsideTap == nil, let window = textView.window else { return }
            let tap = UITapGestureRecognizer(target: self, action: #selector(handleOutsideTap))
            tap.cancelsTouchesInView = false
            tap.delegate = self
            window.addGestureRecognizer(tap)
            outsideTap = tap
            editingView = textView
        }

        private func removeOutsideTap() {
            if let tap = outsideTap { tap.view?.removeGestureRecognizer(tap) }
            outsideTap = nil
            editingView = nil
        }

        @objc private func handleOutsideTap() {
            editingView?.resignFirstResponder()
        }

        func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer, shouldReceive touch: UITouch) -> Bool {
            guard let view = touch.view, let window = view.window else { return false }
            var v: UIView? = view
            while let cur = v {
                if cur is UIControl || cur is UITextView || cur is UITextField { return false }
                v = cur.superview
            }
            return !ComposerHitRegion.contains(touch.location(in: window))
        }

        func gestureRecognizer(
            _ gestureRecognizer: UIGestureRecognizer,
            shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer
        ) -> Bool { true }

        func textViewDidBeginEditing(_ textView: UITextView) {
            parent.onFocusChange?(true)
            installOutsideTap(for: textView)
        }

        func textViewDidEndEditing(_ textView: UITextView) {
            parent.onFocusChange?(false)
            removeOutsideTap()
        }
    }
}

/// Window-space frames of every on-screen composer (deck, chips and all), so a
/// tap inside one never counts as "outside".
enum ComposerHitRegion {
    nonisolated(unsafe) private static var frames: [UUID: CGRect] = [:]
    static func set(_ id: UUID, _ frame: CGRect?) { frames[id] = frame }
    static func contains(_ point: CGPoint) -> Bool { frames.values.contains { $0.contains(point) } }
}

private struct ComposerFrameReporter: View {
    @State private var id = UUID()
    var body: some View {
        GeometryReader { proxy in
            Color.clear
                .onAppear { ComposerHitRegion.set(id, proxy.frame(in: .global)) }
                .onChange(of: proxy.frame(in: .global)) { _, f in ComposerHitRegion.set(id, f) }
                .onDisappear { ComposerHitRegion.set(id, nil) }
        }
        .allowsHitTesting(false)
    }
}

/// UIKit focus navigation and physical key presses identify keyboard input.
/// Touch clears the cue before the editor becomes first responder.
final class ComposerEditorView: UITextView {
    var onKeyboardFocusChange: ((Bool) -> Void)?

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
        onKeyboardFocusChange?(false)
        super.touchesBegan(touches, with: event)
    }

    override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        if presses.contains(where: { $0.key != nil }) { onKeyboardFocusChange?(true) }
        super.pressesBegan(presses, with: event)
    }

    override func didUpdateFocus(in context: UIFocusUpdateContext, with coordinator: UIFocusAnimationCoordinator) {
        super.didUpdateFocus(in: context, with: coordinator)
        if context.nextFocusedView === self, !context.focusHeading.isEmpty {
            onKeyboardFocusChange?(true)
        } else if context.previouslyFocusedView === self {
            onKeyboardFocusChange?(false)
        }
    }
}
