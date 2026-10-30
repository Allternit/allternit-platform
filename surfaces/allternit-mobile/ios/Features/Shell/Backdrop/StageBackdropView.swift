import SwiftUI

/// The animated background behind a bot screen (StageBackdrop.tsx + its
/// ASCII canvas). Everything moves slowly on purpose so it never pulls the
/// eye from the conversation. Colours come from the screen: a bot's colour,
/// a group's member colours. Reduce Motion (the system's or Settings'),
/// Movement: Still and `still` draw one frame.
struct StageBackdropView: View {
    enum Size { case full, compact, tile }

    let variant: String
    /// The screen's colours as hex (a bot's colour, a group's members).
    var colors: [String?] = []
    var size: Size = .full
    /// One frame (Settings tiles that aren't picked).
    var still = false
    /// ASCII looks: the fade behind the conversation (Settings → Veil).
    var veil = true
    /// Code mode has no glass panels: its veil never drops below 55%.
    var minVeil: Double = 0
    /// The page colour under the look and what the veil fades to (the
    /// terminal's colour in Code's terminal look).
    var veilColor = Color("BgPrimary")
    /// The page is dark whatever the theme (Code's terminal look).
    var darkOverride: Bool? = nil

    @ObservedObject private var store = StageBackdropStore.shared
    @ObservedObject private var appearance = AppearanceStore.shared
    @Environment(\.accessibilityReduceMotion) private var systemReduceMotion
    @Environment(\.colorScheme) private var scheme

    private var isStill: Bool { still || systemReduceMotion || appearance.reduceMotion || store.motion == .still }
    private var rate: Double { store.motion == .slow ? 0.5 : 1 }
    private var palette: [Color] { StageBackdrops.palette(colors) }
    private var isDark: Bool { darkOverride ?? (scheme == .dark) }

    var body: some View {
        ZStack {
            veilColor
            if variant != "plain" {
                if let scene = AsciiScenes.scene(variant) {
                    AsciiBackdropCanvas(scene: scene, options: asciiOptions, still: isStill, rate: rate)
                    if veil {
                        veilColor.opacity(max(store.veil, minVeil) / 100)
                    }
                } else {
                    ClassicBackdropCanvas(variant: variant, palette: palette, dark: isDark,
                                          scale: size == .full ? 1 : size == .compact ? 0.7 : 0.22,
                                          still: isStill, rate: rate)
                    if minVeil > 0 { veilColor.opacity(minVeil / 100 * 0.6) }
                }
            }
        }
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }

    private var asciiOptions: AsciiRenderOptions {
        let base: Double = size == .full ? 13 : size == .compact ? 12 : 5
        return AsciiRenderOptions(
            fontPx: max(4, (base * store.fontScale).rounded()),
            density: store.density,
            colour: store.colour,
            chars: store.chars,
            accent: colors.compactMap { $0 }.first ?? "#D97757",
            dark: isDark
        )
    }
}

/// One ASCII look on a Canvas (~30 fps). The renderer is rebuilt when the
/// look or its options change.
private struct AsciiBackdropCanvas: View {
    let scene: AsciiScene
    let options: AsciiRenderOptions
    let still: Bool
    let rate: Double

    @State private var cache = Cache()
    @State private var start = Date()

    /// Holds the renderer across frames without invalidating the view.
    private final class Cache {
        var renderer: AsciiRenderer?
        var lastTap: Date?
    }

    var body: some View {
        GeometryReader { geo in
            let origin = geo.frame(in: .global).origin
            TimelineView(.animation(minimumInterval: 1.0 / 30, paused: still)) { timeline in
                let t = still ? 3 : 3 + timeline.date.timeIntervalSince(start) * rate
                Canvas { context, size in
                    let renderer = current
                    // Still (Reduce Motion, Movement: Still) ignores the finger.
                    if !still { feedTouch(renderer, origin: origin, t: t) }
                    renderer.paint(&context, size: size, t: t)
                }
            }
        }
    }

    /// The finger from BackdropTouch, in this canvas's points.
    private func feedTouch(_ renderer: AsciiRenderer, origin: CGPoint, t: Double) {
        let touch = BackdropTouch.shared
        func local(_ p: CGPoint) -> CGPoint { CGPoint(x: p.x - origin.x, y: p.y - origin.y) }
        if let tap = touch.tap, tap.at != cache.lastTap {
            cache.lastTap = tap.at
            renderer.pulse(local(tap.point), t: t)
        }
        renderer.pointerAt(touch.point.map(local))
        renderer.stepPointer()
    }

    private var current: AsciiRenderer {
        if let renderer = cache.renderer, renderer.scene.id == scene.id, renderer.options == options { return renderer }
        let next = AsciiRenderer(scene: scene, options: options)
        cache.renderer = next
        return next
    }
}

/// The thirteen Classic looks, each drawn on one Canvas from the screen's
/// colours (soft blurred shapes; the web's CSS keyframes as slow sines).
private struct ClassicBackdropCanvas: View {
    let variant: String
    let palette: [Color]
    let dark: Bool
    /// Blur and spacing scale (full 1, compact 0.7, tile 0.22).
    let scale: Double
    let still: Bool
    let rate: Double

    @State private var start = Date()

    private func pick(_ i: Int) -> Color { palette[i % max(1, palette.count)] }

    var body: some View {
        TimelineView(.animation(minimumInterval: 1.0 / 30, paused: still)) { timeline in
            let t = still ? 3 : 3 + timeline.date.timeIntervalSince(start) * rate
            Canvas { context, size in
                draw(&context, size, t)
            }
        }
    }

    private func blob(_ context: inout GraphicsContext, _ center: CGPoint, _ radius: Double, _ color: Color, _ opacity: Double, blur: Double) {
        var layer = context
        layer.addFilter(.blur(radius: blur * scale))
        layer.opacity = opacity
        layer.fill(Path(ellipseIn: CGRect(x: center.x - radius, y: center.y - radius, width: radius * 2, height: radius * 2)), with: .color(color))
    }

    /// 0…1…0 over `period` seconds.
    private func swing(_ t: Double, _ period: Double, _ phase: Double = 0) -> Double { 0.5 - 0.5 * cos((t / period + phase) * .pi * 2) }

    private func draw(_ context: inout GraphicsContext, _ size: CGSize, _ t: Double) {
        let w = Double(size.width), h = Double(size.height)
        let m = max(w, h)
        switch variant {
        case "aurora":
            let blobs: [(Int, (Double, Double), (Double, Double), Double, Double)] = [
                (0, (-0.10, 0.25), (-0.05, 0.20), 0.60, 26), (1, (0.55, 0.30), (0.10, 0.45), 0.55, 32), (2, (0.20, 0.60), (0.55, 0.30), 0.50, 29),
            ]
            for b in blobs {
                let k = swing(t, b.4)
                let x = (b.1.0 + (b.1.1 - b.1.0) * k) * w + b.3 * w / 2
                let y = (b.2.0 + (b.2.1 - b.2.0) * k) * h + b.3 * w / 2
                blob(&context, CGPoint(x: x, y: y), b.3 * w / 2, pick(b.0), dark ? 0.32 : 0.28, blur: 60)
            }
        case "mesh":
            let pct = dark ? 0.33 : 0.25
            let drift = sin(t / 20) * 0.05
            for (i, at) in [(0.0 + drift, 0.0), (1.0 - drift, 0.0), (0.5, 1.0 + drift)].enumerated() {
                let center = CGPoint(x: at.0 * w, y: at.1 * h)
                context.fill(Path(CGRect(origin: .zero, size: size)), with: .radialGradient(
                    Gradient(colors: [pick(i).opacity(pct), .clear]), center: center, startRadius: 0, endRadius: m * 0.6))
            }
        case "constellation":
            let n = scale >= 1 ? 46 : scale >= 0.5 ? 24 : 14
            let rand = Ascii.seeded(7)
            let pts = (0..<n).map { _ -> CGPoint in
                let x = rand(), y = rand(), vx = (rand() - 0.5) * 0.03, vy = (rand() - 0.5) * 0.03
                return CGPoint(x: Ascii.wrap(x + vx * t, 1) * w, y: Ascii.wrap(y + vy * t, 1) * h)
            }
            let reach = 95 * max(scale, 0.35)
            let line = dark ? Color.white : Color(white: 0.12)
            for i in 0..<n {
                for j in (i + 1)..<max(i + 1, n) {
                    let d = hypot(pts[i].x - pts[j].x, pts[i].y - pts[j].y)
                    guard d < reach else { continue }
                    var path = Path()
                    path.move(to: pts[i])
                    path.addLine(to: pts[j])
                    context.opacity = (1 - d / reach) * 0.22
                    context.stroke(path, with: .color(line), lineWidth: 0.7)
                }
            }
            for (i, p) in pts.enumerated() {
                context.opacity = 0.75
                let r = 1.6 * max(scale, 0.5)
                context.fill(Path(ellipseIn: CGRect(x: p.x - r, y: p.y - r, width: r * 2, height: r * 2)), with: .color(pick(i)))
            }
            context.opacity = 1
        case "spotlight":
            let k = swing(t, 8)
            context.fill(Path(CGRect(origin: .zero, size: size)), with: .radialGradient(
                Gradient(colors: [pick(0).opacity(0.30 + 0.1 * k), pick(0).opacity(0.08), .clear]),
                center: CGPoint(x: w * 0.5, y: h * 0.3), startRadius: 0, endRadius: m * (0.45 + 0.05 * k)))
        case "horizon":
            for i in 0..<3 {
                let base = h * (0.62 + Double(i) * 0.1)
                var path = Path()
                path.move(to: CGPoint(x: 0, y: h))
                var x = 0.0
                while x <= w {
                    path.addLine(to: CGPoint(x: x, y: base + sin(x / w * .pi * 2 + t * (0.25 + Double(i) * 0.08) + Double(i)) * h * 0.03))
                    x += max(4, w / 60)
                }
                path.addLine(to: CGPoint(x: w, y: h))
                path.closeSubpath()
                context.opacity = dark ? 0.22 : 0.16
                context.fill(path, with: .color(pick(i)))
            }
            context.opacity = 1
        case "silk":
            for i in 0..<3 {
                var path = Path()
                var x = -0.1 * w
                let y0 = h * (0.25 + Double(i) * 0.22)
                path.move(to: CGPoint(x: x, y: y0))
                while x <= w * 1.1 {
                    path.addLine(to: CGPoint(x: x, y: y0 + sin(x / w * .pi * 1.4 + t * 0.18 + Double(i) * 1.7) * h * 0.08))
                    x += max(4, w / 50)
                }
                var layer = context
                layer.addFilter(.blur(radius: 24 * scale))
                layer.opacity = dark ? 0.30 : 0.22
                layer.stroke(path, with: .color(pick(i)), lineWidth: 70 * scale)
            }
        case "bokeh":
            let rand = Ascii.seeded(11)
            for i in 0..<(scale >= 0.5 ? 14 : 8) {
                let x = rand() * w, speed = 0.01 + rand() * 0.02, r = (12 + rand() * 30) * scale, p = rand()
                let y = h - Ascii.wrap(p + t * speed, 1) * (h + r * 2) + r
                blob(&context, CGPoint(x: x, y: y), r, pick(i), dark ? 0.35 : 0.25, blur: 6)
            }
        case "grid":
            let gap = max(6, 22 * scale)
            let band = Ascii.wrap(t * 0.06, 1.4) - 0.2
            var y = gap / 2
            while y < h {
                var x = gap / 2
                while x < w {
                    let d = abs((x / w + y / h) / 2 - band)
                    let glow = max(0, 1 - d * 6)
                    context.opacity = 0.14 + glow * 0.5
                    let r = 1 + glow * 1.2
                    context.fill(Path(ellipseIn: CGRect(x: x - r, y: y - r, width: r * 2, height: r * 2)), with: .color(glow > 0.05 ? pick(0) : (dark ? .white : .black)))
                    x += gap
                }
                y += gap
            }
            context.opacity = 1
        case "lava":
            for i in 0..<5 {
                let id = Double(i)
                let x = w * (0.2 + 0.6 * swing(t, 23 + id * 4, id * 0.2))
                let y = h * (0.15 + 0.7 * swing(t, 31 + id * 3, id * 0.37))
                blob(&context, CGPoint(x: x, y: y), w * (0.18 + 0.05 * sin(t / 7 + id)), pick(i), dark ? 0.34 : 0.26, blur: 34)
            }
        case "prism":
            context.opacity = dark ? 0.26 : 0.2
            context.fill(Path(CGRect(origin: .zero, size: size)), with: .conicGradient(
                Gradient(colors: [pick(0), pick(1), pick(2), pick(0)]), center: CGPoint(x: w / 2, y: h / 2), angle: .degrees(t * 3)))
            context.opacity = 1
        case "fireflies":
            let rand = Ascii.seeded(23)
            for i in 0..<(scale >= 0.5 ? 26 : 12) {
                let x0 = rand(), y0 = rand(), p = rand() * Ascii.tau
                let x = Ascii.wrap(x0 + sin(t * 0.1 + p) * 0.05, 1) * w
                let y = Ascii.wrap(y0 + cos(t * 0.08 + p) * 0.05, 1) * h
                let on = max(0, sin(t * 0.9 + p))
                blob(&context, CGPoint(x: x, y: y), 3 * max(scale, 0.5), i % 3 == 0 ? Color(hex: "#ffd166") : pick(i), 0.2 + on * 0.7, blur: 2)
            }
        case "dusk":
            let k = swing(t, 40)
            context.fill(Path(CGRect(origin: .zero, size: size)), with: .linearGradient(
                Gradient(colors: [.clear, pick(1).opacity(0.10 + 0.05 * k), pick(0).opacity(dark ? 0.30 : 0.24)]),
                startPoint: .zero, endPoint: CGPoint(x: 0, y: h)))
            blob(&context, CGPoint(x: w / 2, y: h * 1.02), w * 0.5, pick(0), 0.28 + 0.08 * k, blur: 50)
        default:
            break
        }
    }
}

/// The background plus a frosted veil, behind the drawer's contents
/// (RailBackdrop): the look of the screen that's open, in its colours.
struct RailBackdropView: View {
    let look: RailBackdropStore.Look

    var body: some View {
        ZStack {
            StageBackdropView(variant: look.variant, colors: look.colors, size: .compact, veil: false)
            Rectangle().fill(.ultraThinMaterial)
            Color("BgPrimary").opacity(0.38)
        }
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }
}

/// Where the finger is on a screen with a background (the web backdrop's
/// pointer and click, on touch): read by the ASCII canvas every frame.
@MainActor
final class BackdropTouch {
    static let shared = BackdropTouch()

    #if DEBUG
    /// `-backdrop-touch x,y`: holds a finger there (screenshots of the push).
    private init() {
        if let raw = UserDefaults.standard.string(forKey: "backdrop-touch") {
            let n = raw.split(separator: ",").compactMap { Double($0) }
            if n.count == 2 { point = CGPoint(x: n[0], y: n[1]) }
        }
    }
    #endif
    /// The finger in global points while it's down.
    var point: CGPoint?
    /// The last tap, in global points.
    var tap: (point: CGPoint, at: Date)?
}

extension View {
    /// Feeds touches on this screen to its background: a drag pushes the
    /// characters, a tap sends the ring. Taps and scrolls still reach the
    /// screen.
    func backdropTouches(_ enabled: Bool = true) -> some View {
        simultaneousGesture(
            DragGesture(minimumDistance: 0, coordinateSpace: .global)
                .onChanged { value in
                    guard enabled else { return }
                    BackdropTouch.shared.point = value.location
                }
                .onEnded { value in
                    guard enabled else { return }
                    BackdropTouch.shared.point = nil
                    if hypot(value.translation.width, value.translation.height) < 10 {
                        BackdropTouch.shared.tap = (value.location, Date())
                    }
                }
        )
    }
}
