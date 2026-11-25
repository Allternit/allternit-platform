import SwiftUI

/// Owns the live voice conversation for a chat thread, so the compact quick
/// bar above the composer and the full-screen voice mode are two views of
/// the same session (expanding or collapsing never restarts the engine).
@MainActor
final class VoiceHost: ObservableObject {
    enum Presentation: Equatable {
        case none
        /// Compact bar above the composer.
        case bar
        /// Full-screen voice mode.
        case full
    }

    @Published var presentation: Presentation = .none
    @Published private(set) var viewModel: VoiceModeViewModel?
    let dictation = DictationController()
    let speaker = SpeechSpeaker.shared

    var isActive: Bool { viewModel != nil }

    /// A new call: the first orb on screen grows from a dot (SCOPE-B1 clip 3).
    /// Not published: the orb reads it on its first render and takes it once grown.
    private(set) var growPending = false

    /// The quick bar's orb, in window coordinates: where the full-screen orb
    /// travels from on expand and back to on minimize. The bar keeps its place
    /// (hidden) while full voice is up, so this stays current. `.zero` when no
    /// bar has been laid out. Not published (it moves with the keyboard).
    var barOrbFrame: CGRect = .zero

    /// Face sizes of the two orbs; the travel scales one into the other.
    static let barFaceSize: CGFloat = 28
    static let stageFaceSize: CGFloat = 148

    /// The grow has run.
    func finishGrow() { growPending = false }

    /// Bar ↔ full without the system cover slide: the voice views run the
    /// orb's own travel (VoiceModeView) and the bar's fade-in instead.
    func present(_ next: Presentation) {
        // Expanding mid-grow: the orb travels from the bar instead of growing again.
        if next == .full, presentation == .bar { growPending = false }
        var transaction = Transaction()
        transaction.disablesAnimations = true
        withTransaction(transaction) { presentation = next }
    }

    /// Starts a voice conversation in `presentation` (no-op if one is live —
    /// it just changes presentation).
    func start(chat: ChatViewModel, runtimeModelId: String?, effort: String?, bot: AgentRecord? = nil,
               presentation: Presentation = .bar) {
        if viewModel == nil {
            let model = VoiceModeViewModel(chatViewModel: chat, runtimeModelId: runtimeModelId, effort: effort)
            model.botVoice = bot?.kokoroVoiceId
            chat.voiceModel = model
            // end_call: the normal end flow, including the "Voice chat ended" card.
            model.onEndCall = { [weak self, weak chat] in
                if let seconds = self?.end() { chat?.appendVoiceSummary(durationSeconds: seconds) }
            }
            viewModel = model
            model.begin(dictation: dictation, speaker: speaker)
            // A new call: the first orb on screen grows from a dot.
            growPending = true
        }
        if presentation == .full || self.presentation == .full {
            present(presentation)
        } else {
            self.presentation = presentation
        }
    }

    /// Ends the session; returns its length in seconds for the summary card.
    @discardableResult
    func end() -> Int? {
        guard let viewModel else { return nil }
        let seconds = viewModel.endSession()
        self.viewModel = nil
        presentation = .none
        growPending = false
        barOrbFrame = .zero
        speaker.stop()
        return seconds
    }
}

/// Routes dictation / speaker / chat-stream changes into the live voice
/// view model (the wiring that used to live in the full-screen view alone).
struct VoiceHostWiring: ViewModifier {
    @ObservedObject var host: VoiceHost
    @ObservedObject var dictation: DictationController
    @ObservedObject var speaker: SpeechSpeaker
    @ObservedObject var chat: ChatViewModel

    func body(content: Content) -> some View {
        content
            .onChange(of: dictation.transcript) { _, transcript in
                host.viewModel?.liveTranscript = transcript
            }
            .onChange(of: dictation.isRecording) { _, isRecording in
                if !isRecording { host.viewModel?.dictationEnded() }
            }
            .onChange(of: dictation.errorMessage) { _, message in
                if let message { host.viewModel?.dictationFailed(message) }
            }
            .onChange(of: chat.messages) { _, messages in
                host.viewModel?.replyUpdated(messages)
            }
            .onChange(of: speaker.isSpeaking) { _, isSpeaking in
                if !isSpeaking { host.viewModel?.speakerFinished() }
            }
    }
}

extension View {
    func voiceHostWiring(_ host: VoiceHost, chat: ChatViewModel) -> some View {
        modifier(VoiceHostWiring(host: host, dictation: host.dictation, speaker: host.speaker, chat: chat))
    }
}

/// Quick voice bar: orb + status + mute / expand / end. Sits above the
/// composer while a voice conversation runs. A plain chat shows the thinking
/// mark in a small version of the default live mode look.
struct VoiceQuickBar: View {
    @ObservedObject var viewModel: VoiceModeViewModel
    /// Bot conversations: the bot's face in a small version of its live mode look.
    var bot: AgentRecord? = nil
    /// The session's host: the bar reports its orb's frame (the full-screen
    /// orb travels to and from it) and runs a new call's grow.
    var host: VoiceHost? = nil
    /// False while full voice is up: the bar holds its place, hidden, so the
    /// full-screen orb has a fixed spot to travel from and back to.
    var docked: Bool = true
    let onExpand: () -> Void
    let onEnd: () -> Void
    @ObservedObject private var looks = SpeakerLookStore.shared
    @ReducedMotion private var reduceMotion
    /// After minimize the orb lands on the bar first, then the rest fades in.
    @State private var chromeArrived = true

    var body: some View {
        HStack(spacing: 12) {
            Group {
                if let bot {
                    let look = looks.look(for: bot.id)
                    VoiceSpeakerView(phase: speakerPhase, muted: viewModel.isMuted, style: look.style, palette: look.palette,
                                     botColor: BotColorStore.shared.hex(for: bot).map { Color(hex: $0) }, size: VoiceHost.barFaceSize, compact: true) {
                        BotAvatarView(bot: bot, size: VoiceHost.barFaceSize)
                    }
                } else {
                    VoiceSpeakerView(phase: markPhase, muted: viewModel.isMuted, style: looks.style, palette: looks.palette,
                                     botColor: nil, size: VoiceHost.barFaceSize, compact: true) {
                        VoiceMarkFace(state: viewModel.state, size: VoiceHost.barFaceSize)
                    }
                }
            }
            .modifier(VoiceOrbGrow(host: host))
            .opacity(docked ? 1 : 0)
            .frame(width: 34, height: 34)
            .background(GeometryReader { proxy in
                Color.clear
                    .onAppear { host?.barOrbFrame = proxy.frame(in: .global) }
                    .onChange(of: proxy.frame(in: .global)) { _, frame in host?.barOrbFrame = frame }
            })

            Group {
                VStack(alignment: .leading, spacing: 1) {
                    Text(statusText)
                        .font(.subheadline.weight(.medium))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    if let secondary = secondaryText {
                        Text(secondary)
                            .font(.caption)
                            .foregroundColor(viewModel.statusLine != nil ? Theme.statusWarning : Color("TextSecondary"))
                            .lineLimit(1)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .accessibilityElement(children: .combine)
                .accessibilityLabel("Voice chat, \(statusText)")

                barButton(viewModel.isMuted ? "mic.slash.fill" : "mic.fill",
                          label: viewModel.isMuted ? "Unmute microphone" : "Mute microphone",
                          tint: viewModel.isMuted ? Theme.statusError : nil) { viewModel.toggleMute() }
                barButton("arrow.up.left.and.arrow.down.right", label: "Open full-screen voice", action: onExpand)
                barButton("xmark", label: "End voice chat", action: onEnd)
            }
            .opacity(docked && chromeArrived ? 1 : 0)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
        .background(
            RoundedRectangle(cornerRadius: Theme.radiusMD)
                .fill(Color("BgSecondary"))
                .overlay(RoundedRectangle(cornerRadius: Theme.radiusMD).stroke(Theme.borderWarmDefault, lineWidth: 1))
                .opacity(docked && chromeArrived ? 1 : 0)
        )
        .padding(.horizontal, 12)
        .padding(.bottom, 6)
        .allowsHitTesting(docked)
        .accessibilityHidden(!docked)
        .onAppear { chromeArrived = docked }
        .onChange(of: docked) { _, isDocked in
            if isDocked {
                // The full-screen orb has just landed on this one; the bar fades in round it.
                withAnimation(reduceMotion ? Motion.fade : .easeOut(duration: 0.2)) { chromeArrived = true }
            } else {
                chromeArrived = false
            }
        }
    }

    private var speakerPhase: VoiceSpeakerView<BotAvatarView>.Phase {
        if viewModel.isConnecting { return .connecting }
        switch viewModel.state {
        case .idle: return .idle
        case .listening: return .listening
        case .thinking: return .thinking
        case .speaking: return .speaking
        }
    }

    /// `Phase` is nested in the generic view, so the mark face has its own.
    private var markPhase: VoiceSpeakerView<VoiceMarkFace>.Phase {
        if viewModel.isConnecting { return .connecting }
        switch viewModel.state {
        case .idle: return .idle
        case .listening: return .listening
        case .thinking: return .thinking
        case .speaking: return .speaking
        }
    }

    private var statusText: String {
        if viewModel.isConnecting {
            if let p = viewModel.modelProgress { return "Downloading voice model \(Int(p * 100))%" }
            return "Connecting…"
        }
        switch viewModel.state {
        case .idle: return viewModel.isMuted ? "Muted" : "Voice paused"
        case .listening: return viewModel.liveTranscript.isEmpty ? "Listening…" : viewModel.liveTranscript
        case .thinking: return "Thinking…"
        case .speaking: return "Speaking…"
        }
    }

    private var secondaryText: String? {
        viewModel.statusLine ?? viewModel.engineLabel
    }

    private func barButton(_ systemName: String, label: String, tint: Color? = nil, action: @escaping () -> Void) -> some View {
        Button(action: {
            Haptics.light()
            action()
        }) {
            Image(systemName: systemName)
                .font(.system(size: 14, weight: .semibold))
                .foregroundColor(tint ?? Color("TextPrimary"))
                .frame(width: 36, height: 36)
                .background(.ultraThinMaterial, in: Circle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }
}
