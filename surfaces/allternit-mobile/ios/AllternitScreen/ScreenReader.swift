import CoreVideo
import Foundation
import ImageIO
import os
import Vision

/// On-demand screen read for `device.screen.read` (contract §9.1, plan
/// step 6): while the person's broadcast runs, one request reads one frame
/// with Vision text recognition and answers in the `phone.read_ui` element
/// shape (`{class:"Text", label, id:"", bounds:[x1,y1,x2,y2]}`, bounds in
/// pixels of the frame the Desktop sees). Nothing is read continuously.
///
/// iOS kills a broadcast extension near 50 MB. Before reading, the free
/// memory is checked; under `minimumHeadroom` the extension does not read
/// and sends a fresh keyframe instead, so the receiver reads that frame
/// itself. The headroom figure is an estimate, not measured on a phone yet.
final class ScreenReader {
    /// Memory left before Vision may run. `.fast` text recognition on one
    /// ~1–3 MP frame; UNVERIFIED on a real iPhone.
    static let minimumHeadroom = 24 << 20

    struct Filter {
        var label: String?
        var type: String?

        init(_ raw: Any?) {
            let obj = raw as? [String: Any]
            label = (obj?["label"] as? String).flatMap { $0.isEmpty ? nil : $0 }
            type = (obj?["type"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        }

        func keeps(_ text: String) -> Bool {
            if let type, type.caseInsensitiveCompare("Text") != .orderedSame { return false }
            if let label { return text.range(of: label, options: .caseInsensitive) != nil }
            return true
        }
    }

    private let work = DispatchQueue(label: "com.allternit.mobile.screen.read", qos: .userInitiated)
    /// Touched only on the caller's queue.
    private(set) var isReading = false

    /// Bytes left before iOS kills this process.
    static var availableMemory: Int { Int(os_proc_available_memory()) }

    static var hasHeadroom: Bool { availableMemory >= minimumHeadroom }

    /// Reads `pixels` off the caller's queue and calls `completion` with the
    /// contract envelope on `replyQueue`. One read at a time.
    /// - Parameters:
    ///   - rotation: 0/90/180/270, as SampleHandler reports it.
    ///   - encoded: the encoded (sent) frame size before rotation, which is
    ///     what the receiver's screenshot pixels are.
    func read(_ pixels: CVPixelBuffer, rotation: Int, encoded: (width: Int, height: Int), filter: Filter,
              replyQueue: DispatchQueue, completion: @escaping ([String: Any]) -> Void) {
        isReading = true
        work.async {
            let envelope = autoreleasepool { Self.recognize(pixels, rotation: rotation, encoded: encoded, filter: filter) }
            replyQueue.async {
                self.isReading = false
                completion(envelope)
            }
        }
    }

    private static func recognize(_ pixels: CVPixelBuffer, rotation: Int, encoded: (width: Int, height: Int),
                                  filter: Filter) -> [String: Any] {
        let request = VNRecognizeTextRequest()
        request.recognitionLevel = .fast
        request.usesLanguageCorrection = false
        let handler = VNImageRequestHandler(cvPixelBuffer: pixels, orientation: orientation(rotation), options: [:])
        do {
            try handler.perform([request])
        } catch {
            return PhoneUseEnvelope.failure("read_failed", "Text recognition failed on the iPhone: \(error.localizedDescription)")
        }
        // Vision's boxes are normalized to the upright image, origin bottom-left.
        let upright = rotation == 90 || rotation == 270
        let width = upright ? encoded.height : encoded.width
        let height = upright ? encoded.width : encoded.height
        var elements: [[String: Any]] = []
        var lines: [String] = []
        for observation in request.results ?? [] {
            guard let text = observation.topCandidates(1).first?.string, !text.isEmpty else { continue }
            let box = observation.boundingBox
            let bounds = [
                Int((box.minX * CGFloat(width)).rounded()),
                Int(((1 - box.maxY) * CGFloat(height)).rounded()),
                Int((box.maxX * CGFloat(width)).rounded()),
                Int(((1 - box.minY) * CGFloat(height)).rounded()),
            ]
            lines.append(text)
            guard filter.keeps(text) else { continue }
            elements.append(["class": "Text", "label": text, "id": "", "bounds": bounds])
        }
        return PhoneUseEnvelope.ok([
            "elements": elements,
            "text": lines.joined(separator: "\n"),
            "width": width,
            "height": height,
            "rotation": rotation,
            "coordinate_space": "pixels",
            "read_on": "phone",
        ])
    }

    /// The orientation that turns the native (portrait) buffer upright.
    private static func orientation(_ rotation: Int) -> CGImagePropertyOrientation {
        switch rotation {
        case 90: return .left
        case 180: return .down
        case 270: return .right
        default: return .up
        }
    }
}
