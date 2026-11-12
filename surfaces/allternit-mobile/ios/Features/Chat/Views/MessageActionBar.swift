import AVFoundation
import SwiftUI
import UIKit

// MARK: - Message action bar (Phase 8, Claude iOS parity)
//
// The small icon row Claude renders under a completed assistant reply:
// copy, share, speak, thumbs up, thumbs down, retry. Hosted by MessageRow
// under assistant messages (the long-press contextMenu stays as-is); retry
// only appears on the LAST assistant message.

/// Read-aloud controller behind the speak button. One shared instance
/// app-wide so two rows can never talk over each other; tapping speak on
/// another message replaces the current utterance, tapping it again stops.
@MainActor
final class SpeechSpeaker: NSObject, ObservableObject, AVSpeechSynthesizerDelegate {
    static let shared = SpeechSpeaker()

    /// The message currently being read aloud (nil = idle) — the row swaps
    /// its speaker glyph for a stop glyph while this matches its id.
    @Published private(set) var speakingMessageId: String? = nil
    /// True while the synthesizer has anything queued or in flight —
    /// including voice-mode chunks (Phase 7b), which carry no message id.
    @Published private(set) var isSpeaking: Bool = false

    private let synthesizer = AVSpeechSynthesizer()

    override init() {
        super.init()
        synthesizer.delegate = self
    }

    func toggle(messageId: String, text: String) {
        if speakingMessageId == messageId {
            stop()
            return
        }
        synthesizer.stopSpeaking(at: .immediate)
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        speakingMessageId = messageId
        isSpeaking = true
        synthesizer.speak(makeUtterance(trimmed))
    }

    /// Voice mode (Phase 7b): sentence-chunked playback of a streamed reply.
    /// Unlike `toggle`, chunks QUEUE on the synthesizer instead of replacing
    /// each other, so a multi-sentence reply is read end-to-end.
    func speakChunk(_ text: String) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        isSpeaking = true
        synthesizer.speak(makeUtterance(trimmed))
    }

    /// Builds an utterance honoring Settings → Voice: the voice-mode picker
    /// selection (an AVSpeechSynthesisVoice identifier) and the speed
    /// setting (1.0 = the system default rate).
    private func makeUtterance(_ text: String) -> AVSpeechUtterance {
        let settings = SettingsStore.shared
        let utterance = AVSpeechUtterance(string: text)
        if let identifier = settings.voiceIdentifier,
           let voice = AVSpeechSynthesisVoice(identifier: identifier) {
            utterance.voice = voice
        } else {
            utterance.voice = AVSpeechSynthesisVoice(language: nil) // system default
        }
        let rate = AVSpeechUtteranceDefaultSpeechRate * Float(settings.speechSpeed)
        utterance.rate = min(max(rate, AVSpeechUtteranceMinimumSpeechRate), AVSpeechUtteranceMaximumSpeechRate)
        return utterance
    }

    func stop() {
        synthesizer.stopSpeaking(at: .immediate)
        speakingMessageId = nil
        isSpeaking = false
    }

    // Delegate callbacks are called from the synthesizer's queue, which may
    // not be the main actor. Hop to the main actor with a weak capture so the
    // non-Sendable synthesizer/self are not passed across isolation.
    nonisolated func speechSynthesizer(_ synthesizer: AVSpeechSynthesizer, didFinish utterance: AVSpeechUtterance) {
        Task { @MainActor [weak self] in
            guard let self else { return }
            self.speakingMessageId = nil
            // Queued chunks keep isSpeaking true until the LAST one drains.
            self.isSpeaking = self.synthesizer.isSpeaking
        }
    }

    nonisolated func speechSynthesizer(_ synthesizer: AVSpeechSynthesizer, didCancel utterance: AVSpeechUtterance) {
        Task { @MainActor [weak self] in
            guard let self else { return }
            self.speakingMessageId = nil
            self.isSpeaking = self.synthesizer.isSpeaking
        }
    }
}

/// System share sheet for the message text.
private struct ActivityView: UIViewControllerRepresentable {
    let text: String

    func makeUIViewController(context: Context) -> UIActivityViewController {
        UIActivityViewController(activityItems: [text], applicationActivities: nil)
    }

    func updateUIViewController(_ uiViewController: UIActivityViewController, context: Context) {}
}

struct MessageActionBar: View {
    let message: MessageRecord
    /// Retry (regenerate) only makes sense on the most recent assistant
    /// reply — ChatView passes this for that row only.
    let isLastAssistant: Bool
    let onRegenerate: () -> Void
    /// Opens the thread's share-link sheet (nil hides the menu entry).
    var onShareLink: (() -> Void)? = nil

    /// 👍/👎 selection. LOCAL-ONLY: the backend has no message-feedback
    /// endpoint (verified against cmd/allternit-api routes, 2026-07) — the
    /// choice is visual acknowledgement until one ships.
    @State private var feedback: Int = 0 // -1 down, 0 none, +1 up
    @State private var isSharePresented = false
    @StateObject private var speaker = SpeechSpeaker.shared

    var body: some View {
        HStack(spacing: 4) {
            actionButton("doc.on.doc", label: "Copy") {
                UIPasteboard.general.string = message.content
            }
            if let onShareLink {
                Menu {
                    Button { isSharePresented = true } label: { Label("Share text", systemImage: "square.and.arrow.up") }
                    Button { onShareLink() } label: { Label("Share link…", systemImage: "link") }
                } label: {
                    Image(systemName: "square.and.arrow.up")
                        .font(.system(size: 13, weight: .medium))
                        .foregroundColor(Color("TextSecondary"))
                        .frame(width: 30, height: 30)
                        .contentShape(Rectangle())
                }
                .accessibilityLabel("Share")
            } else {
                actionButton("square.and.arrow.up", label: "Share") {
                    isSharePresented = true
                }
            }
            actionButton(
                speaker.speakingMessageId == message.id ? "stop.fill" : "speaker.wave.2",
                label: speaker.speakingMessageId == message.id ? "Stop" : "Read aloud"
            ) {
                speaker.toggle(messageId: message.id, text: message.content)
            }
            actionButton(feedback == 1 ? "hand.thumbsup.fill" : "hand.thumbsup", label: "Good response") {
                feedback = feedback == 1 ? 0 : 1
            }
            actionButton(feedback == -1 ? "hand.thumbsdown.fill" : "hand.thumbsdown", label: "Bad response") {
                feedback = feedback == -1 ? 0 : -1
            }
            if isLastAssistant {
                actionButton("arrow.clockwise", label: "Retry") {
                    onRegenerate()
                }
            }
        }
        .padding(.leading, 8)
        .sheet(isPresented: $isSharePresented) {
            ActivityView(text: message.content)
        }
    }

    private func actionButton(_ systemName: String, label: String, action: @escaping () -> Void) -> some View {
        Button(action: {
            let generator = UISelectionFeedbackGenerator()
            generator.selectionChanged()
            action()
        }) {
            Image(systemName: systemName)
                .font(.system(size: 13, weight: .medium))
                .foregroundColor(Color("TextSecondary"))
                .frame(width: 30, height: 30)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }
}
