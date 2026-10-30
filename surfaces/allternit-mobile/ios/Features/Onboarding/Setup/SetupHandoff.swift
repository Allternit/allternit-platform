import SwiftUI
import UIKit

/// The branded hand-off from setup into the app (~4.5 s), ported from
/// allternit-ai `setup/SetupHandoff.tsx` (built on the locked brand intro):
/// every connected node in the setup graph collapses into a block that flies
/// in to build the A:// mark, the coral core pops and lands in the A, a
/// shockwave + light sweep run, the person's name types under the mark,
/// the core heartbeats, "://" dissolves right→left, and the A's blocks arc
/// up toward the top bar as the app shows through.
///
/// It runs in its OWN window over the scene (the web's own React root):
/// setup is swapped out for the workspace when `finish` lands at 3.3 s, and
/// the flight has to keep going over the freshly mounted app. A tap skips to
/// the end. `finish` runs exactly once, with a timer backstop.
@MainActor
enum SetupHandoff {
    private static var window: UIWindow?

    /// `finish` runs exactly once across the view, the backstop timers and a tap.
    private final class Once { var done = false }

    /// A name as the person gave it, trimmed and capped so it fits under the mark; empty → nil.
    static func name(from raw: String?) -> String? {
        let name = (raw ?? "").split(whereSeparator: \.isWhitespace).joined(separator: " ")
        guard !name.isEmpty else { return nil }
        return name.count > 24 ? String(name.prefix(23)) + "…" : name
    }

    /// Mounts the hand-off and returns true, or false (the caller finishes at
    /// once) when there is no active scene to draw in.
    @discardableResult
    static func start(sources: [CGPoint], name: String?, dark: Bool, reduced: Bool, finish: @escaping @MainActor () -> Void) -> Bool {
        guard window == nil,
              let scene = UIApplication.shared.connectedScenes
                .compactMap({ $0 as? UIWindowScene })
                .first(where: { $0.activationState == .foregroundActive }) else { return false }

        let once = Once()
        let fire: @MainActor () -> Void = {
            guard !once.done else { return }
            once.done = true
            finish()
        }
        let host = UIHostingController(rootView: SetupHandoffView(
            sources: sources, name: name, dark: dark, reduced: reduced,
            onFinish: fire, onDone: { teardown() }))
        host.view.backgroundColor = .clear
        let w = UIWindow(windowScene: scene)
        w.windowLevel = .alert + 1
        w.backgroundColor = .clear
        w.rootViewController = host
        w.isHidden = false
        window = w

        let finishAt = reduced ? SetupHandoffView.reducedFinishAt : SetupHandoffView.finishAt
        let end = reduced ? SetupHandoffView.reducedEnd : SetupHandoffView.end
        DispatchQueue.main.asyncAfter(deadline: .now() + finishAt + 0.7) { fire() }
        DispatchQueue.main.asyncAfter(deadline: .now() + end + 1.5) { fire(); teardown() }
        return true
    }

    static func teardown() {
        window?.isHidden = true
        window = nil
    }
}

struct SetupHandoffView: View {
    let sources: [CGPoint]
    let name: String?
    let dark: Bool
    let reduced: Bool
    let onFinish: @MainActor () -> Void
    let onDone: @MainActor () -> Void

    static let finishAt: Double = 3.3
    static let end: Double = 4.5
    static let reducedFinishAt: Double = 0.8
    static let reducedEnd: Double = 1.1

    @State private var start = Date()
    @State private var fired = false
    @State private var over = false

    // Brand tokens (ALLTERNIT_THEMES in allternit-motion.js).
    private var bg: Color { Color(hex: dark ? "#0F0F0F" : "#FDF8F3") }
    private var block: Color { Color(hex: dark ? "#F0EEE6" : "#141413") }
    private var hot: Color { Color(hex: dark ? "#FFFFFF" : "#7A5D43") }
    private let accent = Color(hex: "#D97757")
    private var accent2: Color { Color(hex: dark ? "#D4B08C" : "#7A5D43") }
    private let grid = Color(hex: "#B08D6E")

    /// The A:// wordmark: (col, row, isCore). Cols 0–4 are the A-block, 6–14 are "://".
    private static let mark: [(Int, Int, Bool)] = [
        (2, 2, true), (2, 0, false), (1, 1, false), (3, 1, false), (0, 2, false), (1, 2, false), (3, 2, false),
        (4, 2, false), (0, 3, false), (4, 3, false), (0, 4, false), (4, 4, false), (6, 1, false), (6, 3, false),
        (8, 3, false), (8, 4, false), (9, 2, false), (10, 0, false), (10, 1, false), (12, 3, false), (12, 4, false),
        (13, 2, false), (14, 0, false), (14, 1, false),
    ]

    private static func cl(_ v: Double) -> Double { max(0, min(1, v)) }
    private static func P(_ t: Double, _ a: Double, _ b: Double) -> Double { cl((t - a) / (b - a)) }
    private static func lerp(_ a: Double, _ b: Double, _ e: Double) -> Double { a + (b - a) * e }
    private static func outExpo(_ x: Double) -> Double { x >= 1 ? 1 : 1 - pow(2, -10 * x) }
    private static func ioExpo(_ x: Double) -> Double {
        x <= 0 ? 0 : x >= 1 ? 1 : x < 0.5 ? pow(2, 20 * x - 10) / 2 : (2 - pow(2, -20 * x + 10)) / 2
    }
    private static func outBack(_ x: Double) -> Double { let c = 1.9; return 1 + (c + 1) * pow(x - 1, 3) + c * pow(x - 1, 2) }
    private static func inCubic(_ x: Double) -> Double { x * x * x }
    private static func ioCubic(_ x: Double) -> Double { x < 0.5 ? 4 * x * x * x : 1 - pow(-2 * x + 2, 3) / 2 }
    /// Deterministic per-block noise (same hash as the brand lib).
    private static func rr(_ i: Int, _ k: Int) -> Double { let x = sin(Double(i) * 127.1 + Double(k) * 311.7) * 43758.5453; return x - floor(x) }

    var body: some View {
        GeometryReader { geo in
            TimelineView(.animation(paused: over)) { tl in
                let t = tl.date.timeIntervalSince(start)
                frame(t: t, size: geo.size)
                    .onChange(of: t >= (reduced ? Self.reducedFinishAt : Self.finishAt)) { _, past in if past { fire() } }
                    .onChange(of: t >= (reduced ? Self.reducedEnd : Self.end)) { _, past in if past { stop() } }
            }
        }
        .ignoresSafeArea()
        // Before finish the overlay catches taps (setup underneath must not
        // get them); after finish it lets touches through to the app.
        .contentShape(Rectangle())
        .onTapGesture { stop() }
        .allowsHitTesting(!fired)
        .accessibilityHidden(true)
        .onAppear { start = Date() }
    }

    private func fire() {
        guard !fired else { return }
        fired = true
        onFinish()
    }

    private func stop() {
        guard !over else { return }
        over = true
        fire()
        onDone()
    }

    @ViewBuilder
    private func frame(t: Double, size: CGSize) -> some View {
        let W = Double(size.width), H = Double(size.height)
        let u = min(44, (W * 0.62) / 15)
        let ox = W / 2 - 7.5 * u
        let my = H / 2 - 1.1 * u
        let oy = my - 2.5 * u
        let s = u * 0.85
        let k = u / 20
        let fontSize = max(13, min(18, u * 0.42))
        let label = name.map { "WELCOME, \($0)" } ?? "WELCOME"

        if reduced {
            // No flight, no particles: the static mark + name, faded in and out.
            let op = Self.P(t, 0, 0.2) * (1 - Self.P(t, Self.reducedFinishAt, Self.reducedEnd))
            Canvas { ctx, _ in
                ctx.fill(Path(CGRect(origin: .zero, size: size)), with: .color(bg))
                for (gx, gy, core) in Self.mark {
                    let r = CGRect(x: ox + (Double(gx) + 0.5) * u - s / 2, y: oy + (Double(gy) + 0.5) * u - s / 2, width: s, height: s)
                    ctx.fill(Path(roundedRect: r, cornerRadius: s * 0.18), with: .color(core ? accent : block))
                }
            }
            .overlay(alignment: .top) { labelText(label, fontSize: fontSize).offset(y: my + 3 * u + 18) }
            .opacity(op)
        } else {
            let typed = Int(Self.P(t, 1.5, 2.05) * Double(label.count))
            Canvas { ctx, _ in
                draw(ctx, t: t, W: W, H: H, u: u, ox: ox, my: my, oy: oy, s: s, k: k)
            }
            .overlay(alignment: .top) {
                VStack(spacing: 10) {
                    HStack(spacing: 2) {
                        labelText(String(label.prefix(typed)), fontSize: fontSize)
                        if t > 1.5 && t < 2.6 && sin(t * 18) > 0 {
                            Rectangle().fill(accent).frame(width: fontSize * 0.6, height: fontSize)
                        }
                    }
                    Rectangle().fill(grid).frame(width: 200 * k * Self.outExpo(Self.P(t, 1.9, 2.3)), height: 2)
                }
                .offset(y: my + 3 * u + 18)
                .opacity(1 - Self.P(t, 2.95, 3.2))
            }
        }
    }

    private func labelText(_ text: String, fontSize: Double) -> some View {
        Text(text)
            .font(.system(size: fontSize, weight: .medium, design: .monospaced))
            .tracking(fontSize * 0.38)
            .foregroundStyle(accent2)
            .lineLimit(1)
    }

    private func draw(_ ctx: GraphicsContext, t: Double, W: Double, H: Double, u: Double,
                      ox: Double, my: Double, oy: Double, s: Double, k: Double) {
        let full = Path(CGRect(x: 0, y: 0, width: W, height: H))
        let bgFade = 1 - Self.P(t, 3.25, 3.7)
        ctx.fill(full, with: .color(bg.opacity(bgFade * Self.ioCubic(Self.P(t, 0.1, 0.7)))))
        // Soft warm glow behind the mark.
        let glow = bgFade * 0.22 * Self.ioCubic(Self.P(t, 0.4, 1.6))
        if glow > 0.001 {
            ctx.fill(full, with: .radialGradient(Gradient(colors: [grid.opacity(0.9 * glow), .clear]),
                                                center: CGPoint(x: ox + 2.5 * u, y: my), startRadius: 0, endRadius: max(W, H) * 0.6))
        }
        let flash = t > 1.1 ? 0.18 * (1 - Self.P(t, 1.1, 1.4)) : 0
        if flash > 0 { ctx.fill(full, with: .color(accent.opacity(flash))) }
        ring(ctx, t, 1.1, 0.6, 260 * k, accent, 4, ox + 2.5 * u, my)
        ring(ctx, t, 1.16, 0.7, 380 * k, accent2, 2, ox + 2.5 * u, my)
        ring(ctx, t, 2.55, 0.5, u * 6, accent, 2, ox + 2.5 * u, my)

        // No real Home mark to land on natively: the A fades as it reaches the top bar.
        let tx0 = W / 2, ty0 = 38.0, tsize = 44.0
        let cell = tsize * 0.2, landSize = tsize * 0.17
        let sweepX = Self.lerp(ox - 120 * k, ox + 15 * u + 120 * k, Self.ioCubic(Self.P(t, 1.45, 2.05)))
        let beat = 1 + 0.2 * max(0, sin((t - 2.55) * 10)) * (t > 2.55 && t < 2.87 ? 1 : 0)
        let outro = 1 - Self.P(t, Self.end - 0.2, Self.end)
        let far = max(520 * k, hypot(W, H) * 0.6)

        for (i, (gx, gy, core)) in Self.mark.enumerated() {
            let tx = ox + (Double(gx) + 0.5) * u
            let ty = oy + (Double(gy) + 0.5) * u
            let d = tx - sweepX + (ty - oy) * 0.8
            let spec = exp(-pow(d, 2) / (2 * 40 * k * 40 * k)) * (t > 1.45 && t < 2.1 ? 1 : 0)
            let inA = gx < 5
            let rip = Double(max(abs(gx - 2), abs(gy - 2)))
            let fp = inA ? Self.ioExpo(Self.P(t, 3.3 + rip * 0.05, 4.25 + rip * 0.05)) : 0
            let fx = tx0 + Double(gx - 2) * cell
            let fy = ty0 + Double(gy - 2) * cell
            let arc = sin(.pi * fp) * -70 * k
            let dz = inA ? 1 : 1 - Self.inCubic(Self.P(t, 2.95 + (Double(14 - gx) / 8) * 0.35, 3.15 + (Double(14 - gx) / 8) * 0.35))
            let tail = 1 - fp

            if core {
                let pop = Self.outBack(Self.P(t, 0.25, 0.6))
                let pre = 1 + 1.6 * (1 - Self.P(t, 0.35, 0.75))
                let mv = Self.ioExpo(Self.P(t, 0.6, 0.95))
                let x = Self.lerp(Self.lerp(W / 2, tx, mv), fx, fp) + arc
                let y = Self.lerp(Self.lerp(my, ty, mv), fy, fp)
                let sz = Self.lerp(s * pop * pre * beat, landSize, fp)
                guard sz > 0 else { continue }
                square(ctx, x, y, sz, color: accent, alpha: tail * outro, rotation: 0)
                continue
            }
            let fromNode = i - 1 < sources.count
            let src = fromNode ? (Double(sources[i - 1].x), Double(sources[i - 1].y))
                : (tx + cos(Self.rr(i, 2) * 6.28) * far, ty + sin(Self.rr(i, 2) * 6.28) * far)
            let ts = 0.15 + (i < 9 ? 0 : 0.45) + hypot(Double(gx - 2), Double(gy - 2)) * 0.04 + Self.rr(i, 1) * 0.06
            // tr = 2,1: short accent trails; tr = 0: the block itself.
            for tr in stride(from: 2, through: 0, by: -1) {
                let p = Self.P(t - Double(tr) * 0.03, ts, ts + 0.6)
                if p <= 0 { continue }
                if tr > 0 && (p >= 1 || fp > 0) { continue }
                let e = Self.outExpo(p)
                let land = 1 - Self.P(t, ts + 0.15, ts + 0.5)
                var x = Self.lerp(src.0, tx, e)
                var y = Self.lerp(src.1, ty, e)
                var sz = s * Self.lerp(fromNode ? 1.6 : 0.4, 1, e)
                if tr == 0 && fp > 0 { x = Self.lerp(tx, fx, fp) + arc; y = Self.lerp(ty, fy, fp); sz = Self.lerp(s, landSize, fp) }
                sz *= dz
                if sz < 0.5 { continue }
                let lit = min(1, land * 0.6 + spec * 0.9)
                let color = tr > 0 ? accent2 : mix(block, hot, lit)
                let alpha = (tr > 0 ? 0.22 * (1 - Double(tr) / 3) : tail) * outro
                square(ctx, x, y, sz, color: color, alpha: alpha, rotation: (1 - e) * (Self.rr(i, 4) - 0.5) * 240)
            }
        }
    }

    private func square(_ ctx: GraphicsContext, _ x: Double, _ y: Double, _ sz: Double, color: Color, alpha: Double, rotation: Double) {
        guard alpha > 0.005 else { return }
        var c = ctx
        c.translateBy(x: x, y: y)
        if rotation != 0 { c.rotate(by: .degrees(rotation)) }
        c.fill(Path(roundedRect: CGRect(x: -sz / 2, y: -sz / 2, width: sz, height: sz), cornerRadius: max(1, sz * 0.18)),
               with: .color(color.opacity(alpha)))
    }

    private func ring(_ ctx: GraphicsContext, _ t: Double, _ st: Double, _ dur: Double, _ R: Double,
                      _ color: Color, _ width: Double, _ cx: Double, _ cy: Double) {
        let p = Self.P(t, st, st + dur)
        guard p > 0, p < 1 else { return }
        let r = R * Self.outExpo(p)
        ctx.stroke(Path(ellipseIn: CGRect(x: cx - r, y: cy - r, width: r * 2, height: r * 2)),
                   with: .color(color.opacity(1 - p)), lineWidth: width)
    }

    private func mix(_ a: Color, _ b: Color, _ p: Double) -> Color {
        let ua = UIColor(a), ub = UIColor(b)
        var (r1, g1, b1, a1): (CGFloat, CGFloat, CGFloat, CGFloat) = (0, 0, 0, 0)
        var (r2, g2, b2, a2): (CGFloat, CGFloat, CGFloat, CGFloat) = (0, 0, 0, 0)
        ua.getRed(&r1, green: &g1, blue: &b1, alpha: &a1)
        ub.getRed(&r2, green: &g2, blue: &b2, alpha: &a2)
        let q = CGFloat(p)
        return Color(red: r1 + (r2 - r1) * q, green: g1 + (g2 - g1) * q, blue: b1 + (b2 - b1) * q)
    }
}
