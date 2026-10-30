import SwiftUI
import UIKit

/// The ASCII backgrounds' engine (allternit-ai
/// src/components/bots/stage/backdrop/ascii/ascii-engine.ts): each look
/// draws characters into a grid every frame, and one Canvas paints the grid.
/// A finger stands in for the cursor: a drag pushes nearby characters aside
/// and brightens them, a tap sends a ring outward (BackdropTouch).
enum Ascii {
    static let tau = Double.pi * 2

    static func hash(_ n: Double) -> Double {
        var x = UInt32(bitPattern: Int32(truncatingIfNeeded: Int(n.rounded(.down))))
        x = (x ^ 0x9e37_79b9) &* 0x85eb_ca6b
        x ^= x >> 13
        x = x &* 0xc2b2_ae35
        x ^= x >> 16
        return Double(x) / 4_294_967_296
    }

    /// Seeded so a look draws the same layout every time it mounts.
    static func seeded(_ seed: Int32) -> () -> Double {
        var a = UInt32(bitPattern: seed)
        return {
            a = a &+ 0x6d2b_79f5
            var t = (a ^ (a >> 15)) &* (1 | a)
            t = (t &+ ((t ^ (t >> 7)) &* (61 | t))) ^ t
            return Double(t ^ (t >> 14)) / 4_294_967_296
        }
    }

    static func smooth(_ t: Double) -> Double { t * t * (3 - 2 * t) }

    private static func valueNoise(_ x: Double, _ y: Double, _ s: Double) -> Double {
        let xi = x.rounded(.down), yi = y.rounded(.down)
        let u = smooth(x - xi), v = smooth(y - yi)
        func h(_ a: Double, _ b: Double) -> Double { hash(a * 7919 + b * 104_729 + s * 15_485_863) }
        return (h(xi, yi) * (1 - u) + h(xi + 1, yi) * u) * (1 - v) + (h(xi, yi + 1) * (1 - u) + h(xi + 1, yi + 1) * u) * v
    }

    /// Three octaves of value noise, 0..1.
    static func fbm(_ x: Double, _ y: Double, _ s: Double = 0) -> Double {
        valueNoise(x, y, s) * 0.55 + valueNoise(x * 2.1, y * 2.1, s + 1) * 0.3 + valueNoise(x * 4.3, y * 4.3, s + 2) * 0.15
    }

    /// How many of something to draw at this density, never fewer than `min`.
    static func count(_ n: Double, _ density: Double, _ min: Int = 2) -> Int {
        Swift.max(min, Int((n * density).rounded()))
    }

    /// JS `%`: the remainder keeps the dividend's sign.
    static func mod(_ a: Double, _ b: Double) -> Double { a.truncatingRemainder(dividingBy: b) }
    /// Always positive.
    static func wrap(_ a: Double, _ b: Double) -> Double { ((a.truncatingRemainder(dividingBy: b)) + b).truncatingRemainder(dividingBy: b) }

    static func line(_ g: AsciiGrid, _ x0: Double, _ y0: Double, _ x1: Double, _ y1: Double, _ ch: Character, _ col: String, _ a: Double) {
        let n = Int(Swift.max(abs(x1 - x0), abs(y1 - y0)))
        guard n > 1 else { return }
        for i in 1..<n {
            let f = Double(i) / Double(n)
            g.put(x0 + (x1 - x0) * f, y0 + (y1 - y0) * f, ch, col, a)
        }
    }

    struct Star { let x: Double; let y: Double; let p: Double }

    static func stars(_ rand: () -> Double, _ c: Int, _ r: Double, _ n: Double, top: Double = 1) -> [Star] {
        (0..<Swift.max(4, Int(n.rounded()))).map { _ in
            Star(x: (rand() * Double(c)).rounded(.down), y: (rand() * r * top).rounded(.down), p: rand() * tau)
        }
    }

    static func drawStars(_ g: AsciiGrid, _ list: [Star], _ t: Double, _ col: String) {
        for s in list { g.put(s.x, s.y, ".", col, 0.35 + 0.35 * sin(t * 1.5 + s.p)) }
    }
}

/// The character grid a look draws into. A space with tiles on renders as a solid block.
final class AsciiGrid {
    let c: Int
    let r: Int
    var ch: [Character?]
    var col: [String]
    var a: [Double]

    init(c: Int, r: Int) {
        self.c = c
        self.r = r
        ch = Array(repeating: nil, count: c * r)
        col = Array(repeating: "", count: c * r)
        a = Array(repeating: 0, count: c * r)
    }

    func clear() {
        for i in ch.indices { ch[i] = nil }
    }

    func put(_ x: Double, _ y: Double, _ char: Character, _ colour: String, _ alpha: Double = 1) {
        guard x.isFinite, y.isFinite else { return }
        let xi = Int((x + 0.5).rounded(.down)), yi = Int((y + 0.5).rounded(.down))
        guard xi >= 0, yi >= 0, xi < c, yi < r, alpha > 0.02 else { return }
        let i = yi * c + xi
        ch[i] = char
        col[i] = colour
        a[i] = min(1, alpha)
    }

    func erase(_ x: Double, _ y: Double) {
        let xi = Int((x + 0.5).rounded(.down)), yi = Int((y + 0.5).rounded(.down))
        if xi >= 0, yi >= 0, xi < c, yi < r { ch[yi * c + xi] = nil }
    }

    func text(_ x: Double, _ y: Double, _ s: String, _ colour: String, _ alpha: Double = 1) {
        for (i, char) in s.enumerated() where char != " " { put(x + Double(i), y, char, colour, alpha) }
    }

    func sprite(_ x: Double, _ y: Double, _ lines: [String], _ colour: String, _ alpha: Double = 1) {
        for (j, l) in lines.enumerated() { text(x, y + Double(j), l, colour, alpha) }
    }

    /// Reads a cell (the lake looks mirror what's above).
    func cell(_ i: Int) -> (ch: Character, col: String, a: Double)? {
        guard i >= 0, i < ch.count, let char = ch[i] else { return nil }
        return (char, col[i], a[i])
    }
}

/// The finger in grid cells: s fades 0..1 as it lands and lifts; wt/wx/wy
/// are the last tap.
struct AsciiPointer {
    var x: Double = -99
    var y: Double = -99
    var s: Double = 0
    var wt: Double = -99
    var wx: Double = -99
    var wy: Double = -99
}

typealias AsciiDraw = (AsciiGrid, Double, AsciiPointer) -> Void

struct AsciiScene: Sendable {
    let id: String
    let name: String
    let note: String
    /// Background gradient, top to bottom.
    let bg: [String]
    /// Characters on their own tiles, as designed.
    var tile = false
    /// Spaces draw as seamless blocks (the A-block mark).
    var solid = false
    /// Handles the pointer itself; skip the shared push and pulse.
    var own = false
    let make: @Sendable (_ c: Int, _ r: Int, _ rand: @escaping () -> Double, _ density: Double) -> AsciiDraw
}

// MARK: - Colour

/// Hex colours as HSL, so a look can be recoloured (Bot's colour, Mono) or
/// have its lightness flipped to sit under the app's theme.
struct AsciiHSL: Hashable {
    var h: Double
    var s: Double
    var l: Double

    init(h: Double, s: Double, l: Double) {
        self.h = h
        self.s = s
        self.l = l
    }

    init(hex: String) {
        var value = hex.trimmingCharacters(in: .whitespaces)
        if value.hasPrefix("#") { value.removeFirst() }
        if value.count == 3 { value = value.map { "\($0)\($0)" }.joined() }
        let n = UInt32(value.prefix(6), radix: 16) ?? 0
        let r = Double((n >> 16) & 255) / 255, g = Double((n >> 8) & 255) / 255, b = Double(n & 255) / 255
        let mx = max(r, g, b), mn = min(r, g, b)
        l = (mx + mn) / 2
        h = 0
        s = 0
        if mx != mn {
            let d = mx - mn
            s = l > 0.5 ? d / (2 - mx - mn) : d / (mx + mn)
            if mx == r { h = (g - b) / d + (g < b ? 6 : 0) }
            else if mx == g { h = (b - r) / d + 2 }
            else { h = (r - g) / d + 4 }
            h /= 6
        }
    }

    /// HSL → Color (SwiftUI takes HSB).
    var color: Color {
        let v = l + s * min(l, 1 - l)
        let sv = v == 0 ? 0 : 2 * (1 - l / v)
        return Color(hue: h, saturation: sv, brightness: v)
    }
}

enum AsciiColourMode: String, CaseIterable { case scene, accent, mono }
enum AsciiCharMode: String, CaseIterable { case design, text, tiles }

struct AsciiRenderOptions: Equatable {
    var fontPx: Double
    var density: Double
    var colour: AsciiColourMode
    var chars: AsciiCharMode
    /// The screen's colour for Bot's colour mode (hex).
    var accent: String
    /// The app is dark. A look whose sky is the other tone is drawn with its
    /// lightness flipped, so text that follows the theme stays readable.
    var dark: Bool
}

/// One look on one canvas: builds the grid for a size and paints frames.
final class AsciiRenderer {
    let scene: AsciiScene
    let options: AsciiRenderOptions
    private var grid: AsciiGrid?
    private var draw: AsciiDraw?
    private var size: CGSize = .zero
    private(set) var cw: Double
    private(set) var chh: Double
    private var colours: [String: Color] = [:]
    private let accentHSL: AsciiHSL
    private let flip: Bool
    var pointer = AsciiPointer()
    private var target = (x: -99.0, y: -99.0, inside: false)
    private static let pulseSeconds = 1.6
    private static let pushRadius = 12.0

    init(scene: AsciiScene, options: AsciiRenderOptions) {
        self.scene = scene
        self.options = options
        accentHSL = AsciiHSL(hex: options.accent)
        let ls = scene.bg.map { AsciiHSL(hex: $0).l }
        let sceneDark = ls.reduce(0, +) / Double(max(1, ls.count)) < 0.5
        flip = sceneDark != options.dark
        let font = UIFont.monospacedSystemFont(ofSize: options.fontPx, weight: .regular)
        cw = Double(("M" as NSString).size(withAttributes: [.font: font]).width)
        if cw <= 0 { cw = options.fontPx * 0.6 }
        chh = options.fontPx * 1.2
    }

    private func fit(_ next: CGSize) {
        guard next != size, next.width > 0, next.height > 0 else { return }
        size = next
        let c = Int((Double(next.width) / cw).rounded(.up))
        let r = Int((Double(next.height) / chh).rounded(.up))
        grid = AsciiGrid(c: c, r: r)
        let seed = scene.id.unicodeScalars.reduce(Int32(7)) { $0 &* 31 &+ Int32($1.value) }
        draw = scene.make(c, r, Ascii.seeded(seed), options.density)
    }

    /// The look's colour for a hex, recoloured and flipped as the options ask.
    func tc(_ col: String) -> Color {
        if let cached = colours[col] { return cached }
        var hsl = AsciiHSL(hex: col)
        switch options.colour {
        case .scene: break
        case .mono: hsl.s = 0
        case .accent:
            if hsl.s >= 0.06 { hsl.s = min(1, accentHSL.s * 0.85) }
            hsl.h = accentHSL.h
        }
        if flip { hsl.l = 1 - hsl.l }
        let value = (options.colour == .scene && !flip) ? Color(hex: col) : hsl.color
        colours[col] = value
        return value
    }

    /// The finger in points relative to the canvas, or nil when it lifts.
    func pointerAt(_ point: CGPoint?) {
        guard let point else { target.inside = false; return }
        target.x = Double(point.x) / cw
        target.y = Double(point.y) / chh
        if !target.inside && pointer.s < 0.01 {
            pointer.x = target.x
            pointer.y = target.y
        }
        target.inside = true
    }

    func pulse(_ point: CGPoint, t: Double) {
        pointerAt(point)
        pointer.wt = t
        pointer.wx = Double(point.x) / cw
        pointer.wy = Double(point.y) / chh
    }

    /// Eases the pointer toward the finger.
    func stepPointer() {
        pointer.x += (target.x - pointer.x) * 0.3
        pointer.y += (target.y - pointer.y) * 0.3
        pointer.s += ((target.inside ? 1 : 0) - pointer.s) * 0.18
    }

    func paint(_ context: inout GraphicsContext, size next: CGSize, t: Double) {
        fit(next)
        guard let g = grid, let draw else { return }
        let rect = CGRect(origin: .zero, size: next)
        let stops = scene.bg.map { tc($0) }
        context.fill(Path(rect), with: .linearGradient(
            Gradient(colors: stops.count > 1 ? stops : [stops.first ?? .black, stops.first ?? .black]),
            startPoint: .zero, endPoint: CGPoint(x: 0, y: next.height)))
        g.clear()
        draw(g, t, pointer)
        let tiles = options.chars == .tiles || (options.chars == .design && scene.tile)
        let font = Font.system(size: options.fontPx, design: .monospaced)
        let ink: Color = flip ? .white : .black
        var resolved: [String: GraphicsContext.ResolvedText] = [:]
        let P = pointer
        let push = P.s > 0.01 && !scene.own
        let age = t - P.wt
        let wave = age >= 0 && age < Self.pulseSeconds && !scene.own
        let asp = chh / cw
        for y in 0..<g.r {
            for x in 0..<g.c {
                let i = y * g.c + x
                guard let char = g.ch[i] else { continue }
                var a = g.a[i]
                let colKey = g.col[i]
                var px = Double(x) * cw, py = Double(y) * chh
                if push {
                    let dx = Double(x) - P.x, dy = (Double(y) - P.y) * asp
                    let d = (dx * dx + dy * dy).squareRoot() + 0.001
                    if d < Self.pushRadius {
                        let f = pow(1 - d / Self.pushRadius, 2) * P.s * 2.4
                        px += dx / d * f * cw
                        py += dy / d * f * cw
                        a = min(1, a + 0.35 * f)
                    }
                }
                if wave {
                    let dx = Double(x) - P.wx, dy = (Double(y) - P.wy) * asp
                    let d = (dx * dx + dy * dy).squareRoot() + 0.001
                    let k = exp(-pow(d - age * 45, 2) / 30) * (1 - age / Self.pulseSeconds)
                    if k > 0.01 {
                        px += dx / d * k * cw * 2.2
                        py += dy / d * k * cw * 2.2
                        a = min(1, a + 0.5 * k)
                    }
                }
                if tiles && a >= 0.8 {
                    let fill = tc(colKey)
                    if scene.solid && char == " " {
                        context.opacity = a
                        context.fill(Path(CGRect(x: px.rounded(.down), y: py.rounded(.down), width: cw.rounded(.up) + 1, height: chh.rounded(.up) + 1)), with: .color(fill))
                        continue
                    }
                    context.opacity = a * 0.92
                    context.fill(Path(CGRect(x: px + 0.5, y: py + 0.5, width: cw - 1, height: chh - 1)), with: .color(fill))
                    if char != " " {
                        context.opacity = 0.42
                        let key = "\(char)|tile"
                        let text = resolved[key] ?? context.resolve(Text(String(char)).font(font).foregroundColor(ink))
                        resolved[key] = text
                        context.draw(text, at: CGPoint(x: px, y: py), anchor: .topLeading)
                    }
                } else {
                    context.opacity = a
                    let glyph = char == " " ? "█" : String(char)
                    let key = "\(glyph)|\(colKey)"
                    let text = resolved[key] ?? context.resolve(Text(glyph).font(font).foregroundColor(tc(colKey)))
                    resolved[key] = text
                    context.draw(text, at: CGPoint(x: px, y: py), anchor: .topLeading)
                }
            }
        }
        context.opacity = 1
    }
}
