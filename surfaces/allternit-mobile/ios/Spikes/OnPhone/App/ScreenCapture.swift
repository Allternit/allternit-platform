import CoreImage
import Foundation
import Vision
#if canImport(ScreenCaptureKit)
import ScreenCaptureKit

/// Whole-screen capture with ScreenCaptureKit (public, new on iOS 27). The person
/// picks "Display" in the system content picker. On iOS 27 the SDK marks the
/// ReplayKit broadcast extension "No longer supported", so this replaces it.
///
/// For each kept frame (at most 2 per second) it times the JPEG encode (the
/// on-phone "screenshot"). While a read window is open it runs Vision text
/// recognition on the frame (the on-phone "screen read"). Whether frames keep
/// coming while another app is in front is one of the things S0 measures:
/// `bgFrames` counts frames that arrived while this app was not active.
final class ScreenCapture: NSObject, SCContentSharingPickerObserver, SCStreamOutput, SCStreamDelegate {
    static let shared = ScreenCapture()

    var onLog: ((String) -> Void)?
    private(set) var stream: SCStream?
    private let ci = CIContext()
    private let work = DispatchQueue(label: "onphone.capture", qos: .userInitiated)
    private var lastKept: Double = 0
    private var busy = false
    var appActive = true
    private(set) var frames = 0
    private(set) var bgFrames = 0

    func present() {
        let picker = SCContentSharingPicker.shared
        picker.add(self)
        picker.isActive = true
        picker.present(using: .display)
    }

    func stop() {
        stream?.stopCapture { _ in }
        stream = nil
        Spike.appendLine(Spike.tunnelLog, ["event": "capture_stop", "frames": frames, "bgFrames": bgFrames])
    }

    // MARK: picker observer

    func contentSharingPicker(_ picker: SCContentSharingPicker, didCancelFor stream: SCStream?) {
        onLog?("capture picker cancelled")
    }

    func contentSharingPickerStartDidFailWithError(_ error: Error) {
        onLog?("capture picker failed: \(error)")
        Spike.appendLine(Spike.tunnelLog, ["event": "capture_picker_failed", "error": "\(error)"])
    }

    func contentSharingPicker(_ picker: SCContentSharingPicker, didUpdateWith filter: SCContentFilter, for stream: SCStream?) {
        // iOS has no updateContentFilter; a new pick replaces the running stream.
        if let old = stream ?? self.stream { old.stopCapture { _ in } }
        let config = SCStreamConfiguration()
        let s = SCStream(filter: filter, configuration: config, delegate: self)
        do {
            try s.addStreamOutput(self, type: .screen, sampleHandlerQueue: work)
        } catch {
            onLog?("capture addStreamOutput: \(error)")
            return
        }
        s.startCapture { [weak self] error in
            if let error {
                self?.onLog?("capture start failed: \(error)")
                Spike.appendLine(Spike.tunnelLog, ["event": "capture_start_failed", "error": "\(error)"])
            } else {
                self?.onLog?("capture started")
                Spike.appendLine(Spike.tunnelLog, ["event": "capture_start"])
            }
        }
        self.stream = s
    }

    // MARK: stream

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        Spike.appendLine(Spike.tunnelLog, ["event": "capture_stopped", "error": "\(error)", "frames": frames, "bgFrames": bgFrames])
        onLog?("capture stopped: \(error)")
        self.stream = nil
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen, !busy, let pixels = CMSampleBufferGetImageBuffer(sampleBuffer) else { return }
        let now = Spike.now
        guard now - lastKept >= 0.5 else { return }
        lastKept = now
        busy = true
        defer { busy = false }
        frames += 1
        if !appActive { bgFrames += 1 }
        let image = CIImage(cvPixelBuffer: pixels)
        shot(image, active: appActive)
        if let m = Spike.readJSON(Spike.measureFile), let until = m["until"] as? Double, until > Spike.now,
           let app = m["app"] as? String {
            read(image, app: app, active: appActive)
        }
    }

    private func shot(_ image: CIImage, active: Bool) {
        let t0 = CFAbsoluteTimeGetCurrent()
        guard let cs = CGColorSpace(name: CGColorSpace.sRGB),
              let jpeg = ci.jpegRepresentation(of: image, colorSpace: cs, options: [:]) else { return }
        let ms = (CFAbsoluteTimeGetCurrent() - t0) * 1000
        try? jpeg.write(to: Spike.url("latest.jpg"), options: .atomic)
        Spike.appendLine(Spike.shotsLog, ["ms": ms, "bytes": jpeg.count, "appActive": active])
    }

    private func read(_ image: CIImage, app: String, active: Bool) {
        let t0 = CFAbsoluteTimeGetCurrent()
        guard let cg = ci.createCGImage(image, from: image.extent) else { return }
        let req = VNRecognizeTextRequest()
        req.recognitionLevel = .accurate
        req.usesLanguageCorrection = false
        do {
            try VNImageRequestHandler(cgImage: cg, options: [:]).perform([req])
        } catch {
            Spike.appendLine(Spike.readsLog, ["app": app, "error": error.localizedDescription])
            return
        }
        // Read time = frame to text, including the image conversion.
        let ms = (CFAbsoluteTimeGetCurrent() - t0) * 1000
        let lines = (req.results ?? []).compactMap { $0.topCandidates(1).first?.string }
        Spike.appendLine(Spike.readsLog, [
            "app": app, "ms": ms, "lines": lines.count, "appActive": active,
            "chars": lines.reduce(0) { $0 + $1.count }, "sample": Array(lines.prefix(6)),
        ])
    }
}
#else
/// The iOS Simulator SDK has no ScreenCaptureKit; the spike measures capture on a device only.
final class ScreenCapture {
    static let shared = ScreenCapture()
    var onLog: ((String) -> Void)?
    var appActive = true
    private(set) var bgFrames = 0
    func present() { onLog?("screen capture: not available in the Simulator") }
    func stop() {}
}
#endif
