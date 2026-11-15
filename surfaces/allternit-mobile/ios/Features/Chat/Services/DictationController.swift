import AVFoundation
import Foundation
import Speech

/// Live dictation engine for the composer mic (Claude parity): mic tap →
/// live transcription into the editable text field.
///
/// Wraps SFSpeechRecognizer + AVAudioEngine behind a MainActor surface.
/// Partial results publish into `transcript` as they arrive — the composer
/// renders them as base-text + partial so Speech's word corrections replace
/// earlier words instead of appending. A short silence auto-stops the
/// session; `stop()` keeps the last transcript for the field to fold in.
///
/// Swift 6 notes: the audio tap runs on the render thread and the Speech
/// result handler on an arbitrary queue, so both cross the actor boundary
/// carrying Sendable values only; deinit inlines resource cleanup (see its
/// comment — `isolated deinit` is unavailable before Swift 6.1).
@MainActor
final class DictationController: ObservableObject {
    /// Latest transcript (partial results included); empty until Speech
    /// delivers the first result.
    @Published private(set) var transcript: String = ""
    @Published private(set) var isRecording: Bool = false
    /// User-facing failure (permission denied, engine start, recognition
    /// error); cleared at the next `start()`.
    @Published var errorMessage: String? = nil

    /// Recognizer is built per session from the user's speech-language
    /// setting (Settings → Voice, or the dictation onboarding sheet); nil =
    /// System default (Locale.current). Rebuilt at every `start()` so a
    /// language change applies without recreating the controller.
    /// Unsupported locales remain unavailable instead of silently switching
    /// languages.
    private var speechRecognizer: SFSpeechRecognizer? {
        let locale = SettingsStore.shared.speechLanguage
            .map { Locale(identifier: $0.rawValue) } ?? .current
        return SFSpeechRecognizer(locale: locale)
    }
    // `nonisolated(unsafe)`: deinit must tear these down, and Swift 6.0's
    // nonisolated deinit may only read Sendable stored properties (these
    // AVFoundation/Speech types aren't). Safe by invariant: private, every
    // other access is MainActor-isolated, and deinit has exclusive access.
    nonisolated(unsafe) private var audioEngine: AVAudioEngine?
    nonisolated(unsafe) private var recognitionRequest: SFSpeechAudioBufferRecognitionRequest?
    nonisolated(unsafe) private var recognitionTask: SFSpeechRecognitionTask?
    /// Silence auto-stop; re-armed by every partial result.
    private var silenceTask: Task<Void, Never>?

    /// No new partials for this long → end the session (stop-on-silence).
    private static let silenceTimeoutNs: UInt64 = 2_500_000_000

    /// `isolated deinit` needs Swift 6.1+ (Xcode 16.2 ships 6.0). A plain
    /// deinit may still READ stored properties — the object is uniquely
    /// referenced here — it just can't call MainActor methods like
    /// `teardown()`, so the resource cleanup is inlined (no @Published
    /// writes; nobody can observe them during deinit anyway).
    deinit {
        silenceTask?.cancel()
        recognitionTask?.cancel()
        recognitionRequest?.endAudio()
        if let engine = audioEngine {
            engine.stop()
            engine.inputNode.removeTap(onBus: 0)
        }
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    }

    // MARK: - Start / stop

    /// Requests permissions (first run surfaces both system prompts), then
    /// starts capture. No-op while already recording.
    func start() async {
        guard !isRecording else { return }
        errorMessage = nil
        transcript = ""

        guard await requestAuthorization() else { return }
        guard let speechRecognizer else {
            errorMessage = "Speech recognition doesn't support the current language."
            return
        }
        guard speechRecognizer.isAvailable else {
            errorMessage = "Speech recognition isn't available right now."
            return
        }

        do {
            try beginRecording(using: speechRecognizer)
            isRecording = true
            scheduleSilenceStop()
        } catch {
            teardown()
            errorMessage = "Couldn't start dictation: \(error.localizedDescription)"
        }
    }

    /// End tap: stop capture, keep the last transcript.
    func stop() {
        guard isRecording else { return }
        teardown()
    }

    /// Shared teardown for end tap / final result / silence / error: stops
    /// the engine, ends the request, and deactivates the audio session.
    private func teardown() {
        silenceTask?.cancel()
        silenceTask = nil
        recognitionTask?.cancel()
        recognitionTask = nil
        recognitionRequest?.endAudio()
        recognitionRequest = nil
        if let engine = audioEngine {
            engine.stop()
            engine.inputNode.removeTap(onBus: 0)
        }
        audioEngine = nil
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
        isRecording = false
    }

    // MARK: - Authorization

    /// True while `start()` would still surface a system prompt (Speech or
    /// mic not yet determined). The composer consults this BEFORE calling
    /// `start()` so it can show the app-owned priming sheet first (Claude
    /// iOS parity: an explainer ahead of the iOS dialogs, shown once per
    /// permission via the AppPermission UserDefaults flag).
    nonisolated static func systemPromptsPending() -> Bool {
        SFSpeechRecognizer.authorizationStatus() == .notDetermined
            || AVAudioApplication.shared.recordPermission == .undetermined
    }

    /// Speech + mic authorization; returns false (with `errorMessage` set)
    /// when either is denied.
    private func requestAuthorization() async -> Bool {
        let speechStatus = await withCheckedContinuation { (continuation: CheckedContinuation<SFSpeechRecognizerAuthorizationStatus, Never>) in
            SFSpeechRecognizer.requestAuthorization { status in
                continuation.resume(returning: status)
            }
        }
        guard speechStatus == .authorized else {
            errorMessage = "Speech recognition is off. Enable it in Settings to dictate messages."
            return false
        }

        // iOS 17+ async permission API (AVAudioSession's is deprecated).
        guard await AVAudioApplication.requestRecordPermission() else {
            errorMessage = "Microphone access is off. Enable it in Settings to dictate messages."
            return false
        }
        return true
    }

    // MARK: - Capture

    private func beginRecording(using speechRecognizer: SFSpeechRecognizer) throws {
        recognitionTask?.cancel()
        recognitionTask = nil

        let audioSession = AVAudioSession.sharedInstance()
        try audioSession.setCategory(.record, mode: .measurement, options: .duckOthers)
        try audioSession.setActive(true, options: .notifyOthersOnDeactivation)

        let request = SFSpeechAudioBufferRecognitionRequest()
        request.shouldReportPartialResults = true
        recognitionRequest = request

        let engine = AVAudioEngine()
        audioEngine = engine
        let inputNode = engine.inputNode

        // The tap block is @Sendable and runs on the audio render thread;
        // the non-Sendable request crosses in a box. It is only ever
        // appended there and ended on the MainActor — Speech's documented
        // producer pattern (Apple's own sample does the same).
        let requestBox = UncheckedSendableBox(value: request)
        inputNode.installTap(onBus: 0, bufferSize: 1024, format: inputNode.outputFormat(forBus: 0)) { buffer, _ in
            requestBox.value.append(buffer)
            // Live mode's speaker look follows the mic.
            VoiceLevels.shared.pushMic(buffer)
        }

        engine.prepare()
        try engine.start()

        recognitionTask = speechRecognizer.recognitionTask(with: request) { [weak self] result, error in
            // Nonisolated and @Sendable — extract Sendable values and hop
            // to the MainActor (SFSpeechRecognitionResult isn't Sendable).
            let text = result?.bestTranscription.formattedString
            let isFinal = result?.isFinal ?? false
            Task { @MainActor in
                self?.handleResult(text: text, isFinal: isFinal, error: error)
            }
        }
    }

    /// Applies one Speech callback on the MainActor. Late deliveries after
    /// `teardown()` are dropped so a stale final can't resurrect text the
    /// composer already sent.
    private func handleResult(text: String?, isFinal: Bool, error: (any Error)?) {
        guard isRecording else { return }

        if let text {
            transcript = text
            if isFinal {
                teardown()
                return
            }
            scheduleSilenceStop()
        }

        guard let error else { return }
        // Benign assistant-domain codes from natural teardown (cancel on
        // our own stop, "no speech detected", short-utterance retry) —
        // finish silently; anything else surfaces once via `errorMessage`.
        let nsError = error as NSError
        let benignCodes: Set<Int> = [203, 209, 1110]
        if !(nsError.domain == "kAFAssistantErrorDomain" && benignCodes.contains(nsError.code)) {
            errorMessage = "Dictation failed: \(error.localizedDescription)"
        }
        teardown()
    }

    /// Re-arms the silence auto-stop; every partial result pushes it out.
    private func scheduleSilenceStop() {
        silenceTask?.cancel()
        silenceTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: Self.silenceTimeoutNs)
            guard let self, !Task.isCancelled else { return }
            self.teardown()
        }
    }
}

/// Swift 6 capture helper: @Sendable closures (the audio tap) can't capture
/// the non-Sendable SFSpeechAudioBufferRecognitionRequest. The boxed value
/// is only touched from the tap thread (`append`) and the MainActor
/// (`endAudio`), matching Speech's documented usage.
private struct UncheckedSendableBox<Value>: @unchecked Sendable {
    let value: Value
}
