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

    /// Starts a voice conversation in `presentation` (no-op if one is live —
    /// it just changes presentation).
    func start(chat: ChatViewModel, runtimeModelId: String?, effort: String?, presentation: Presentation = .bar) {
        if viewModel == nil {
            let model = VoiceModeViewModel(chatViewModel: chat, runtimeModelId: runtimeModelId, effort: effort)
            viewModel = model
            model.begin(dictation: dictation, speaker: speaker)
        }
        self.presentation = presentation
    }

    /// Ends the session; returns its length in seconds for the summary card.
    @discardableResult
    func end() -> Int? {
        guard let viewModel else { return nil }
        let seconds = viewModel.endSession()
        self.viewModel = nil
        presentation = .none
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
/// composer while a voice conversation runs.
struct VoiceQuickBar: View {
    @ObservedObject var viewModel: VoiceModeViewModel
    let onExpand: () -> Void
    let onEnd: () -> Void

    var body: some View {
        HStack(spacing: 12) {
            ThinkingOrb(state: orbState, size: .avatar, paused: viewModel.state == .idle)
                .scaleEffect(0.5)
                .frame(width: 34, height: 34)
                .accessibilityHidden(true)

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
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
        .background(Color("BgSecondary"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
        .overlay(RoundedRectangle(cornerRadius: Theme.radiusMD).stroke(Theme.borderWarmDefault, lineWidth: 1))
        .padding(.horizontal, 12)
        .padding(.bottom, 6)
    }

    private var orbState: ThinkingOrbState {
        switch viewModel.state {
        case .idle, .listening: return .listening
        case .thinking: return .breathing
        case .speaking: return .composing
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
            UIImpactFeedbackGenerator(style: .light).impactOccurred()
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
