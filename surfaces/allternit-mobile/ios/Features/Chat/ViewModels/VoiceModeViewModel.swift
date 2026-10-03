import AVFoundation
import Foundation
import SwiftUI

/// Turn state machine behind VoiceModeView (Phase 7b, Claude iOS voice-mode
/// parity). One turn: user speaks → live transcript (on-device Speech via
/// DictationController) → transcript sent through the EXISTING chat stream
/// path (ChatViewModel.sendMessage — the conversation stays in the normal
/// text thread) → the streamed reply is read aloud sentence-chunked
/// (SpeechSpeaker) → back to listening (hands-free) or idle (push-to-talk).
///
/// The view owns the DictationController/SpeechSpeaker objects and forwards
/// their changes here via SwiftUI `onChange` (the same wiring pattern
/// ComposerView uses for dictation) — no Combine inside the view model.
/// Errors surface in-place as a small status line; nothing here crashes.
@MainActor
final class VoiceModeViewModel: ObservableObject {
    /// Gradient-driving turn state. idle → listening → thinking (stream in
    /// flight) → speaking → listening…
    enum TurnState: String, Equatable, Sendable {
        case idle
        case listening
        case thinking
        case speaking
    }

    @Published private(set) var state: TurnState = .idle
    /// Live STT partial, rendered in italics at the bottom of the takeover.
    @Published var liveTranscript: String = ""
    /// The current reply's streamed text, rendered in serif.
    @Published private(set) var replyText: String = ""
    /// Mute gates BOTH directions: recognition won't (re)start and replies
    /// aren't read aloud while on.
    @Published private(set) var isMuted: Bool = false
    /// Small in-place error/status line (permission, engine, stream).
    @Published private(set) var statusLine: String? = nil
    /// "Cloud voice" / "On this device" once a session engine is running;
    /// nil on the system-recognizer path. Shown under the orb.
    @Published private(set) var engineLabel: String? = nil
    /// True while an engine is being chosen/connected (or its model is
    /// downloading) — the view shows "Connecting…" instead of "Tap to talk".
    @Published private(set) var isConnecting: Bool = false
    /// 0…1 while the on-device voice model downloads.
    @Published private(set) var modelProgress: Double? = nil

    private let chatViewModel: ChatViewModel
    private let runtimeModelId: String?
    private let effort: String?
    // Owned by the view (its @StateObjects); weak so teardown can't keep
    // audio resources alive past the cover's dismissal.
    private weak var dictation: DictationController?
    private weak var speaker: SpeechSpeaker?

    /// The in-flight assistant reply's message id (captured right after
    /// sendMessage appends its placeholder).
    private var replyMessageId: String? = nil

    // Voice Session path (cloud or on-device engine). nil = system path.
    private var sessionClient: VoiceSessionClient?
    private var startTask: Task<Void, Never>?
    private var utteranceId = ""
    private var spokenLength = 0
    private var currentSpeakId: String?
    private var inSession: Bool { sessionClient != nil }
    private var startedAt: Date = Date()
    /// False once `endSession()` runs — late dictation/speech callbacks are
    /// dropped after the cover is gone.
    private var isActive: Bool = false
    /// Set when a dictation session is torn down by mute/interruption so its
    /// end isn't treated as a finished user turn.
    private var suppressNextDictationEnd: Bool = false
    /// `nonisolated(unsafe)` so deinit can unregister — same Swift 6.0
    /// deinit constraint DictationController documents (non-Sendable stored
    /// properties can't be read from a nonisolated deinit; here access is
    /// MainActor-only everywhere else and deinit has exclusive access).
    nonisolated(unsafe) private var interruptionObserver: NSObjectProtocol? = nil

    init(chatViewModel: ChatViewModel, runtimeModelId: String? = nil, effort: String? = nil) {
        self.chatViewModel = chatViewModel
        self.runtimeModelId = runtimeModelId
        self.effort = effort
    }

    deinit {
        if let observer = interruptionObserver {
            NotificationCenter.default.removeObserver(observer)
        }
    }

    private var interactionMode: VoiceInteractionMode {
        SettingsStore.shared.voiceInteractionMode
    }

    /// True in push-to-talk mode — the mic button's hold gesture owns
    /// recognition starts (Settings → Voice settings → Mode).
    var isPushToTalk: Bool { interactionMode == .pushToTalk }

    // MARK: - Lifecycle

    /// Called from the view's onAppear with its audio objects. Configures
    /// the `.playAndRecord` session and enters the first turn — unless a
    /// DEBUG `-voice-state` fixture forces a static state for screenshots
    /// (no audio hardware on the simulator).
    func begin(dictation: DictationController, speaker: SpeechSpeaker) {
        self.dictation = dictation
        self.speaker = speaker
        startedAt = Date()
        isActive = true

        #if DEBUG
        if applyForcedStateIfAny() { return }
        #endif

        statusLine = nil
        startTask = Task { [weak self] in await self?.startBestRoute() }
    }

    /// Tries each engine `VoiceRouter` lists for the user's "Where voice
    /// runs" setting; the system recognizer path is the last resort.
    private func startBestRoute() async {
        let preference = SettingsStore.shared.voiceRoute
        let routes = VoiceRouter.candidates(preference: preference, pushToTalk: isPushToTalk)
        isConnecting = true
        defer { isConnecting = false; modelProgress = nil }
        var notice: String?
        for route in routes {
            guard isActive, !Task.isCancelled else { return }
            switch route {
            case .legacy:
                if let notice { statusLine = notice }
                startLegacy()
                return
            case .cloud:
                do {
                    try await startSession(route: .cloud)
                    return
                } catch {
                    guard isActive else { return }
                    if preference == .cloud {
                        statusLine = error.localizedDescription
                        state = .idle
                        return
                    }
                    notice = VoiceRouter.fallbackNotice(for: error)
                }
            case .device:
                do {
                    try await startSession(route: .device)
                    if let notice { statusLine = notice }
                    return
                } catch {
                    guard isActive else { return }
                    notice = error.localizedDescription
                }
            }
        }
    }

    private func startLegacy() {
        configurePlaybackSession()
        installInterruptionObserver()
        switch interactionMode {
        case .handsFree:
            startListening()
        case .pushToTalk:
            // The mic button's hold gesture owns recognition starts.
            state = .idle
        }
    }

    /// Starts a Voice Session on `route` (.cloud or .device). Throws when it
    /// can't (the caller falls back or surfaces the error).
    private func startSession(route: VoiceRoute) async throws {
        guard await Self.ensureMicPermission() else {
            throw VoiceClientError.server(code: "mic_denied",
                message: "Microphone access is off. Turn it on in Settings › Allternit.")
        }
        let factory: VoiceTransportFactory
        var options = VoiceSessionOptions()
        options.language = SettingsStore.shared.speechLanguage?.rawValue
            ?? Locale.current.language.languageCode?.identifier ?? "en"
        switch route {
        case .cloud:
            options.voice = SettingsStore.shared.cloudVoiceId
            options.sttModel = "light"
            factory = {
                let ticket = try await VoiceCloudClient.shared.mintTicket()
                return WebSocketVoiceTransport(url: ticket.connectURL)
            }
            engineLabel = "Cloud voice"
        case .device:
            let packs = VoicePackManager.shared
            if !packs.isReady {
                statusLine = "Downloading the voice model…"
                let progressTask = Task { @MainActor [weak self] in
                    while !Task.isCancelled {
                        if case .downloading(let p) = packs.state { self?.modelProgress = p }
                        try? await Task.sleep(nanoseconds: 250_000_000)
                    }
                }
                await packs.install()
                progressTask.cancel()
                statusLine = nil
            }
            guard let paths = packs.paths else {
                if case .failed(let message) = packs.state { throw VoiceClientError.server(code: "pack", message: message) }
                throw VoiceClientError.server(code: "pack", message: "The on-device voice model isn't installed.")
            }
            options.voice = SettingsStore.shared.voiceIdentifier
            let engine = LocalVoiceEngineFactory.make(paths: paths)
            factory = { engine }
            engineLabel = "On this device"
        case .legacy:
            return
        }
        let client = VoiceSessionClient(factory: factory)
        client.onEvent = { [weak self] event in self?.handleSession(event) }
        sessionClient = client
        do {
            try await client.start(options: options)
        } catch {
            sessionClient = nil
            engineLabel = nil
            throw error
        }
        guard isActive else { client.end(); return }
        statusLine = nil
        state = .listening
    }

    private static func ensureMicPermission() async -> Bool {
        switch AVAudioApplication.shared.recordPermission {
        case .granted: return true
        case .denied: return false
        default: return await AVAudioApplication.requestRecordPermission()
        }
    }

    /// X button: tears down audio and returns the session length in seconds
    /// for the feed's "Voice chat ended · Ns" card.
    func endSession() -> Int {
        isActive = false
        startTask?.cancel()
        sessionClient?.end()
        sessionClient = nil
        suppressNextDictationEnd = true
        dictation?.stop()
        speaker?.stop()
        if let observer = interruptionObserver {
            NotificationCenter.default.removeObserver(observer)
            interruptionObserver = nil
        }
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
        return max(1, Int(Date().timeIntervalSince(startedAt)))
    }

    // MARK: - Controls

    /// Mic tap toggles mute. Muting mid-turn discards any in-flight
    /// recognition; unmuting in hands-free resumes listening.
    func toggleMute() {
        let generator = UIImpactFeedbackGenerator(style: .light)
        generator.impactOccurred()
        isMuted.toggle()
        if inSession {
            sessionClient?.setMuted(isMuted)
            if isMuted {
                liveTranscript = ""
                sessionClient?.cancelSpeech(id: currentSpeakId)
                if state == .listening { state = .idle }
            } else if state == .idle {
                state = .listening
            }
            return
        }
        if isMuted {
            suppressNextDictationEnd = true
            liveTranscript = ""
            dictation?.stop()
            speaker?.stop()
            if state == .listening || state == .speaking {
                state = .idle
            }
        } else if isActive, state == .idle, interactionMode == .handsFree {
            startListening()
        }
    }

    /// True while a tap on the reply area interrupts (thinking/speaking).
    var canInterrupt: Bool {
        state == .thinking || state == .speaking
    }

    /// Tap-to-interrupt — neither Claude's nor OpenAI's voice mode has
    /// barge-in, so this puts us ahead: speaking stops the TTS playback,
    /// thinking aborts the stream (the partial reply stays in the feed);
    /// either way the next turn starts (hands-free re-listens, PTT idles).
    func interrupt() {
        guard isActive else { return }
        if inSession {
            guard state == .thinking || state == .speaking else { return }
            UIImpactFeedbackGenerator(style: .medium).impactOccurred()
            sessionClient?.cancelSpeech(id: currentSpeakId)
            abortTurn()
            return
        }
        switch state {
        case .speaking:
            let generator = UIImpactFeedbackGenerator(style: .medium)
            generator.impactOccurred()
            speaker?.stop()
            replyMessageId = nil
            advanceAfterSpeech()
        case .thinking:
            let generator = UIImpactFeedbackGenerator(style: .medium)
            generator.impactOccurred()
            chatViewModel.stopStreaming()
            replyMessageId = nil
            advanceAfterSpeech()
        case .idle, .listening:
            break
        }
    }

    /// Push-to-talk: the mic button's hold gesture began.
    func pressBegan() {
        guard isActive, interactionMode == .pushToTalk, !isMuted else { return }
        guard state == .idle || state == .listening else { return }
        startListening()
    }

    /// Push-to-talk: released — stop recognition; the isRecording-false
    /// change forwards the transcript as a turn.
    func pressEnded() {
        guard isActive, interactionMode == .pushToTalk else { return }
        dictation?.stop()
    }

    // MARK: - Dictation events (wired by the view's onChange)

    /// DictationController stopped recording (silence, end tap, PTT release,
    /// error). A non-empty transcript becomes the next turn.
    func dictationEnded() {
        guard isActive, !inSession else { return }
        if suppressNextDictationEnd {
            suppressNextDictationEnd = false
            return
        }
        let transcript = dictation?.transcript.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        if transcript.isEmpty {
            // Silence with nothing said: hands-free re-arms; PTT stays idle.
            if interactionMode == .handsFree, !isMuted, state == .listening {
                startListening()
            } else if state == .listening {
                state = .idle
            }
            return
        }
        sendTurn(transcript)
    }

    func dictationFailed(_ message: String) {
        guard isActive, !inSession else { return }
        statusLine = message
        state = .idle
    }

    // MARK: - Streaming reply (wired by the view's onChange)

    /// The chat feed changed — track the current reply's text and drive the
    /// thinking → speaking transition.
    func replyUpdated(_ messages: [MessageRecord]) {
        guard isActive, let replyMessageId,
              let reply = messages.first(where: { $0.id == replyMessageId }) else { return }
        guard state == .thinking || state == .speaking else { return }

        replyText = reply.content

        if inSession {
            sessionReplyUpdated(reply)
            return
        }

        if let error = reply.error {
            statusLine = error.title
            speaker?.stop()
            state = .idle
            self.replyMessageId = nil
            return
        }

        guard !reply.isStreaming, state == .thinking else { return }

        // Stream finished: read the reply sentence-chunked, then take the
        // next turn when the speaker drains (speakerFinished below).
        self.replyMessageId = nil
        if isMuted || reply.content.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            advanceAfterSpeech()
            return
        }
        state = .speaking
        // Dictation's teardown deactivated the audio session — re-apply the
        // playback configuration before queueing utterances.
        configurePlaybackSession()
        speakSentenceChunks(of: reply.content)
        // A reply whose chunks were all empty never starts the synthesizer —
        // don't park in .speaking waiting for a finish that never comes.
        if speaker?.isSpeaking != true {
            advanceAfterSpeech()
        }
    }

    /// SpeechSpeaker drained its queue (wired from `speaker.isSpeaking`).
    func speakerFinished() {
        guard isActive, !inSession, state == .speaking else { return }
        advanceAfterSpeech()
    }

    // MARK: - Voice Session events

    private func handleSession(_ event: VoiceSessionClientEvent) {
        guard isActive else { return }
        switch event {
        case .state(let connection):
            switch connection {
            case .reconnecting(let attempt):
                statusLine = "Reconnecting… (\(attempt)/\(VoiceSessionClient.maxReconnectAttempts))"
            case .ready:
                if statusLine?.hasPrefix("Reconnecting") == true { statusLine = nil }
            case .failed(let message):
                sessionClient = nil
                statusLine = message
                state = .idle
            case .idle, .connecting, .ended:
                break
            }
        case .reconnected:
            // Speech in flight was lost with the old connection.
            currentSpeakId = nil
            if state == .speaking { state = .listening }
        case .server(let server):
            handleServer(server)
        }
    }

    private func handleServer(_ event: VoiceServerEvent) {
        switch event {
        case .speechStarted:
            if state == .idle || state == .listening { state = .listening }
        case .transcriptDelta(_, let text), .transcriptFinal(_, let text):
            liveTranscript = text
        case .turnEnded(let text, _):
            let transcript = text.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !transcript.isEmpty, !isMuted else { return }
            sendTurn(transcript)
        case .speakStarted(let id):
            currentSpeakId = id
            if replyMessageId != nil || state == .thinking { state = .speaking }
        case .speakEnded(let id):
            guard id == currentSpeakId || currentSpeakId == nil else { return }
            currentSpeakId = nil
            // The reply may still be streaming more sentences; only go back
            // to listening once it is complete.
            if replyMessageId == nil, state == .speaking { state = .listening }
        case .speakInterrupted:
            // Barge-in: the engine stopped; stop the agent's turn too.
            abortTurn()
        case .error(let code, let message, let fatal):
            if code == "turn_unavailable" { return }
            if fatal { statusLine = message } else if code != "bad_message" { statusLine = message }
        case .ready, .speechStopped:
            break
        }
    }

    /// Ends the current reply (tap-to-interrupt or barge-in): aborts the
    /// agent stream, keeps the partial reply in the thread, back to listening.
    private func abortTurn() {
        chatViewModel.stopStreaming()
        replyMessageId = nil
        currentSpeakId = nil
        state = isMuted ? .idle : .listening
    }

    /// Streams the reply's new text to the engine as it arrives.
    private func sessionReplyUpdated(_ reply: MessageRecord) {
        if let error = reply.error {
            statusLine = error.title
            sessionClient?.cancelSpeech(id: currentSpeakId)
            replyMessageId = nil
            state = isMuted ? .idle : .listening
            return
        }
        let content = reply.content
        if !isMuted, content.count > spokenLength {
            let delta = String(content.dropFirst(spokenLength))
            spokenLength = content.count
            sessionClient?.speakDelta(id: utteranceId, text: delta)
        }
        guard !reply.isStreaming else { return }
        replyMessageId = nil
        if isMuted || content.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            state = isMuted ? .idle : .listening
            return
        }
        sessionClient?.speakDone(id: utteranceId)
        // speak.ended returns the state to listening; if nothing is playing
        // (engine had no speakable text) it still sends started+ended.
    }

    // MARK: - Turns

    private func startListening() {
        guard !isMuted, let dictation else { return }
        statusLine = nil
        liveTranscript = ""
        state = .listening
        Task { await dictation.start() }
    }

    private func sendTurn(_ transcript: String) {
        liveTranscript = ""
        replyText = ""
        statusLine = nil
        state = .thinking
        utteranceId = UUID().uuidString
        spokenLength = 0
        currentSpeakId = nil
        chatViewModel.sendMessage(transcript, runtimeModelId: runtimeModelId, effort: effort)
        // sendMessage appends the user bubble + streaming assistant
        // placeholder synchronously — the placeholder is the reply.
        replyMessageId = chatViewModel.messages.last(where: { $0.role == "assistant" })?.id
        if replyMessageId == nil {
            // Never happens with the current send path, but degrade instead
            // of parking in .thinking forever.
            statusLine = "Couldn't start the reply stream."
            state = .idle
        }
    }

    /// Next turn after a spoken reply: hands-free re-listens, PTT idles.
    private func advanceAfterSpeech() {
        guard isActive else { return }
        if interactionMode == .handsFree, !isMuted {
            // Back to record mode for the next turn.
            startListening()
        } else {
            state = .idle
        }
    }

    /// Splits the reply into sentences and queues them on SpeechSpeaker so
    /// prosody pauses land naturally (one utterance per sentence).
    private func speakSentenceChunks(of text: String) {
        for sentence in Self.sentences(in: text) {
            speaker?.speakChunk(sentence)
        }
    }

    /// Sentence splitter: breaks after ., !, ?, or a newline, keeping the
    /// terminator. Good enough for speech chunking — abbreviations just
    /// produce shorter chunks, not wrong words.
    nonisolated static func sentences(in text: String) -> [String] {
        var sentences: [String] = []
        var current = ""
        for character in text {
            current.append(character)
            if character == "." || character == "!" || character == "?" || character == "\n" {
                let trimmed = current.trimmingCharacters(in: .whitespacesAndNewlines)
                if !trimmed.isEmpty { sentences.append(trimmed) }
                current = ""
            }
        }
        let trimmed = current.trimmingCharacters(in: .whitespacesAndNewlines)
        if !trimmed.isEmpty { sentences.append(trimmed) }
        return sentences
    }

    // MARK: - Audio session

    /// `.playAndRecord` with duck-others so a reply reads through the
    /// speaker while the session can flip back to recording for the next
    /// turn without a category change. DictationController reconfigures to
    /// `.record` while it runs; this is re-applied before playback.
    private func configurePlaybackSession() {
        let session = AVAudioSession.sharedInstance()
        try? session.setCategory(.playAndRecord, mode: .default, options: [.duckOthers, .defaultToSpeaker])
        try? session.setActive(true, options: .notifyOthersOnDeactivation)
    }

    /// Phone-call / Siri interruptions: pause in place; resume hands-free
    /// listening when the system says we may.
    private func installInterruptionObserver() {
        interruptionObserver = NotificationCenter.default.addObserver(
            forName: AVAudioSession.interruptionNotification,
            object: nil,
            queue: .main
        ) { [weak self] notification in
            // Extract Sendable values before hopping to the MainActor — the
            // notification's userInfo dictionary isn't Sendable.
            guard let info = notification.userInfo,
                  let rawType = info[AVAudioSessionInterruptionTypeKey] as? UInt,
                  let type = AVAudioSession.InterruptionType(rawValue: rawType) else { return }
            let rawOptions = info[AVAudioSessionInterruptionOptionKey] as? UInt ?? 0
            Task { @MainActor in
                guard let self, self.isActive else { return }
                switch type {
                case .began:
                    self.suppressNextDictationEnd = true
                    self.dictation?.stop()
                    self.speaker?.stop()
                    self.statusLine = "Paused — audio interrupted"
                    self.state = .idle
                case .ended:
                    let options = AVAudioSession.InterruptionOptions(rawValue: rawOptions)
                    if options.contains(.shouldResume), self.interactionMode == .handsFree, !self.isMuted {
                        self.startListening()
                    }
                @unknown default:
                    break
                }
            }
        }
    }

    // MARK: - DEBUG fixtures

    #if DEBUG
    /// `-voice-state listening|thinking|speaking` (DEBUG only): pins the
    /// takeover in one gradient state with fixture text for screenshot
    /// verification — the simulator has no real microphone. Returns true
    /// when a fixture was applied (real audio is skipped entirely).
    private func applyForcedStateIfAny() -> Bool {
        guard let raw = UserDefaults.standard.string(forKey: "voice-state") else { return false }
        switch raw {
        case "idle":
            state = .idle
        case "listening":
            state = .listening
            liveTranscript = "So the deploy should run before the…"
        case "thinking":
            state = .thinking
        case "speaking":
            state = .speaking
            replyText = "Good thought — running the deploy first keeps the migration reversible. I'd stage it behind the feature flag, watch error rates for ten minutes, then roll it out to everyone."
        default:
            return false
        }
        return true
    }
    #endif
}
