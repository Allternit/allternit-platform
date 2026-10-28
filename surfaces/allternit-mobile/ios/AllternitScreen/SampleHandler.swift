// Ported from iCTRL (https://github.com/Allternit/allternit-ictrl, iCTRLBroadcast/SampleHandler.swift).
// Copyright iCTRL authors. Licensed under the Apache License, Version 2.0;
// see http://www.apache.org/licenses/LICENSE-2.0. Modified for Allternit:
// the loopback IngestLink is replaced by an outbound WebSocket to the user's
// Desktop through the cloud relay, every packet is sealed, and controls
// arrive as JSON text frames instead of single bytes.

import CoreMedia
import Foundation
import ReplayKit
import UIKit

/// The live screen source: a ReplayKit broadcast upload extension that
/// encodes the screen to H.264 and the app sound to AAC-LC, and sends both to
/// the paired Desktop (`WS /api/v1/phones/app/ingest`, reached through
/// api.allternit.com). Only a person can start it, from the system picker.
///
/// Memory is the constraint that shapes this file. iOS kills a broadcast
/// extension past ~50 MB, so nothing is queued: a frame that arrives while the
/// previous ones are still being sent is skipped before it is encoded (skipping
/// an encoded P-frame instead would corrupt everything after it).
final class SampleHandler: RPBroadcastSampleHandler {
    private let queue = DispatchQueue(label: "com.allternit.mobile.screen.broadcast")
    private lazy var link = IngestSocket(queue: queue)
    private let encoder = H264Encoder()
    private let audioEncoder = AACEncoder()

    private struct Config: Equatable {
        var width: Int
        var height: Int
        var rotation: Int
    }

    // All state below is touched only on `queue`.
    /// Starts paused: nothing is encoded until the node says someone is watching.
    private var paused = true
    private var forceKeyframe = true
    private var config: Config?
    private var configSentOnLink = false
    /// The last screen ReplayKit delivered (one buffer, never a queue), kept so
    /// a viewer joining a still screen gets a picture now: ReplayKit sends
    /// nothing while nothing moves, so waiting for the next real frame could
    /// mean waiting forever.
    private var lastFrame: CVPixelBuffer?
    private var lastPTS = CMTime.invalid
    /// AUDIO_CONFIG already sent for the current audio format on this link.
    private var audioConfigSentOnLink = false
    private var heartbeat: DispatchSourceTimer?

    /// Frames allowed between "submitted to the encoder" and "written to the
    /// socket". Two lets encoding of one overlap the send of the previous.
    private static let maxInFlight = 2

    /// The live handler, for the Darwin-notification callback (a C function
    /// pointer can't capture). One broadcast runs per extension process.
    private static weak var current: SampleHandler?

    override func broadcastStarted(withSetupInfo setupInfo: [String: NSObject]?) {
        guard let credentials = AppPhoneKeychain.load(), let key = credentials.sessionKey else {
            finish("Pair this iPhone first: open Allternit, then Settings → Phones.")
            return
        }
        Self.current = self
        observeStop()
        encoder.onFrame = { [weak self] au, _ in
            self?.queue.async { self?.link.sendFrame(LiveScreenPacket.encode(.frame, au)) }
        }
        encoder.onDrop = { [weak self] in
            self?.queue.async { self?.link.release() }
        }
        queue.async {
            self.link.onReady = { [weak self] in self?.linkReady() }
            self.link.onClosed = { [weak self] in self?.setPaused(true) }
            self.link.onControl = { [weak self] type in self?.control(type) }
            self.link.onFatal = { [weak self] message in self?.finish(message) }
            self.link.start(credentials: credentials, key: key)
            self.startHeartbeat()
        }
    }

    override func broadcastFinished() {
        queue.sync {
            heartbeat?.cancel()
            heartbeat = nil
            encoder.invalidate()
            audioEncoder.invalidate()
            lastFrame = nil
            link.finish(with: LiveScreenPacket.json(.ended, ["reason": "stopped"]))
        }
        stopObservingStop()
        AppPhoneSharedState.markEnded()
        AppPhoneSharedState.post(AppPhoneSharedState.changedNotification)
    }

    override func processSampleBuffer(_ sampleBuffer: CMSampleBuffer, with sampleBufferType: RPSampleBufferType) {
        // App sound plays in the viewer; the microphone (`.audioMic`) is never
        // sent — the picker hides the mic button and it is ignored here too.
        if sampleBufferType == .audioApp {
            queue.sync { handleAudio(sampleBuffer) }
            return
        }
        guard sampleBufferType == .video, let pixels = CMSampleBufferGetImageBuffer(sampleBuffer) else { return }
        let rotation = Self.rotation(of: sampleBuffer)
        let pts = CMSampleBufferGetPresentationTimeStamp(sampleBuffer)
        // Synchronous: ReplayKit owns the buffer only for this call, and
        // returning before it is consumed is what lets frames pile up.
        queue.sync { handle(pixels, rotation: rotation, pts: pts) }
    }

    /// Ends the broadcast with a message the system shows the person. Off
    /// `queue` on purpose: ReplayKit may call `broadcastFinished` from inside,
    /// which syncs onto `queue`.
    private func finish(_ message: String) {
        let error = NSError(domain: "com.allternit.live-screen", code: 1,
                            userInfo: [NSLocalizedDescriptionKey: message])
        AppPhoneSharedState.markEnded()
        AppPhoneSharedState.post(AppPhoneSharedState.changedNotification)
        DispatchQueue.main.async { self.finishBroadcastWithError(error) }
    }

    // MARK: - Frames

    private func handle(_ pixels: CVPixelBuffer, rotation: Int, pts: CMTime) {
        let size = H264Encoder.targetSize(width: CVPixelBufferGetWidth(pixels),
                                          height: CVPixelBufferGetHeight(pixels))
        let next = Config(width: size.width, height: size.height, rotation: rotation)
        if next != config {
            if next.width != config?.width || next.height != config?.height {
                encoder.invalidate()
                forceKeyframe = true
            }
            config = next
            configSentOnLink = false
        }
        sendConfigIfNeeded()

        lastFrame = pixels
        guard link.isReady, !paused else { return }
        guard link.inFlight < Self.maxInFlight else { return }   // skip, don't queue
        encode(pixels, pts: pts)
    }

    private func encode(_ source: CVPixelBuffer, pts: CMTime) {
        guard let c = config, encoder.prepare(width: c.width, height: c.height),
              let frame = encoder.scaled(source) else { return }
        // VideoToolbox requires rising timestamps; a repeated still frame is
        // stamped "now", which can tie with the last real one.
        var stamp = pts.isValid ? pts : CMClockGetTime(CMClockGetHostTimeClock())
        if lastPTS.isValid, CMTimeCompare(stamp, lastPTS) <= 0 {
            stamp = CMTimeAdd(lastPTS, CMTime(value: 1, timescale: 1000))
        }
        link.reserve()
        if encoder.encode(frame, pts: stamp, forceKeyframe: forceKeyframe) {
            lastPTS = stamp
            forceKeyframe = false
        } else {
            link.release()
        }
    }

    /// Re-encodes the last frame as an IDR so a viewer that just joined sees
    /// the screen without waiting for it to change.
    private func repeatLastFrameAsKeyframe() {
        forceKeyframe = true
        guard link.isReady, !paused, let source = lastFrame,
              link.inFlight < Self.maxInFlight else { return }
        encode(source, pts: CMClockGetTime(CMClockGetHostTimeClock()))
    }

    private func sendConfigIfNeeded() {
        guard link.isReady, !configSentOnLink, let c = config else { return }
        link.send(LiveScreenPacket.json(.config, [
            "width": c.width,
            "height": c.height,
            "scale": Self.pixelsPerPoint(encodedWidth: c.width, encodedHeight: c.height),
            "rotation": c.rotation,
        ]))
        configSentOnLink = true
    }

    // MARK: - Audio

    private func handleAudio(_ sample: CMSampleBuffer) {
        // Only while someone is watching: an idle broadcast encodes nothing.
        guard link.isReady, !paused else { return }
        let (packets, formatChanged) = audioEncoder.encode(sample)
        if formatChanged { audioConfigSentOnLink = false }
        sendAudioConfigIfNeeded()
        guard audioConfigSentOnLink else { return }
        for au in packets { link.sendAudio(LiveScreenPacket.encode(.audio, au)) }
    }

    private func sendAudioConfigIfNeeded() {
        guard link.isReady, !audioConfigSentOnLink, let f = audioEncoder.format else { return }
        link.send(LiveScreenPacket.json(.audioConfig, [
            "codec": "aac",
            "sampleRate": f.sampleRate,
            "channels": f.channels,
            "asc": f.asc.base64EncodedString(),
        ]))
        audioConfigSentOnLink = true
    }

    /// A viewer joined or came back: it needs the audio format again before
    /// any AAC (the node may cache it, but a restarted node has nothing).
    private func resendAudioConfig() {
        audioConfigSentOnLink = false
        sendAudioConfigIfNeeded()
    }

    // MARK: - Link

    private func linkReady() {
        // A fresh node socket knows nothing: say we're live, then the size as
        // soon as there is one. Paused until it asks.
        setPaused(true)
        configSentOnLink = false
        audioConfigSentOnLink = false
        audioEncoder.reset()
        link.send(LiveScreenPacket.json(.status, ["broadcasting": true]))
        sendConfigIfNeeded()
    }

    private func control(_ type: String) {
        switch type {
        case "resume":
            setPaused(false)
            resendAudioConfig()
            repeatLastFrameAsKeyframe()
        case "pause":
            setPaused(true)
            // Sound buffered before a pause must not play after the resume.
            audioEncoder.reset()
        case "keyframe":
            resendAudioConfig()
            repeatLastFrameAsKeyframe()
        case "stop":
            finish("Stopped from Allternit")
        default:
            break
        }
    }

    private func setPaused(_ value: Bool) {
        guard paused != value else { return }
        paused = value
        AppPhoneSharedState.markLive(watched: !value)
        AppPhoneSharedState.post(AppPhoneSharedState.changedNotification)
    }

    /// Keeps the app's "Your bot can see this screen" banner honest: a
    /// heartbeat that stops (the extension was killed) clears it.
    private func startHeartbeat() {
        AppPhoneSharedState.markLive(watched: !paused)
        AppPhoneSharedState.post(AppPhoneSharedState.changedNotification)
        let timer = DispatchSource.makeTimerSource(queue: queue)
        timer.schedule(deadline: .now() + AppPhoneSharedState.heartbeatInterval,
                       repeating: AppPhoneSharedState.heartbeatInterval)
        timer.setEventHandler { [weak self] in
            guard let self else { return }
            AppPhoneSharedState.markLive(watched: !self.paused)
        }
        timer.resume()
        heartbeat = timer
    }

    // MARK: - Stop from the app

    private func observeStop() {
        CFNotificationCenterAddObserver(
            CFNotificationCenterGetDarwinNotifyCenter(), nil,
            { _, _, _, _, _ in SampleHandler.current?.finish("Stopped from Allternit") },
            AppPhoneSharedState.stopNotification as CFString, nil, .deliverImmediately)
    }

    private func stopObservingStop() {
        CFNotificationCenterRemoveEveryObserver(CFNotificationCenterGetDarwinNotifyCenter(), nil)
    }

    // MARK: - Geometry

    /// 0/90/180/270 as ReplayKit reports the sample's orientation, so the
    /// viewer can turn a landscape app upright. ReplayKit keeps encoding the
    /// panel in its native (portrait) layout and only labels the rotation.
    private static func rotation(of sample: CMSampleBuffer) -> Int {
        guard let raw = CMGetAttachment(sample, key: RPVideoSampleOrientationKey as CFString,
                                        attachmentModeOut: nil) as? NSNumber,
              let orientation = CGImagePropertyOrientation(rawValue: raw.uint32Value) else { return 0 }
        switch orientation {
        case .left, .leftMirrored: return 90
        case .down, .downMirrored: return 180
        case .right, .rightMirrored: return 270
        default: return 0
        }
    }

    /// Encoded pixels per UIKit point — what turns a touch on the video into
    /// a tap coordinate. From the encoded size, not UIScreen.scale: after the
    /// 1920 downscale a 3x phone is no longer 3 pixels per point.
    private static func pixelsPerPoint(encodedWidth: Int, encodedHeight: Int) -> Double {
        let bounds = UIScreen.main.bounds
        let points = max(bounds.width, bounds.height)
        guard points > 0 else { return Double(UIScreen.main.scale) }
        let s = Double(max(encodedWidth, encodedHeight)) / Double(points)
        return (s * 1000).rounded() / 1000
    }
}
