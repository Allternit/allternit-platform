import SwiftUI
import UIKit

/// The "working" look for anything that is waiting on something real: a field
/// of dots (the iOS side of the web/Android dot field, DOT-EFFECTS spec).
///
/// - `.field`: ink dots on the card, a light band sweeping (or sine waves).
/// - `.halftone`: a real picture drawn as dots, finer as the real fraction
///   grows (coarse and held while the fraction is unknown, solid at 0.85+).
/// - `.reveal`: the real picture arriving, cells blooming in random order or a
///   crossfade out of the halftone, once, over `duration`.
///
/// The dots only mean "working": callers keep their own words, seconds and
/// bar. Reduce Motion draws a still frame (a fixed medium halftone, no sweep,
/// the reveal at its end). The timeline pauses off screen and when the app is
/// not active.
struct DotFieldView: View {
    enum Pattern { case band, sine }
    enum RevealStyle { case cells, crossfade }

    enum Mode {
        case field(Pattern = .band, amplitude: Double = 1)
        case halftone(UIImage, fraction: Double?)
        case reveal(UIImage, RevealStyle, duration: Double = 0.8)
    }

    var mode: Mode
    /// Dot pitch in points.
    var pitch: CGFloat = 9

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.scenePhase) private var scenePhase
    @State private var onScreen = false
    @State private var startedAt = Date()
    @State private var sampler: DotSampler?

    private var running: Bool { onScreen && scenePhase == .active && !reduceMotion }

    var body: some View {
        GeometryReader { proxy in
            TimelineView(.animation(minimumInterval: 1.0 / 30.0, paused: !running)) { timeline in
                Canvas { ctx, size in
                    draw(&ctx, size: size, now: timeline.date)
                }
            }
            .task(id: samplerKey(proxy.size)) { sampler = makeSampler(proxy.size) }
        }
        .allowsHitTesting(false)
        .accessibilityHidden(true)
        .onAppear { onScreen = true; startedAt = Date() }
        .onDisappear { onScreen = false }
    }

    // MARK: - Image sampling (once per image and size)

    private var image: UIImage? {
        switch mode {
        case .field: return nil
        case .halftone(let i, _), .reveal(let i, _, _): return i
        }
    }

    private func samplerKey(_ size: CGSize) -> String {
        guard let image else { return "none" }
        return "\(ObjectIdentifier(image).hashValue)-\(Int(size.width))x\(Int(size.height))"
    }

    private func makeSampler(_ size: CGSize) -> DotSampler? {
        guard let image, size.width > 1, size.height > 1 else { return nil }
        return DotSampler(image: image, size: size)
    }

    // MARK: - Drawing

    private func draw(_ ctx: inout GraphicsContext, size: CGSize, now: Date) {
        let t = reduceMotion ? 3.2 : now.timeIntervalSinceReferenceDate
        let ink = Color("TextPrimary")
        switch mode {
        case .field(let pattern, let amplitude):
            drawField(&ctx, size: size, t: t, ink: ink, pattern: pattern, amplitude: reduceMotion ? 0.55 : amplitude)
        case .halftone(_, let fraction):
            drawHalftone(&ctx, size: size, fraction: reduceMotion ? 0.5 : fraction, ink: ink)
        case .reveal(_, let style, let duration):
            let q = reduceMotion ? 1 : min(1, max(0, now.timeIntervalSince(startedAt) / duration))
            drawReveal(&ctx, size: size, q: q, style: style, ink: ink)
        }
    }

    static func pattern(_ p: Pattern, t: Double, x: CGFloat, y: CGFloat, w: CGFloat, h: CGFloat) -> Double {
        let cx = Double(x / w) - 0.5, cy = Double(y / h) - 0.5
        switch p {
        case .band:
            let d = (cx + cy * 0.6) - ((t * 0.35).truncatingRemainder(dividingBy: 1.6) - 0.8)
            return 0.25 + 0.75 * exp(-d * d * 40)
        case .sine:
            return 0.5 + 0.5 * sin(cx * 9 + sin(cy * 6 + t) * 1.2 - t * 2.2)
        }
    }

    private func drawField(_ ctx: inout GraphicsContext, size: CGSize, t: Double, ink: Color, pattern: Pattern, amplitude: Double) {
        let p = pitch
        var y = p / 2
        while y < size.height {
            var x = p / 2
            while x < size.width {
                let v = Self.pattern(pattern, t: t, x: x, y: y, w: size.width, h: size.height) * amplitude
                let r = p * 0.16 + p * 0.2 * v
                let dot = Path(ellipseIn: CGRect(x: x - r, y: y - r, width: r * 2, height: r * 2))
                ctx.fill(dot, with: .color(ink.opacity(0.18 + 0.6 * v)))
                x += p
            }
            y += p
        }
    }

    /// Pitch 2.2p down to 0.3p by the real fraction; unknown holds the coarse pitch.
    static func halftonePitch(base p: CGFloat, fraction: Double?) -> CGFloat {
        guard let f = fraction else { return p * 2.2 }
        let c = min(1, max(0, f))
        return max(3, p * (2.2 - 1.9 * c))
    }

    private func drawHalftone(_ ctx: inout GraphicsContext, size: CGSize, fraction: Double?, ink: Color, alpha: Double = 1) {
        guard let sampler else { return }
        let pp = Self.halftonePitch(base: pitch, fraction: fraction)
        let solid = (fraction ?? 0) >= 0.85
        var y = pp / 2
        while y < size.height {
            var x = pp / 2
            while x < size.width {
                let c = sampler.color(x: x, y: y)
                let l = c.luma
                let r = pp * 0.5 * (solid ? 1.05 : (0.35 + 0.65 * (1 - l * 0.6)))
                let dot = Path(ellipseIn: CGRect(x: x - r, y: y - r, width: r * 2, height: r * 2))
                ctx.fill(dot, with: .color(Color(red: c.r, green: c.g, blue: c.b).opacity(alpha)))
                x += pp
            }
            y += pp
        }
    }

    private func drawReveal(_ ctx: inout GraphicsContext, size: CGSize, q: Double, style: RevealStyle, ink: Color) {
        guard let sampler, let image else { return }
        switch style {
        case .crossfade:
            // Halftone (solid) fading into the real picture.
            drawHalftone(&ctx, size: size, fraction: 1, ink: ink)
            var layer = ctx
            layer.opacity = q
            layer.draw(Image(uiImage: image), in: CGRect(origin: .zero, size: size).aspectFillRect(of: image.size))
        case .cells:
            let p = pitch
            var y = p / 2
            var row = 0
            while y < size.height {
                var x = p / 2
                var col = 0
                while x < size.width {
                    if q >= Self.cellThreshold(col, row) {
                        let c = sampler.color(x: x, y: y)
                        ctx.fill(Path(CGRect(x: x - p / 2, y: y - p / 2, width: p + 1, height: p + 1)),
                                 with: .color(Color(red: c.r, green: c.g, blue: c.b)))
                    } else {
                        let v = 0.55
                        let r = p * 0.16 + p * 0.2 * v
                        ctx.fill(Path(ellipseIn: CGRect(x: x - r, y: y - r, width: r * 2, height: r * 2)),
                                 with: .color(ink.opacity(0.18 + 0.6 * v)))
                    }
                    x += p; col += 1
                }
                y += p; row += 1
            }
            if q > 0.75 {
                var layer = ctx
                layer.opacity = (q - 0.75) * 4
                layer.draw(Image(uiImage: image), in: CGRect(origin: .zero, size: size).aspectFillRect(of: image.size))
            }
        }
    }

    /// Stable per-cell value in 0...1, so the same cells bloom in the same order.
    static func cellThreshold(_ col: Int, _ row: Int) -> Double {
        let s = sin(Double(col) * 12.9898 + Double(row) * 78.233) * 43758.5453
        return s - s.rounded(.down)
    }
}

// MARK: - Helpers

/// The picture reduced once to a small aspect-filled bitmap the dots read from.
final class DotSampler {
    struct RGB { let r: Double, g: Double, b: Double; var luma: Double { r * 0.3 + g * 0.59 + b * 0.11 } }

    private let w: Int, h: Int
    private let scaleX: CGFloat, scaleY: CGFloat
    private var data = [UInt8]()

    init?(image: UIImage, size: CGSize) {
        let maxSide: CGFloat = 200
        let k = min(1, maxSide / max(size.width, size.height))
        w = max(1, Int(size.width * k)); h = max(1, Int(size.height * k))
        scaleX = CGFloat(w) / size.width; scaleY = CGFloat(h) / size.height
        data = [UInt8](repeating: 0, count: w * h * 4)
        let space = CGColorSpaceCreateDeviceRGB()
        let drawn: Bool = data.withUnsafeMutableBytes { buf in
            guard let cg = CGContext(data: buf.baseAddress, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4, space: space,
                                     bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue),
                  let src = image.cgImage else { return false }
            cg.interpolationQuality = .medium
            let rect = CGRect(x: 0, y: 0, width: w, height: h).aspectFillRect(of: CGSize(width: src.width, height: src.height))
            cg.draw(src, in: rect)
            return true
        }
        if !drawn { return nil }
    }

    func color(x: CGFloat, y: CGFloat) -> RGB {
        let ix = min(w - 1, max(0, Int(x * scaleX))), iy = min(h - 1, max(0, Int(y * scaleY)))
        let i = (iy * w + ix) * 4
        return RGB(r: Double(data[i]) / 255, g: Double(data[i + 1]) / 255, b: Double(data[i + 2]) / 255)
    }
}

extension CGRect {
    /// This rect grown (centered) so a picture of `source` size covers it.
    func aspectFillRect(of source: CGSize) -> CGRect {
        guard source.width > 0, source.height > 0, width > 0, height > 0 else { return self }
        let s = max(width / source.width, height / source.height)
        let nw = source.width * s, nh = source.height * s
        return CGRect(x: minX - (nw - width) / 2, y: minY - (nh - height) / 2, width: nw, height: nh)
    }
}
