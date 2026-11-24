import SwiftUI

/// The chief dock (product shape C) at the bottom of the Workspace floor: the
/// main bot's face and name, a hold-to-talk mic, the Autopilot switch, and the
/// main bot's open suggestion ("Yes, do it" / "Not now").
///
/// Nothing new underneath: the mic is the chat composer's dictation
/// (`DictationController`, ComposerView's mic), the words go to the main bot's
/// main chat through the floor's `OfficePanelThread.send`, the suggestion uses
/// `BotModeStore.approve` / `decline` (threads approve/decline), and "Pick
/// your main bot" opens `MainBotSetupSheet` (PrimaryBotSetupDialog).
struct ChiefDockView: View {
    enum Account { case signedIn, signedOut }

    let account: Account
    /// The main bot; nil = none chosen yet.
    let mainBot: AgentRecord?
    /// The main bot's open suggestion (a proposed thread), if any.
    let suggestion: BotThread?
    @ObservedObject var autopilot: FloorAutopilotModel
    /// Sends the words to the main bot's main chat; false when it couldn't.
    let onSend: @MainActor (String) async -> Bool
    let onOpenBot: (String) -> Void
    let onPickMainBot: () -> Void
    let onSignIn: () -> Void
    let onOpenAutopilot: () -> Void
    /// The suggestion was accepted: the floor shows a done bubble.
    let onSuggestionStarted: (BotThread) -> Void

    private enum Mic: Equatable { case idle, holding, toggled }

    @StateObject private var dictation = DictationController()
    @ObservedObject private var status = BotStatusStore.shared
    @Environment(\.dynamicTypeSize) private var typeSize
    @ReducedMotion private var reduceMotion

    @State private var mic: Mic = .idle
    @State private var pressStart: Date?
    @State private var cancelArmed = false
    @State private var sendState: String?
    @State private var suggestionBusy: String?
    @State private var suggestionError: String?

    /// A press shorter than this is a tap: it keeps listening until the next tap.
    private static let tapThreshold: TimeInterval = 0.35
    /// Dragging this far from the mic cancels the recording.
    private static let cancelDistance: CGFloat = 70

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if let suggestion, let mainBot, account == .signedIn {
                suggestionCard(suggestion, bot: mainBot)
            }
            switch (account, mainBot) {
            case (.signedOut, _): signedOutRow
            case (.signedIn, nil): pickRow
            case (.signedIn, let bot?): botRow(bot)
            }
            if mic != .idle || !(sendState ?? "").isEmpty || dictation.errorMessage != nil {
                micStatus
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 18, style: .continuous).fill(Color("BgPanel")))
        .overlay(RoundedRectangle(cornerRadius: 18, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
        .shadow(color: .black.opacity(0.1), radius: 10, y: 3)
        .onChange(of: dictation.isRecording) { _, recording in
            // Silence or a final result ended the recording: send what was heard.
            if !recording, mic != .idle, pressStart == nil { finish() }
        }
        .onChange(of: dictation.errorMessage) { _, message in
            if message != nil { mic = .idle; pressStart = nil }
        }
        .onDisappear { if mic != .idle { cancel() } }
    }

    // MARK: - Rows

    private var signedOutRow: some View {
        HStack(spacing: 10) {
            Image(systemName: "person.crop.circle")
                .font(.title2)
                .foregroundColor(Color("TextSecondary"))
                .accessibilityHidden(true)
            Text("Sign in to talk to your main bot")
                .font(.subheadline)
                .foregroundColor(Color("TextPrimary"))
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
            Button("Sign in", action: onSignIn)
                .buttonStyle(QuestionCardButtonStyle(kind: .primary))
        }
    }

    private var pickRow: some View {
        Button(action: onPickMainBot) {
            HStack(spacing: 10) {
                Image(systemName: "crown")
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 36, height: 36)
                    .background(Circle().fill(Color("FillNeutral")))
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 1) {
                    Text("Pick your main bot")
                        .font(.subheadline.weight(.semibold))
                        .foregroundColor(Color("TextPrimary"))
                    Text("It runs your floor: talk to it, and let it handle what you allow.")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 0)
                Image(systemName: "chevron.right")
                    .font(.footnote.weight(.semibold))
                    .foregroundColor(Color("TextSecondary"))
                    .accessibilityHidden(true)
            }
            .frame(minHeight: 44)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Pick your main bot")
        .accessibilityHint("Opens main bot setup")
    }

    @ViewBuilder
    private func botRow(_ bot: AgentRecord) -> some View {
        if typeSize.isAccessibilitySize {
            VStack(alignment: .leading, spacing: 10) {
                identity(bot)
                HStack(spacing: 12) {
                    autopilotControl
                    Spacer(minLength: 0)
                    micButton(bot)
                }
            }
        } else {
            HStack(spacing: 10) {
                identity(bot)
                Spacer(minLength: 4)
                autopilotControl
                micButton(bot)
            }
        }
    }

    private func identity(_ bot: AgentRecord) -> some View {
        Button { onOpenBot(bot.id) } label: {
            HStack(spacing: 10) {
                BotAvatarView(bot: bot, size: 36, working: status.isWorking(for: bot.id))
                VStack(alignment: .leading, spacing: 1) {
                    BotNameText(bot.botDisplayName, size: 15)
                        .font(.subheadline.weight(.semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Text(subtitle)
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                }
            }
            .frame(minHeight: 44)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(bot.botDisplayName), your main bot. \(subtitle)")
        .accessibilityHint("Opens \(bot.botDisplayName)'s thread")
    }

    private var subtitle: String {
        switch autopilot.state {
        case .offline where autopilot.autopilot == nil: return "You're offline"
        case .failed where autopilot.autopilot == nil: return "Autopilot unavailable"
        default: return autopilot.enabled ? "Autopilot on" : "Autopilot off"
        }
    }

    /// "Autopilot" switch plus the info control that opens the rules.
    private var autopilotControl: some View {
        HStack(spacing: 2) {
            if autopilot.saving || (autopilot.state == .loading && autopilot.autopilot == nil) {
                ProgressView()
                    .controlSize(.small)
                    .frame(width: 51)
                    .accessibilityLabel("Autopilot loading")
            } else {
                Toggle("Autopilot", isOn: Binding(
                    get: { autopilot.enabled },
                    set: { on in Task { await autopilot.setEnabled(on) } }
                ))
                .labelsHidden()
                .tint(Color("AccentPrimary"))
                .disabled(autopilot.autopilot == nil)
                .accessibilityLabel("Autopilot")
                .accessibilityValue(autopilot.enabled ? "On" : "Off")
            }
            Button(action: onOpenAutopilot) {
                Image(systemName: "info.circle")
                    .font(.system(size: 17))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 36, height: 44)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Autopilot rules")
        }
    }

    // MARK: - Hold to talk

    /// Hold = record, release = send, drag away = cancel. A short tap toggles
    /// (tap to start, tap again to send); VoiceOver's activate does the same.
    private func micButton(_ bot: AgentRecord) -> some View {
        let active = mic != .idle
        return ZStack {
            if active && !reduceMotion {
                Circle()
                    .stroke(Color("AccentPrimary").opacity(0.35), lineWidth: 6)
                    .frame(width: 52, height: 52)
                    .transition(.opacity)
            }
            Circle()
                .fill(cancelArmed ? Theme.statusError : active ? Color("AccentPrimary") : Color("FillNeutral"))
                .frame(width: 44, height: 44)
            Image(systemName: cancelArmed ? "xmark" : active ? "waveform" : "mic.fill")
                .font(.system(size: 17, weight: .semibold))
                .foregroundColor(active || cancelArmed ? .white : Color("TextPrimary"))
        }
        .frame(width: 52, height: 52)
        .contentShape(Circle())
        .gesture(
            DragGesture(minimumDistance: 0)
                .onChanged { value in
                    if pressStart == nil {
                        pressStart = Date()
                        if mic == .idle { begin() }
                    }
                    let distance = hypot(value.translation.width, value.translation.height)
                    let armed = mic != .idle && distance > Self.cancelDistance
                    if armed != cancelArmed {
                        cancelArmed = armed
                        if armed { Haptics.warning() }
                    }
                }
                .onEnded { _ in released() }
        )
        .accessibilityElement(children: .ignore)
        .accessibilityAddTraits(.isButton)
        .accessibilityLabel(active ? "Stop and send to \(bot.botDisplayName)" : "Talk to \(bot.botDisplayName)")
        .accessibilityValue(active ? "Listening" : "")
        .accessibilityHint(active ? "Sends what you said" : "Hold to talk, or activate to start and again to send")
        .accessibilityAction { toggleForAccessibility() }
        .animation(reduceMotion ? nil : .easeOut(duration: 0.15), value: active)
    }

    private var micStatus: some View {
        VStack(alignment: .leading, spacing: 2) {
            if mic != .idle {
                if !dictation.transcript.isEmpty {
                    Text(dictation.transcript)
                        .font(.footnote)
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(2)
                }
                Text(cancelArmed ? "Release to cancel"
                     : mic == .holding ? "Listening… release to send, drag away to cancel"
                     : "Listening… tap the mic to send")
                    .font(.caption)
                    .foregroundColor(cancelArmed ? Theme.statusError : Color("TextSecondary"))
            } else if let error = dictation.errorMessage {
                Text(plainErrorText(message: error))
                    .font(.caption)
                    .foregroundColor(Theme.statusError)
            } else if let sendState {
                Text(sendState)
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .combine)
    }

    private func begin() {
        Haptics.light()
        mic = .holding
        sendState = nil
        Task { await dictation.start() }
    }

    private func released() {
        let started = pressStart
        pressStart = nil
        if cancelArmed {
            cancel()
            return
        }
        switch mic {
        case .idle:
            break
        case .holding:
            if let started, Date().timeIntervalSince(started) < Self.tapThreshold {
                // A tap: keep listening until the next tap.
                mic = .toggled
            } else {
                finish()
            }
        case .toggled:
            finish()
        }
    }

    private func toggleForAccessibility() {
        if mic == .idle {
            begin()
            mic = .toggled
        } else {
            finish()
        }
    }

    private func cancel() {
        dictation.stop()
        mic = .idle
        cancelArmed = false
        pressStart = nil
        Haptics.soft()
    }

    private func finish() {
        let text = dictation.transcript.trimmingCharacters(in: .whitespacesAndNewlines)
        dictation.stop()
        mic = .idle
        cancelArmed = false
        guard !text.isEmpty, let name = mainBot?.botDisplayName else { return }
        sendState = "Sending to \(name)…"
        Task {
            let sent = await onSend(text)
            sendState = sent ? "Sent to \(name)" : "Couldn't send. Try again."
            if sent { Haptics.success() } else { Haptics.error() }
            try? await Task.sleep(for: .seconds(4))
            if sendState == "Sent to \(name)" { sendState = nil }
        }
    }

    // MARK: - Suggestion

    private func suggestionCard(_ thread: BotThread, bot: AgentRecord) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                Image(systemName: "lightbulb.fill")
                    .font(.caption)
                    .accessibilityHidden(true)
                Text("\(bot.botDisplayName) suggests")
                    .font(.caption.weight(.semibold))
            }
            .foregroundColor(Color("TextSecondary"))
            Text(thread.displayTitle)
                .font(.subheadline.weight(.semibold))
                .foregroundColor(Color("TextPrimary"))
                .fixedSize(horizontal: false, vertical: true)
            if let line = thread.statusLine, !line.isEmpty {
                Text(line)
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(2)
            }
            HStack(spacing: 6) {
                suggestionButton("Not now", id: "no", kind: .quiet, thread: thread) {
                    try await BotModeStore.shared.decline(thread)
                }
                suggestionButton("Yes, do it", id: "yes", kind: .primary, thread: thread) {
                    try await BotModeStore.shared.approve(thread)
                    onSuggestionStarted(thread)
                }
            }
            if let suggestionError {
                Text(suggestionError)
                    .font(.caption)
                    .foregroundColor(Theme.statusError)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("FillNeutral")))
        .accessibilityElement(children: .contain)
        .accessibilityLabel("\(bot.botDisplayName) suggests: \(thread.displayTitle)")
    }

    private func suggestionButton(_ title: String, id: String, kind: QuestionCardButtonStyle.Kind, thread: BotThread,
                                  run: @escaping @MainActor () async throws -> Void) -> some View {
        Button {
            suggestionBusy = id
            suggestionError = nil
            Task {
                do {
                    try await run()
                    Haptics.success()
                } catch {
                    suggestionError = "Couldn't send that. \(FriendlyErrorMessage.from(error.localizedDescription))"
                    Haptics.error()
                }
                suggestionBusy = nil
            }
        } label: {
            if suggestionBusy == id {
                ProgressView().controlSize(.small)
            } else {
                Text(title)
            }
        }
        .buttonStyle(QuestionCardButtonStyle(kind: kind))
        .disabled(suggestionBusy != nil)
        .accessibilityLabel("\(title): \(thread.displayTitle)")
    }
}
