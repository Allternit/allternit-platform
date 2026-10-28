import AVFoundation
import AudioToolbox
import Foundation
#if canImport(LiveKit)
import LiveKit
#endif

// MARK: - Seam

/// A joined call's audio. The web's `CallAudioSession`
/// (allternit-ai `src/lib/gateway/call-webrtc.ts`): every remote audio track
/// plays, the person's mic is published, `leave()` disconnects.
@MainActor
protocol CallAudioSession: AnyObject {
    func setMuted(_ muted: Bool)
    func setSpeaker(_ on: Bool)
    func leave()
}

/// Joins a call's LiveKit room with the grant cloud-api hands back
/// (`{token, url, room}`). The web uses `livekit-client`; the iPhone app uses
/// the LiveKit Swift SDK (`client-sdk-swift`, Apache-2.0). Until that
/// package is in project.yml, `isAvailable` is false and the call screens
/// say audio can't connect in this version instead of pretending.
@MainActor
enum CallAudio {
    enum JoinResult {
        case joined(CallAudioSession)
        case failed
    }

    static var isAvailable: Bool {
        #if canImport(LiveKit)
        true
        #else
        false
        #endif
    }

    /// `onRemoteEnd` fires when the other side leaves or the room drops.
    static func join(url: String, token: String, onRemoteEnd: @escaping @MainActor @Sendable () -> Void) async -> JoinResult {
        guard !url.isEmpty, !token.isEmpty else { return .failed }
        #if canImport(LiveKit)
        guard CallAudioSessionConfig.activateForCall() else { return .failed }
        if let session = await LiveKitCallAudio.connect(url: url, token: token, onRemoteEnd: onRemoteEnd) {
            return .joined(session)
        }
        CallAudioSessionConfig.deactivate()
        return .failed
        #else
        return .failed
        #endif
    }
}

// MARK: - AVAudioSession

@MainActor
enum CallAudioSessionConfig {
    /// A two-way voice call: playAndRecord + voiceChat (echo cancellation,
    /// the receiver by default), Bluetooth headsets allowed.
    static func activateForCall() -> Bool {
        let session = AVAudioSession.sharedInstance()
        do {
            try session.setCategory(.playAndRecord, mode: .voiceChat, options: [.allowBluetooth, .allowBluetoothA2DP])
            try session.setActive(true)
            return true
        } catch {
            return false
        }
    }

    static func setSpeaker(_ on: Bool) {
        try? AVAudioSession.sharedInstance().overrideOutputAudioPort(on ? .speaker : .none)
    }

    static func deactivate() {
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    }

    /// Mic permission for answering. Asks once; nil never happens.
    static func microphoneAllowed() async -> Bool {
        switch AVAudioApplication.shared.recordPermission {
        case .granted: return true
        case .denied: return false
        default: return await AVAudioApplication.requestRecordPermission()
        }
    }
}

// MARK: - Ring

/// The ring while the app is open: the web's soft two-tone burst
/// (440 + 480 Hz, 0.9 s every 3 s, `ring-tone.ts`), synthesised in memory,
/// plus a vibration each burst. Uses the ambient category so the silent
/// switch silences the tone (the vibration still comes). `sound: false`
/// (Settings → Notifications → Ring sound off) vibrates only.
@MainActor
final class CallRingtone {
    private var player: AVAudioPlayer?
    private var vibrate: Timer?

    func start(sound: Bool = true) {
        guard player == nil, vibrate == nil else { return }
        if sound {
            try? AVAudioSession.sharedInstance().setCategory(.ambient, options: [.mixWithOthers])
            try? AVAudioSession.sharedInstance().setActive(true)
            if let data = Self.toneWAV(), let player = try? AVAudioPlayer(data: data) {
                player.numberOfLoops = -1
                player.volume = 0.6
                player.play()
                self.player = player
            }
        }
        AudioServicesPlaySystemSound(kSystemSoundID_Vibrate)
        vibrate = Timer.scheduledTimer(withTimeInterval: 3, repeats: true) { _ in
            AudioServicesPlaySystemSound(kSystemSoundID_Vibrate)
        }
    }

    func stop() {
        vibrate?.invalidate()
        vibrate = nil
        if player != nil {
            player?.stop()
            player = nil
            try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
        }
    }

    /// 3 s of 16-bit mono PCM: 0.9 s burst with soft edges, then silence.
    private static func toneWAV() -> Data? {
        let rate = 22_050
        let total = rate * 3
        let burst = Int(Double(rate) * 0.9)
        let fade = Int(Double(rate) * 0.05)
        var pcm = Data(capacity: total * 2)
        for i in 0..<total {
            var sample = 0.0
            if i < burst {
                let t = Double(i) / Double(rate)
                let edge = min(1, Double(min(i, burst - i)) / Double(fade))
                sample = (sin(2 * .pi * 440 * t) + sin(2 * .pi * 480 * t)) * 0.25 * edge
            }
            var value = Int16(max(-1, min(1, sample)) * Double(Int16.max)).littleEndian
            withUnsafeBytes(of: &value) { pcm.append(contentsOf: $0) }
        }
        var header = Data()
        func u32(_ v: UInt32) { var x = v.littleEndian; withUnsafeBytes(of: &x) { header.append(contentsOf: $0) } }
        func u16(_ v: UInt16) { var x = v.littleEndian; withUnsafeBytes(of: &x) { header.append(contentsOf: $0) } }
        header.append(contentsOf: Array("RIFF".utf8)); u32(UInt32(36 + pcm.count))
        header.append(contentsOf: Array("WAVE".utf8))
        header.append(contentsOf: Array("fmt ".utf8)); u32(16); u16(1); u16(1)
        u32(UInt32(rate)); u32(UInt32(rate * 2)); u16(2); u16(16)
        header.append(contentsOf: Array("data".utf8)); u32(UInt32(pcm.count))
        return header + pcm
    }
}

// MARK: - LiveKit

#if canImport(LiveKit)
/// LiveKit Swift SDK 2.x. Remote audio tracks play on their own once
/// subscribed (no attach step like the web's `<audio>` elements).
@MainActor
final class LiveKitCallAudio: CallAudioSession {
    private let room: Room
    private let watcher: RoomWatcher
    private var left = false

    private init(room: Room, watcher: RoomWatcher) {
        self.room = room
        self.watcher = watcher
    }

    static func connect(url: String, token: String, onRemoteEnd: @escaping @MainActor @Sendable () -> Void) async -> LiveKitCallAudio? {
        // The receiver is the default output; Speaker is a toggle.
        AudioManager.shared.isSpeakerOutputPreferred = false
        let room = Room()
        let watcher = RoomWatcher(onEnd: { Task { @MainActor in onRemoteEnd() } })
        room.add(delegate: watcher)
        do {
            try await room.connect(url: url, token: token)
            try await room.localParticipant.setMicrophone(enabled: true)
        } catch {
            await room.disconnect()
            return nil
        }
        return LiveKitCallAudio(room: room, watcher: watcher)
    }

    func setMuted(_ muted: Bool) {
        guard !left else { return }
        let room = self.room
        Task { _ = try? await room.localParticipant.setMicrophone(enabled: !muted) }
    }

    func setSpeaker(_ on: Bool) {
        AudioManager.shared.isSpeakerOutputPreferred = on
        CallAudioSessionConfig.setSpeaker(on)
    }

    func leave() {
        guard !left else { return }
        left = true
        watcher.disarm()
        let room = self.room
        Task {
            await room.disconnect()
            await MainActor.run { CallAudioSessionConfig.deactivate() }
        }
    }
}

/// Tells the call the other person hung up (they left the room, or the
/// room dropped).
private final class RoomWatcher: NSObject, RoomDelegate, @unchecked Sendable {
    private let lock = NSLock()
    private var onEnd: (@Sendable () -> Void)?

    init(onEnd: @escaping @Sendable () -> Void) {
        self.onEnd = onEnd
    }

    func disarm() {
        lock.lock(); onEnd = nil; lock.unlock()
    }

    private func fire() {
        lock.lock(); let cb = onEnd; onEnd = nil; lock.unlock()
        cb?()
    }

    func room(_ room: Room, participantDidDisconnect participant: RemoteParticipant) {
        if room.remoteParticipants.isEmpty { fire() }
    }

    func room(_ room: Room, didDisconnectWithError error: LiveKitError?) {
        fire()
    }
}
#endif
