import SwiftUI

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
    var deck: ComposerDeckVariant = .none
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
    /// The Auto / Ask / Read-only pill (chat only; the Bots home has none).
    var showsPermission: Bool = true
    /// The tray above the card (ThreadComposer `topDeck`): a bot thread's
    /// tabs — the open thread · Threads · Tasks · Bot.
    var topDeck: AnyView? = nil
    /// Voice mode in the send slot when there's no draft; off, the slot is
    /// always Send (group rooms).
    var showsVoice: Bool = true
    /// The + (files and photos); off where a send can't carry files.
    var showsAttach: Bool = true

    @EnvironmentObject private var modeStore: AppModeStore
    @EnvironmentObject private var agentModeStore: AgentModeStore
    @StateObject private var modelStore = ModelStore.shared
    @ObservedObject private var usageStore = UsageStore.shared
    @ObservedObject private var storefront = StorefrontGate.shared
    @State private var isPlusSheetPresented = false
    @State private var isModelPickerPresented = false
    /// App-owned mic priming sheet, shown before voice mode's first system
    /// prompt (once per install via AppPermission.microphone).
    @State private var isMicPrimingPresented = false
    @State private var isFocused = false
    @State private var placeholderIndex = 0
    @State private var upgradeURL: IdentifiableURL? = nil

    private var mode: AppMode { modeStore.mode }
    private var agentOn: Bool { agentModeStore.isAgentEnabled(for: mode) }

    /// Code mode keeps its terminal flavor: `❯` prompt, monospace text.
    private var isTerminal: Bool { mode == .code }

    private var hasDraft: Bool {
        !inputText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !attachmentStore.attachments.isEmpty
    }

    private var canSend: Bool {
        hasDraft && !isSendDisabled && !isUsageLocked
    }

    /// Launch screen: rotating suggestions (web useRotatingPlaceholder);
    /// a chat in session: "Reply…".
    private var placeholder: String {
        if isTerminal { return "describe the task…" }
        if let placeholderOverride { return placeholderOverride }
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
            }

            card
                .zIndex(1)

            if deck != .none, !isTerminal {
                ComposerDeckBand(variant: deck, inputText: $inputText)
                    .zIndex(0)
                    .transition(.move(edge: .top).combined(with: .opacity))
            }
        }
        .animation(DeckMotion.animation, value: deck)
        .sheet(isPresented: $isPlusSheetPresented) {
            ComposerPlusSheet(attachmentStore: attachmentStore, sessionId: sessionId)
        }
        .sheet(isPresented: $isModelPickerPresented) {
            ModelPickerSheet(modelStore: modelStore)
        }
        .sheet(isPresented: $isMicPrimingPresented) {
            PermissionPrimingSheet(permission: .microphone) { onVoiceMode() }
        }
        .sheet(item: $upgradeURL) { item in
            SafariView(url: item.url)
        }
        .onAppear {
            modelStore.fetchModelsIfNeeded()
            usageStore.fetchUsageIfNeeded()
            #if DEBUG
            // `-open-plus-sheet` / `-open-model-picker` (DEBUG only): jump
            // straight to a composer sheet for screenshots (simctl has no
            // tap injection).
            if launchArgumentEnabled("open-plus-sheet") { isPlusSheetPresented = true }
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

            HStack(alignment: .top, spacing: 6) {
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
                    onFocusChange: { isFocused = $0 }
                )
            }
            .padding(.horizontal, 16)
            .padding(.top, 14)
            .padding(.bottom, 4)

            controlsRow
                .padding(.vertical, 8)
                .overlay(alignment: .top) {
                    Rectangle().fill(Color("BorderSubtle")).frame(height: 1)
                }
                .padding(.horizontal, 8)
        }
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: 18, style: .continuous))
        .overlay(
            // Quiet card: hairline border that steps up on focus.
            RoundedRectangle(cornerRadius: 18, style: .continuous)
                .stroke(accentBorder ?? (isFocused ? Color("BorderStrong") : Color("BorderSubtle")), lineWidth: 1)
        )
        .shadow(color: .black.opacity(0.04), radius: 1, y: 1)
        .shadow(color: .black.opacity(0.03), radius: 4, y: 2)
    }

    private var controlsRow: some View {
        HStack(spacing: 6) {
            HStack(spacing: 4) {
                if showsAttach {
                    Button(action: {
                        UIImpactFeedbackGenerator(style: .light).impactOccurred()
                        isPlusSheetPresented = true
                    }) {
                        Image(systemName: "plus")
                            .font(.system(size: 20, weight: .medium))
                            .foregroundColor(Color("TextSecondary"))
                            .rotationEffect(.degrees(isPlusSheetPresented ? 45 : 0))
                            .frame(width: 44, height: 44)
                            .background(Color("FillNeutral"), in: Circle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Add files")
                }

                // How freely this chat works: Auto · Ask · Read-only (chat only).
                if showsPermission, mode == .chat || mode == .cowork {
                    ChatPermissionPill(sessionId: sessionId)
                }
            }
            .fixedSize()

            Spacer(minLength: 0)

            HStack(spacing: 8) {
                modelChip
                // How full this chat's context is, and the plan's usage.
                ComposerUsageMeter(sessionId: sessionId, refreshKey: isStreaming ? "streaming" : "idle")
                sendSlot
            }
        }
    }

    /// Model chip: provider mark + name (parenthetical dropped on the phone) + ▾.
    private var modelChip: some View {
        Button(action: { isModelPickerPresented = true }) {
            HStack(spacing: 4) {
                if modelStore.isLoading && modelStore.models.isEmpty {
                    ProgressView()
                        .controlSize(.mini)
                    Text("Loading...")
                        .font(.system(size: 14, weight: .medium))
                } else {
                    Image("MatrixLogo")
                        .resizable()
                        .scaledToFit()
                        .frame(width: 18, height: 18)
                        .frame(width: 20, height: 20)
                        .background(Color("FillNeutral"), in: RoundedRectangle(cornerRadius: 6))
                        .overlay(RoundedRectangle(cornerRadius: 6).stroke(Color("BorderSubtle"), lineWidth: 1))
                    Text(ComposerModelLabel.label(modelStore))
                        .font(.system(size: 14, weight: .medium))
                        .lineLimit(1)
                        .truncationMode(.tail)
                }
                Image(systemName: "chevron.down")
                    .font(.system(size: 10, weight: .semibold))
                    .opacity(0.8)
                    .rotationEffect(.degrees(isModelPickerPresented ? 180 : 0))
            }
            .foregroundColor(isTerminal ? TerminalTheme.dim : Color("TextSecondary"))
            .padding(.horizontal, 6)
            .frame(height: 44)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Model: \(ComposerModelLabel.label(modelStore))")
    }

    /// Phone: voice and send share one slot (ChatComposer phoneSendSlotTaken).
    @ViewBuilder
    private var sendSlot: some View {
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
                if storefront.allowsPurchaseLinks {
                    Button("Upgrade") {
                        upgradeURL = IdentifiableURL(url: AccountPlanStore.shared.upgradeURL)
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
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
        if DictationController.systemPromptsPending(), !AppPermission.microphone.hasPrimed {
            // App-owned explainer before the system prompts, once per install.
            isMicPrimingPresented = true
        } else {
            onVoiceMode()
        }
    }

    private func sendTapped() {
        guard canSend else { return }
        onSend(attachmentStore.attachments)
        attachmentStore.clear()
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
                UIImpactFeedbackGenerator(style: .light).impactOccurred()
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
        guard let id = store.selectedModelId, !id.isEmpty else { return "Default Model" }
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
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

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
        let textView = UITextView()
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

    final class Coordinator: NSObject, UITextViewDelegate {
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

        func textViewDidBeginEditing(_ textView: UITextView) {
            parent.onFocusChange?(true)
        }

        func textViewDidEndEditing(_ textView: UITextView) {
            parent.onFocusChange?(false)
        }
    }
}
