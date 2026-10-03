import AVFoundation
import Foundation

/// Mic capture (16 kHz mono PCM16) and speech playback for a voice session,
/// on one AVAudioEngine.
///
/// The audio session is `.playAndRecord` / `.voiceChat` with voice
/// processing enabled on the input node, so the OS echo-cancels the speaker
/// out of the mic — that is what lets the user barge in over the bot without
/// headphones. Playback is an AVAudioPlayerNode fed by `PlaybackJitterBuffer`;
/// `flushPlayback()` stops the node at once (instant cut on
/// `speak.interrupted`).
final class VoiceAudioIO: @unchecked Sendable {
    /// Called on the audio thread with each captured frame (PCM16 LE, mono,
    /// `VoiceAudioIO.captureSampleRate`).
    var onMicFrame: (@Sendable (Data) -> Void)?

    static let captureSampleRate: Double = 16_000

    private let engine = AVAudioEngine()
    private let player = AVAudioPlayerNode()
    private let lock = NSLock()
    private var jitter = PlaybackJitterBuffer(sampleRate: 24_000)
    private var playbackFormat: AVAudioFormat?
    private var converter: AVAudioConverter?
    private var captureFormat: AVAudioFormat?
    private var running = false
    private var micMuted = false

    /// Starts the audio session and engine. Throws if the session can't be
    /// activated (e.g. another app holds a non-mixable call).
    func start(outputSampleRate: Int) throws {
        lock.lock(); defer { lock.unlock() }
        guard !running else { return }

        let session = AVAudioSession.sharedInstance()
        try session.setCategory(.playAndRecord, mode: .voiceChat, options: [.defaultToSpeaker, .allowBluetooth])
        try session.setActive(true, options: [])

        let input = engine.inputNode
        // Echo cancellation + noise suppression (barge-in on the speaker).
        try? input.setVoiceProcessingEnabled(true)

        jitter = PlaybackJitterBuffer(sampleRate: outputSampleRate)
        guard let playback = AVAudioFormat(commonFormat: .pcmFormatFloat32,
                                           sampleRate: Double(outputSampleRate),
                                           channels: 1, interleaved: false),
              let capture = AVAudioFormat(commonFormat: .pcmFormatFloat32,
                                          sampleRate: Self.captureSampleRate,
                                          channels: 1, interleaved: false) else {
            throw VoiceAudioError.formatUnavailable
        }
        playbackFormat = playback
        captureFormat = capture

        engine.attach(player)
        engine.connect(player, to: engine.mainMixerNode, format: playback)

        let hardwareFormat = input.outputFormat(forBus: 0)
        guard hardwareFormat.sampleRate > 0, hardwareFormat.channelCount > 0 else {
            throw VoiceAudioError.noInput
        }
        converter = AVAudioConverter(from: hardwareFormat, to: capture)
        input.installTap(onBus: 0, bufferSize: 1_024, format: hardwareFormat) { [weak self] buffer, _ in
            self?.handleCapture(buffer)
        }

        engine.prepare()
        try engine.start()
        player.play()
        running = true
    }

    func stop() {
        lock.lock(); defer { lock.unlock() }
        guard running else { return }
        running = false
        engine.inputNode.removeTap(onBus: 0)
        player.stop()
        engine.stop()
        engine.detach(player)
        try? engine.inputNode.setVoiceProcessingEnabled(false)
        jitter.flush()
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    }

    /// While muted, captured frames are dropped client-side as well (the
    /// server is told with `mic.mute`; this keeps audio off the wire even if
    /// the frame is still in flight).
    func setMicMuted(_ muted: Bool) {
        lock.lock(); micMuted = muted; lock.unlock()
    }

    /// Queues speech audio (PCM16 LE mono at the session's output rate).
    func play(pcm16 data: Data) {
        let samples = VoicePCM.decode(data)
        lock.lock()
        let frames = running ? jitter.push(samples) : []
        lock.unlock()
        schedule(frames)
    }

    /// End of an utterance: play whatever is still under the prebuffer.
    func finishUtterance() {
        lock.lock()
        let frames = running ? jitter.finish() : []
        lock.unlock()
        schedule(frames)
    }

    /// Cuts playback immediately and drops queued audio.
    func flushPlayback() {
        lock.lock()
        jitter.flush()
        let wasRunning = running
        lock.unlock()
        guard wasRunning else { return }
        player.stop()
        player.play()
    }

    private func schedule(_ frames: [[Float]]) {
        guard let format = playbackFormat else { return }
        for samples in frames {
            guard let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(samples.count)),
                  let channel = buffer.floatChannelData?[0] else { continue }
            samples.withUnsafeBufferPointer { channel.update(from: $0.baseAddress!, count: samples.count) }
            buffer.frameLength = AVAudioFrameCount(samples.count)
            player.scheduleBuffer(buffer, completionHandler: nil)
        }
    }

    private func handleCapture(_ buffer: AVAudioPCMBuffer) {
        lock.lock()
        let muted = micMuted
        let converter = self.converter
        let format = captureFormat
        lock.unlock()
        guard !muted, let converter, let format else { return }

        let ratio = format.sampleRate / buffer.format.sampleRate
        let capacity = AVAudioFrameCount(Double(buffer.frameLength) * ratio) + 16
        guard let out = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: capacity) else { return }
        nonisolated(unsafe) var supplied = false
        var error: NSError?
        converter.convert(to: out, error: &error) { _, status in
            if supplied {
                status.pointee = .noDataNow
                return nil
            }
            supplied = true
            status.pointee = .haveData
            return buffer
        }
        guard error == nil, out.frameLength > 0, let channel = out.floatChannelData?[0] else { return }
        let samples = Array(UnsafeBufferPointer(start: channel, count: Int(out.frameLength)))
        onMicFrame?(VoicePCM.encode(samples))
    }
}

enum VoiceAudioError: Error, LocalizedError {
    case formatUnavailable
    case noInput

    var errorDescription: String? {
        switch self {
        case .formatUnavailable: return "Couldn't set up the audio format."
        case .noInput: return "No microphone input is available."
        }
    }
}
