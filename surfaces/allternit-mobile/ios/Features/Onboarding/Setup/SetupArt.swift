import SwiftUI
import UIKit

/// Visual building blocks of the first-run setup, ported from allternit-ai
/// `setup/SetupArt.tsx` + `setup.css`: the setup palette (white/neutral in
/// light, graphite in dark — never tan), the A:// block mark, the block
/// field, the 30-block progress meter, the result cards and the haptic block
/// ripple. Coral (#D97757) is only the A:// core, the leading progress
/// block, the flowing packets and agent emphasis.

// MARK: - Palette (setup.css `--as-*` tokens)

enum SetupPalette {
    private static func dyn(_ light: String, _ dark: String) -> Color {
        Color(UIColor { $0.userInterfaceStyle == .dark ? UIColor(Color(hex: dark)) : UIColor(Color(hex: light)) })
    }
    private static func dynA(_ light: (String, CGFloat), _ dark: (String, CGFloat)) -> Color {
        Color(UIColor { t in
            let (hex, a) = t.userInterfaceStyle == .dark ? dark : light
            return UIColor(Color(hex: hex)).withAlphaComponent(a)
        })
    }

    static let ink = dyn("#141413", "#F0EEE6")
    static let sub = dyn("#5E5E66", "#A1A1AA")
    static let mut = dyn("#8A8A93", "#7C7C86")
    static let line = dyn("#E4E4E7", "#2E2E33")
    static let lineDash = dyn("#D4D4D8", "#3A3A3F")
    static let fill = dyn("#F4F4F5", "#1F1F22")
    static let fill2 = dyn("#EDEDEF", "#29292D")
    static let bg = dyn("#FFFFFF", "#141413")
    static let card = dyn("#FFFFFF", "#1A1A1A")
    static let cardGlass = dynA(("#FFFFFF", 0.92), ("#1A1A1A", 0.92))
    static let coral = Color(hex: "#D97757")
    static let ok = Color(hex: "#22A06B")
    static let okBg = dynA(("#E8F5EE", 1), ("#22A06B", 0.16))
    static let okFg = dyn("#17663F", "#5FD39B")
    static let warnBg = dynA(("#FDF0EB", 1), ("#D97757", 0.16))
    static let warnFg = dyn("#9A4426", "#F0A285")
    static let errBg = dynA(("#FDECEC", 1), ("#DC4646", 0.16))
    static let errFg = dyn("#A12A2A", "#F19A9A")
    static let term = dyn("#0F0F0F", "#0A0A0A")
    static let termFg = Color(hex: "#D4D4D8")
    static let field = dynA(("#141413", 0.05), ("#F0EEE6", 0.06))
    static let edge = dyn("#D4D4D8", "#3A3A3F")
    static let coralHalo = Color(hex: "#D97757").opacity(0.16)

    static func mono(_ size: CGFloat, _ weight: Font.Weight = .medium) -> Font {
        .system(size: size, weight: weight, design: .monospaced)
    }
}

/// The ease the prototype uses for cards (cubic-bezier(.2,.8,.2,1)).
extension Animation {
    static var setupCard: Animation { .timingCurve(0.2, 0.8, 0.2, 1, duration: 0.45) }
    static var setupFade: Animation { .easeOut(duration: 0.4) }
    static var setupPop: Animation { .spring(response: 0.45, dampingFraction: 0.68) }
}

/// Fade + rise on appear (the `as-fade` / `as-card` keyframes). Static under Reduce Motion.
struct SetupRise: ViewModifier {
    var distance: CGFloat = 6
    var delay: Double = 0
    var animation: Animation = .setupFade
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var shown = false

    func body(content: Content) -> some View {
        content
            .opacity(shown || reduceMotion ? 1 : 0)
            .offset(y: shown || reduceMotion ? 0 : distance)
            .onAppear {
                guard !reduceMotion else { return }
                withAnimation(animation.delay(delay)) { shown = true }
            }
    }
}

extension View {
    func setupRise(_ distance: CGFloat = 6, delay: Double = 0, animation: Animation = .setupFade) -> some View {
        modifier(SetupRise(distance: distance, delay: delay, animation: animation))
    }
}

// MARK: - A:// block mark (web AllternitOrb)

enum AllternitOrbState: String, Sendable {
    case thinking, searching, working, writing, speaking, idle
}

/// The A-block mark: eleven ink blocks forming the "A" around a coral core.
/// Each state is a choreography of the same blocks (allternit-orb.css):
/// thinking breathes, searching pings with a radar sweep, working runs the
/// outline like a conveyor, writing emits and recalls in stroke order.
struct AllternitOrbView: View {
    var state: AllternitOrbState = .thinking
    var size: CGFloat = 16
    var label: String?

    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private struct Block { let x: Int, y: Int, ripple: Int, outline: Int, stroke: Int }
    private static let blocks: [Block] = [
        Block(x: 2, y: 0, ripple: 2, outline: 4, stroke: 4),
        Block(x: 1, y: 1, ripple: 1, outline: 3, stroke: 3),
        Block(x: 3, y: 1, ripple: 1, outline: 5, stroke: 5),
        Block(x: 0, y: 2, ripple: 2, outline: 2, stroke: 2),
        Block(x: 1, y: 2, ripple: 1, outline: 10, stroke: 9),
        Block(x: 3, y: 2, ripple: 1, outline: 9, stroke: 10),
        Block(x: 4, y: 2, ripple: 2, outline: 6, stroke: 6),
        Block(x: 0, y: 3, ripple: 2, outline: 1, stroke: 1),
        Block(x: 4, y: 3, ripple: 2, outline: 7, stroke: 7),
        Block(x: 0, y: 4, ripple: 2, outline: 0, stroke: 0),
        Block(x: 4, y: 4, ripple: 2, outline: 8, stroke: 8),
    ]
    private static let byOutline = blocks.sorted { $0.outline < $1.outline }

    var body: some View {
        Group {
            if reduceMotion || state == .idle {
                Canvas { ctx, sz in draw(ctx, sz, t: nil) }
            } else {
                TimelineView(.animation) { tl in
                    Canvas { ctx, sz in draw(ctx, sz, t: tl.date.timeIntervalSinceReferenceDate) }
                }
            }
        }
        .frame(width: size, height: size)
        .accessibilityElement()
        .accessibilityLabel(label ?? "")
        .accessibilityHidden(label == nil)
    }

    /// One block transform: offset (cells), rotation, scale, opacity.
    private struct Pose { var dx = 0.0, dy = 0.0, rot = 0.0, scale = 1.0, alpha = 1.0 }

    private func draw(_ ctx: GraphicsContext, _ sz: CGSize, t: Double?) {
        let unit = Double(sz.width) / 50
        let ink = SetupPalette.ink
        for b in Self.blocks {
            let p = t.map { pose(b, t: $0) } ?? Pose(alpha: reduceMotion && state != .idle ? 0.55 : 0.88)
            rect(ctx, cx: Double(b.x) * 10 + 5, cy: Double(b.y) * 10 + 5, pose: p, unit: unit, color: ink)
        }
        let core = t.map { corePose($0) } ?? Pose()
        rect(ctx, cx: 25, cy: 25, pose: core, unit: unit, color: SetupPalette.coral)
    }

    private func rect(_ ctx: GraphicsContext, cx: Double, cy: Double, pose: Pose, unit: Double, color: Color) {
        let side = 8.5 * pose.scale * unit
        guard side > 0.1, pose.alpha > 0.01 else { return }
        var c = ctx
        c.translateBy(x: (cx + pose.dx) * unit, y: (cy + pose.dy) * unit)
        c.rotate(by: .degrees(pose.rot))
        let r = CGRect(x: -side / 2, y: -side / 2, width: side, height: side)
        c.fill(Path(roundedRect: r, cornerRadius: 1.5 * unit * pose.scale), with: .color(color.opacity(pose.alpha)))
    }

    /// Keyframe interpolation over [(t, value)] with the given progress 0…1.
    private func kf(_ p: Double, _ frames: [(Double, Double)]) -> Double {
        guard let first = frames.first else { return 0 }
        if p <= first.0 { return first.1 }
        for i in 1..<frames.count where p <= frames[i].0 {
            let (t0, v0) = frames[i - 1], (t1, v1) = frames[i]
            let x = (p - t0) / max(0.0001, t1 - t0)
            let e = x < 0.5 ? 4 * x * x * x : 1 - pow(-2 * x + 2, 3) / 2
            return v0 + (v1 - v0) * e
        }
        return frames.last!.1
    }

    private func phase(_ t: Double, period: Double, delay: Double) -> Double {
        let v = (t - delay).truncatingRemainder(dividingBy: period)
        return (v < 0 ? v + period : v) / period
    }

    private func pose(_ b: Block, t: Double) -> Pose {
        let dx = Double(b.x - 2), dy = Double(b.y - 2)
        switch state {
        case .thinking:
            let p = phase(t, period: 2.4, delay: Double(b.ripple) * 0.14)
            let out = kf(p, [(0.1, 0), (0.42, 3.5), (0.64, -1), (0.82, 0)])
            return Pose(dx: dx * out, dy: dy * out,
                        rot: kf(p, [(0.1, 0), (0.42, 90), (0.82, 90)]),
                        scale: kf(p, [(0.1, 1), (0.42, 0.66), (0.64, 0.94), (0.82, 1)]),
                        alpha: kf(p, [(0.1, 0.9), (0.42, 0.4), (0.64, 1), (0.82, 0.9)]))
        case .searching:
            let sweep = (atan2(dx, -dy) / (2 * .pi) + 1).truncatingRemainder(dividingBy: 1)
            let p = phase(t, period: 1.6, delay: sweep * 1.6)
            let out = kf(p, [(0, 0), (0.09, 2.8), (0.45, 0)])
            return Pose(dx: dx * out, dy: dy * out, scale: kf(p, [(0, 1), (0.09, 1.18), (0.45, 1)]),
                        alpha: kf(p, [(0, 0.3), (0.09, 1), (0.45, 0.3)]))
        case .working:
            let n = Self.byOutline[(b.outline + 1) % Self.byOutline.count]
            let nx = Double(n.x - b.x), ny = Double(n.y - b.y)
            let p = phase(t, period: 0.62, delay: 0)
            let d = kf(p, [(0, 0), (0.3, 5), (0.62, 10)])
            return Pose(dx: nx * d, dy: ny * d, scale: kf(p, [(0, 1), (0.3, 0.72), (0.62, 1)]), alpha: 0.92)
        case .writing:
            let p = phase(t, period: 2.8, delay: Double(b.stroke) * 0.105)
            let d = kf(p, [(0, -10), (0.13, 0.9), (0.2, 0), (0.64, 0), (0.84, -10)])
            return Pose(dx: dx * d, dy: dy * d,
                        scale: kf(p, [(0, 0.2), (0.13, 1.1), (0.2, 1), (0.64, 1), (0.84, 0.2)]),
                        alpha: kf(p, [(0, 0), (0.13, 1), (0.2, 0.95), (0.64, 0.95), (0.84, 0)]))
        case .speaking:
            let p = phase(t, period: 1.8, delay: Double(b.ripple) * 0.09)
            let d = kf(p, [(0, -2.4), (0.5, -3.2), (1, -2.4)])
            return Pose(dx: dx * d, dy: dy * d, scale: kf(p, [(0, 0.78), (0.5, 0.84), (1, 0.78)]), alpha: 0.95)
        case .idle:
            return Pose(alpha: 0.88)
        }
    }

    private func corePose(_ t: Double) -> Pose {
        switch state {
        case .thinking:
            let p = phase(t, period: 2.4, delay: 0)
            return Pose(rot: kf(p, [(0.1, 0), (0.42, 45), (0.64, 90), (0.82, 90)]),
                        scale: kf(p, [(0.1, 1), (0.42, 1.22), (0.64, 0.82), (0.82, 1)]))
        case .searching:
            let p = phase(t, period: 1.6, delay: 0)
            return Pose(rot: p * 360, scale: kf(p, [(0, 1.1), (0.5, 0.88), (1, 1.1)]))
        case .working:
            let p = phase(t, period: 0.62, delay: 0)
            return Pose(rot: kf(p, [(0, 0), (0.3, 45), (0.62, 90)]), scale: kf(p, [(0, 1), (0.3, 0.86), (0.62, 1)]))
        case .writing:
            let p = phase(t, period: 1.4, delay: 0)
            return Pose(scale: kf(p, [(0, 0.9), (0.3, 1.16), (1, 0.9)]), alpha: kf(p, [(0, 0.8), (0.3, 1), (1, 0.8)]))
        case .speaking:
            let p = phase(t, period: 1.8, delay: 0)
            return Pose(scale: kf(p, [(0, 1), (0.5, 1.1), (1, 1)]))
        case .idle:
            return Pose()
        }
    }
}

// MARK: - Gizzi

/// Gizzi's face for the setup duo (the GizziMascot asset). The web mascot's
/// emotion shows as a small idle motion: curious tilts, pleased bobs,
/// focused holds still. Static under Reduce Motion.
struct SetupGizzi: View {
    enum Emotion { case curious, pleased, focused }
    var size: CGFloat
    var emotion: Emotion = .curious

    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        Group {
            if reduceMotion || emotion == .focused {
                face(0)
            } else {
                TimelineView(.animation(minimumInterval: 1.0 / 30)) { tl in
                    face(tl.date.timeIntervalSinceReferenceDate)
                }
            }
        }
        .frame(width: size, height: size)
        .accessibilityLabel("Gizzi")
    }

    private func face(_ t: Double) -> some View {
        let s = sin(t * 2.2)
        return Image("GizziMascot")
            .resizable()
            .interpolation(.high)
            .scaledToFit()
            .rotationEffect(.degrees(emotion == .curious ? s * 5 : 0))
            .offset(y: emotion == .pleased ? CGFloat(s) * size * 0.04 : 0)
    }
}

// MARK: - Block field

/// The quiet grid of rounded squares behind the graph (22pt cells, breathing).
struct SetupBlockField: View {
    var opacity: Double = 1
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var breathe = false

    var body: some View {
        Canvas { ctx, sz in
            let cell: CGFloat = 22
            var path = Path()
            var y: CGFloat = 0
            while y < sz.height {
                var x: CGFloat = 0
                while x < sz.width {
                    path.addRoundedRect(in: CGRect(x: x + 2, y: y + 2, width: 18, height: 18),
                                        cornerSize: CGSize(width: 3.2, height: 3.2))
                    x += cell
                }
                y += cell
            }
            ctx.fill(path, with: .color(SetupPalette.field))
        }
        .opacity(opacity * (reduceMotion ? 1 : (breathe ? 1 : 0.75)))
        .onAppear {
            guard !reduceMotion else { return }
            withAnimation(.easeInOut(duration: 3).repeatForever(autoreverses: true)) { breathe = true }
        }
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }
}

// MARK: - Progress

/// The 30-block progress meter (phone: its own full-width row).
struct SetupProgressBlocks: View {
    var step: SetupStep
    var order: [SetupStep]
    var height: CGFloat = 5

    var body: some View {
        let blocks = progressBlocks(step, order: order)
        let lit = blocks.filter { $0 != .off }.count
        HStack(spacing: 2) {
            ForEach(blocks.indices, id: \.self) { i in
                RoundedRectangle(cornerRadius: 1.5)
                    .fill(color(blocks[i]))
                    .frame(maxWidth: .infinity)
                    .frame(height: height)
            }
        }
        .animation(.easeOut(duration: 0.3), value: lit)
        .accessibilityElement()
        .accessibilityLabel("Setup progress: \(lit) of \(blocks.count)")
    }

    private func color(_ b: ProgressBlock) -> Color {
        switch b {
        case .off: return SetupPalette.fill2
        case .on: return SetupPalette.ink
        case .lead: return SetupPalette.coral
        }
    }
}

// MARK: - Chips + result cards

struct SetupChip: View {
    var text: String
    var tone: ChipTone

    var body: some View {
        Text(text.uppercased())
            .font(SetupPalette.mono(9.5, .semibold))
            .tracking(0.6)
            .lineLimit(1)
            .padding(.horizontal, 7)
            .padding(.vertical, 3)
            .background(bg, in: RoundedRectangle(cornerRadius: 5))
            .foregroundStyle(fg)
            .fixedSize()
    }

    private var bg: Color {
        switch tone {
        case .ok: return SetupPalette.okBg
        case .warn: return SetupPalette.warnBg
        case .soft: return SetupPalette.fill
        case .error: return SetupPalette.errBg
        }
    }
    private var fg: Color {
        switch tone {
        case .ok: return SetupPalette.okFg
        case .warn: return SetupPalette.warnFg
        case .soft: return SetupPalette.sub
        case .error: return SetupPalette.errFg
        }
    }
}

/// Collapsed log row for a finished step: `▸ Where it runs  MacBook  PAIRED`.
struct SetupDoneRow: View {
    var card: ResultCard

    var body: some View {
        HStack(spacing: 8) {
            AllternitOrbView(state: .idle, size: 14)
            Text(card.title).font(SetupPalette.mono(11, .semibold)).foregroundStyle(SetupPalette.sub).lineLimit(1).fixedSize()
            Text(card.summary).font(.system(size: 12, weight: .medium)).foregroundStyle(SetupPalette.ink).lineLimit(1).truncationMode(.tail)
            Spacer(minLength: 4)
            SetupChip(text: card.chip, tone: card.tone)
        }
        .padding(.horizontal, 10)
        .frame(height: 34)
        .background(SetupPalette.fill.opacity(0.55), in: RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).stroke(SetupPalette.line))
        .setupRise(6, animation: .easeOut(duration: 0.35))
        .accessibilityElement(children: .combine)
    }
}

/// Gizzi's output for the current choice — a mono log that types itself out.
struct SetupResultCardView: View {
    var card: ResultCard
    var working: Bool
    var lineDelayBase: Double = 0.15

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                AllternitOrbView(state: working ? .working : (card.done ? .idle : .working), size: 16)
                (Text("A:// ").foregroundStyle(SetupPalette.ink) + Text("· \(card.title)").foregroundStyle(SetupPalette.mut))
                    .font(SetupPalette.mono(11, .semibold))
                    .lineLimit(1)
                Spacer(minLength: 4)
                SetupChip(text: card.chip, tone: card.tone)
            }
            .padding(.horizontal, 10)
            .frame(height: 34)
            .background(SetupPalette.fill.opacity(0.55))
            .overlay(alignment: .bottom) { SetupPalette.line.frame(height: 1) }

            VStack(alignment: .leading, spacing: 3) {
                ForEach(Array(card.lines.enumerated()), id: \.offset) { i, line in
                    SetupLogLine(text: line, done: card.done && i == card.lines.count - 1,
                                 delay: lineDelayBase + Double(i) * 0.32)
                        .id("\(i)-\(line)")
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 12)
            .padding(.vertical, 10)
            .background(SetupPalette.term)

            HStack(spacing: 16) {
                Text(card.kvKey).foregroundStyle(SetupPalette.mut)
                Text(card.kvValue).fontWeight(.semibold).foregroundStyle(SetupPalette.ink).lineLimit(1)
                Spacer(minLength: 0)
            }
            .font(.system(size: 11.5))
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .overlay(alignment: .top) { SetupPalette.line.frame(height: 1) }
        }
        .background(SetupPalette.card)
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(SetupPalette.line))
        .shadow(color: .black.opacity(0.05), radius: 7, y: 4)
        .setupRise(8, animation: .setupCard)
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(.updatesFrequently)
    }
}

/// One `▸ line` of the log, sliding in after its stagger delay.
struct SetupLogLine: View {
    var text: String
    var done: Bool
    var delay: Double
    var single: Bool = false

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var shown = false

    var body: some View {
        (Text("▸ ").foregroundStyle(SetupPalette.coral)
            + Text(text).foregroundStyle(SetupPalette.termFg)
            + Text(done ? "  ✓" : "").foregroundStyle(Color(hex: "#4ADE80")))
            .font(SetupPalette.mono(11))
            .lineLimit(single ? 1 : nil)
            .truncationMode(.tail)
            .fixedSize(horizontal: false, vertical: !single)
            .opacity(shown || reduceMotion ? 1 : 0)
            .offset(x: shown || reduceMotion ? 0 : -4)
            .onAppear {
                guard !reduceMotion else { return }
                withAnimation(.easeOut(duration: 0.3).delay(delay)) { shown = true }
            }
    }
}

// MARK: - Buttons

struct SetupPrimaryButtonStyle: ButtonStyle {
    @Environment(\.isEnabled) private var enabled
    var height: CGFloat = 44

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 14, weight: .semibold))
            .foregroundStyle(SetupPalette.bg)
            .padding(.horizontal, 18)
            .frame(minHeight: height)
            .frame(maxWidth: .infinity)
            .background(SetupPalette.ink, in: RoundedRectangle(cornerRadius: 10))
            .opacity(enabled ? 1 : 0.35)
            .scaleEffect(configuration.isPressed ? 0.985 : 1)
            .animation(.easeOut(duration: 0.1), value: configuration.isPressed)
    }
}

struct SetupGhostButtonStyle: ButtonStyle {
    @Environment(\.isEnabled) private var enabled

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 13, weight: .semibold))
            .foregroundStyle(configuration.isPressed ? SetupPalette.ink : SetupPalette.sub)
            .padding(.horizontal, 14)
            .frame(minHeight: 44)
            .background(configuration.isPressed ? SetupPalette.fill : .clear, in: RoundedRectangle(cornerRadius: 10))
            .opacity(enabled ? 1 : 0.4)
    }
}

/// The small ink button inside connect panels (`.as-inline-btn`).
struct SetupInlineButtonStyle: ButtonStyle {
    @Environment(\.isEnabled) private var enabled

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 12.5, weight: .semibold))
            .foregroundStyle(SetupPalette.bg)
            .padding(.horizontal, 14)
            .frame(minHeight: 36)
            .background(SetupPalette.ink, in: RoundedRectangle(cornerRadius: 9))
            .opacity(enabled ? (configuration.isPressed ? 0.85 : 1) : 0.4)
    }
}

/// A connect panel under the result card (`.as-panel`).
struct SetupPanel<Content: View>: View {
    var title: String
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title.uppercased())
                .font(SetupPalette.mono(10.5, .semibold))
                .tracking(0.8)
                .foregroundStyle(SetupPalette.mut)
            content
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(12)
        .background(SetupPalette.card, in: RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(SetupPalette.line))
        .setupRise(6, animation: .easeOut(duration: 0.35))
    }
}

struct SetupNote: View {
    var text: String
    var body: some View {
        Text(text)
            .font(.system(size: 12.5))
            .lineSpacing(2)
            .foregroundStyle(SetupPalette.sub)
            .fixedSize(horizontal: false, vertical: true)
    }
}

// MARK: - Haptic ripple

/// Block squares that ripple out from the chosen tile, keyed by the ripple
/// counter. Each square peaks at .13 opacity, 700ms, with a 45ms Chebyshev
/// ring delay. Hidden under Reduce Motion.
struct SetupHapticRipple: View {
    var rippleKey: Int
    var origin: RippleOrigin?
    var cell: CGFloat

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var start: Date?

    var body: some View {
        GeometryReader { geo in
            if let start, let origin, !reduceMotion {
                TimelineView(.animation) { tl in
                    let t = tl.date.timeIntervalSince(start)
                    Canvas { ctx, sz in
                        let cols = max(1, Int(ceil(sz.width / cell)))
                        let rows = max(1, Int(ceil(sz.height / cell)))
                        for (i, c) in rippleCells(cols: cols, rows: rows, origin: origin).enumerated() {
                            let p = (t - c.delay) / 0.7
                            guard p > 0, p < 1 else { continue }
                            let alpha = p < 0.35 ? 0.13 * p / 0.35 : 0.13 * (1 - (p - 0.35) / 0.65)
                            let scale = p < 0.35 ? 0.3 + 0.7 * p / 0.35 : 1 - 0.2 * (p - 0.35) / 0.65
                            let col = i % cols, row = i / cols
                            let side = (cell - 10) * scale
                            let r = CGRect(x: CGFloat(col) * cell + cell / 2 - side / 2,
                                           y: CGFloat(row) * cell + cell / 2 - side / 2, width: side, height: side)
                            ctx.fill(Path(roundedRect: r, cornerRadius: 5),
                                     with: .color((c.origin ? SetupPalette.coral : SetupPalette.ink).opacity(alpha)))
                        }
                    }
                    .frame(width: geo.size.width, height: geo.size.height)
                }
            }
        }
        .allowsHitTesting(false)
        .accessibilityHidden(true)
        .onChange(of: rippleKey) { _, key in
            guard key > 0, origin != nil else { return }
            let mine = Date()
            start = mine
            DispatchQueue.main.asyncAfter(deadline: .now() + 1.8) {
                if start == mine { start = nil }
            }
        }
    }
}
