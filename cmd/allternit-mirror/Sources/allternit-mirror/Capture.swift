// Eyes: one frame of the iPhone Mirroring window (ScreenCaptureKit, that
// window only, no cursor, no shadow) and Vision text recognition on it.

import CoreGraphics
import Foundation
import ImageIO
import MirrorCore
import ScreenCaptureKit
import UniformTypeIdentifiers
import Vision

struct TextLine {
    let text: String
    let box: CGRect // Vision: normalized, bottom-left origin
    let confidence: Float
}

enum Capture {
    /// The window's pixels at its backing scale: the image every tap coordinate refers to.
    static func image(_ window: MirrorWindow) async throws -> CGImage {
        let content: SCShareableContent
        do {
            content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
        } catch {
            throw MirrorError("capture_failed", "Could not list windows to capture: \(error.localizedDescription)")
        }
        guard let target = content.windows.first(where: { $0.windowID == window.id }) else {
            throw MirrorError("no_window", "The iPhone Mirroring window went away.", extra: ["health": Health.windowHidden.rawValue])
        }
        let filter = SCContentFilter(desktopIndependentWindow: target)
        let config = SCStreamConfiguration()
        config.width = window.geometry.pixelWidth
        config.height = window.geometry.pixelHeight
        config.showsCursor = false
        config.ignoreShadowsSingleWindow = true
        config.capturesAudio = false
        do {
            return try await SCScreenshotManager.captureImage(contentFilter: filter, configuration: config)
        } catch {
            throw MirrorError("capture_failed", "Could not capture the iPhone Mirroring window: \(error.localizedDescription)")
        }
    }

    static func png(_ image: CGImage) throws -> Data {
        let data = NSMutableData()
        guard let dest = CGImageDestinationCreateWithData(data, UTType.png.identifier as CFString, 1, nil) else {
            throw MirrorError("capture_failed", "Could not encode PNG.")
        }
        CGImageDestinationAddImage(dest, image, nil)
        guard CGImageDestinationFinalize(dest) else { throw MirrorError("capture_failed", "Could not encode PNG.") }
        return data as Data
    }

    /// A small grayscale copy (rows top to bottom), for Desktop's changed-screen check.
    static func gray(_ image: CGImage, maxWidth: Int = 180) -> (width: Int, height: Int, data: Data)? {
        let w = max(1, min(maxWidth, image.width))
        let h = max(1, Int((Double(image.height) * Double(w) / Double(image.width)).rounded()))
        guard let ctx = CGContext(
            data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w,
            space: CGColorSpaceCreateDeviceGray(), bitmapInfo: CGImageAlphaInfo.none.rawValue
        ) else { return nil }
        ctx.interpolationQuality = .medium
        ctx.draw(image, in: CGRect(x: 0, y: 0, width: w, height: h))
        guard let base = ctx.data else { return nil }
        return (w, h, Data(bytes: base, count: w * h))
    }

    /// A JPEG or PNG Desktop sent (read_image).
    static func decode(_ data: Data) throws -> CGImage {
        guard let source = CGImageSourceCreateWithData(data as CFData, nil),
              let image = CGImageSourceCreateImageAtIndex(source, 0, nil) else {
            throw MirrorError("bad_request", "The image could not be read.")
        }
        return image
    }

    static func text(_ image: CGImage, fast: Bool) throws -> [TextLine] {
        let request = VNRecognizeTextRequest()
        request.recognitionLevel = fast ? .fast : .accurate
        request.usesLanguageCorrection = !fast
        do {
            try VNImageRequestHandler(cgImage: image, options: [:]).perform([request])
        } catch {
            throw MirrorError("read_failed", "Text recognition failed: \(error.localizedDescription)")
        }
        return (request.results ?? []).compactMap { obs in
            guard let top = obs.topCandidates(1).first else { return nil }
            return TextLine(text: top.string, box: obs.boundingBox, confidence: top.confidence)
        }
    }
}
