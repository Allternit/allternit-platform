import SwiftUI

/// A grey badge that pops to one solid colour when earned (PLAN-v2 step 3.7).
/// Ported from handoffs/ui-polish-2026-11-24/reference/BadgePop.reference.tsx:
/// 120 x 120 geometry; on earn the colour wipes in from the left (0.4 s), the
/// shape settles .92 -> 1.04 -> 1, the mark draws or scales in, a glow ring
/// fades out; 0.8 s in all, once. Un-earn reverses in 0.25 s without a ring.
/// A badge that mounts already earned shows the end frame. Reduce Motion
/// switches straight to the earned or grey frame.
struct BadgePop: View {
    enum Shape { case seal, hex, flower }
    enum Tone { case success, accent }

    var shape: Shape = .seal
    var earned: Bool
    var tone: Tone = .success
    var size: CGFloat = 40
    /// Accessibility label for the earned state; the other state reads "Not yet".
    var earnedLabel = "Earned"
    var notEarnedLabel = "Not yet"

    @ReducedMotion private var reduceMotion
    /// When the last change happened, and which way.
    @State private var changedAt: Date?
    @State private var wasEarned: Bool?

    var body: some View {
        Group {
            if reduceMotion || changedAt == nil {
                canvas(t: nil)
            } else {
                TimelineView(.animation) { tl in
                    canvas(t: tl.date.timeIntervalSince(changedAt ?? tl.date))
                }
            }
        }
        .frame(width: size, height: size)
        .onChange(of: earned) { _, _ in changedAt = Date() }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(earned ? earnedLabel : notEarnedLabel)
        .accessibilityAddTraits(.isImage)
    }

    // MARK: Colours

    private var onColor: Color { tone == .success ? Theme.statusSuccess : Color("AccentPrimary") }
    private var markColor: Color { tone == .success ? .white : Color("BgPrimary") }
    private var grey: Color { Color("TextTertiary").opacity(0.35) }

    // MARK: Geometry

    private static let seal: Path = {
        var p = Path()
        let steps = 96
        for i in 0...steps {
            let a = Double(i) / Double(steps) * .pi * 2
            let r = 46 + 6 * cos(12 * a)
            let q = CGPoint(x: 60 + r * cos(a), y: 60 + r * sin(a))
            if i == 0 { p.move(to: q) } else { p.addLine(to: q) }
        }
        p.closeSubpath()
        return p
    }()

    private static let hex: Path = {
        var p = Path()
        p.move(to: CGPoint(x: 60, y: 8)); p.addLine(to: CGPoint(x: 105, y: 34)); p.addLine(to: CGPoint(x: 105, y: 86))
        p.addLine(to: CGPoint(x: 60, y: 112)); p.addLine(to: CGPoint(x: 15, y: 86)); p.addLine(to: CGPoint(x: 15, y: 34))
        p.closeSubpath()
        return p
    }()

    private static let flower: Path = {
        var p = Path()
        for deg in stride(from: 0.0, to: 360, by: 60) {
            let a = deg * .pi / 180
            let cx = 60 + 30 * cos(a), cy = 60 + 30 * sin(a)
            p.addEllipse(in: CGRect(x: cx - 24, y: cy - 24, width: 48, height: 48))
        }
        p.addEllipse(in: CGRect(x: 34, y: 34, width: 52, height: 52))
        return p
    }()

    private static let check: Path = {
        var p = Path()
        p.move(to: CGPoint(x: 42, y: 62)); p.addLine(to: CGPoint(x: 55, y: 75)); p.addLine(to: CGPoint(x: 79, y: 48))
        return p
    }()

    private static let bolt: Path = {
        var p = Path()
        let pts: [(Double, Double)] = [(66, 34), (46, 64), (62, 64), (54, 88), (76, 56), (60, 56)]
        p.move(to: CGPoint(x: pts[0].0, y: pts[0].1))
        for q in pts.dropFirst() { p.addLine(to: CGPoint(x: q.0, y: q.1)) }
        p.closeSubpath()
        return p
    }()

    private static let star: Path = {
        var p = Path()
        let pts: [(Double, Double)] = [(60, 38), (66.5, 53), (82, 54.6), (70.3, 65), (74, 80.5), (60, 72.4),
                                       (46, 80.5), (49.7, 65), (38, 54.6), (53.5, 53)]
        p.move(to: CGPoint(x: pts[0].0, y: pts[0].1))
        for q in pts.dropFirst() { p.addLine(to: CGPoint(x: q.0, y: q.1)) }
        p.closeSubpath()
        return p
    }()

    // MARK: Easing (CSS cubic-bezier)

    private static func bezier(_ x1: Double, _ y1: Double, _ x2: Double, _ y2: Double, _ x: Double) -> Double {
        if x <= 0 { return 0 }
        if x >= 1 { return 1 }
        func b(_ a: Double, _ c: Double, _ t: Double) -> Double {
            let u = 1 - t
            return 3 * u * u * t * a + 3 * u * t * t * c + t * t * t
        }
        var lo = 0.0, hi = 1.0
        for _ in 0..<32 {
            let mid = (lo + hi) / 2
            if b(x1, x2, mid) < x { lo = mid } else { hi = mid }
        }
        return b(y1, y2, (lo + hi) / 2)
    }
    private static func out(_ x: Double) -> Double { bezier(0.16, 1, 0.3, 1, x) }
    private static func inOut(_ x: Double) -> Double { bezier(0.65, 0, 0.35, 1, x) }
    /// Progress of `t` through [start, start + dur], 0...1.
    private static func seg(_ t: Double, _ start: Double, _ dur: Double) -> Double {
        max(0, min(1, (t - start) / dur))
    }

    // MARK: Frame

    private struct Frame {
        var wipe = 1.0        // coloured layer width, 0...1
        var scale = 1.0       // shape scale
        var draw = 1.0        // check trim / bolt+star scale
        var markOpacity = 1.0
        var ring: (scale: Double, opacity: Double, width: Double)?
    }

    /// `t` is seconds since the last change; nil = settled frame.
    private func frame(_ t: Double?) -> Frame {
        guard let t else {
            return Frame(wipe: earned ? 1 : 0, scale: 1, draw: 1, markOpacity: earned ? 1 : 0, ring: nil)
        }
        if earned {
            let wipe = Self.out(Self.seg(t, 0, 0.4))
            // .92 -> 1.04 at 0.28 -> 1 at 0.50, ease out per segment.
            let scale: Double
            if t < 0.28 { scale = 0.92 + (1.04 - 0.92) * Self.out(t / 0.28) }
            else { scale = 1.04 + (1 - 1.04) * Self.out(Self.seg(t, 0.28, 0.22)) }
            let draw = shape == .seal ? Self.out(Self.seg(t, 0.15, 0.4)) : Self.out(Self.seg(t, 0.2, 0.3))
            let r = Self.out(Self.seg(t, 0.1, 0.7))
            let ring = t < 0.8 ? (0.85 + 0.4 * r, 0.5 * (1 - r), 3 - 2 * r) : nil
            return Frame(wipe: wipe, scale: scale, draw: draw, markOpacity: 1, ring: ring)
        }
        let back = Self.inOut(Self.seg(t, 0, 0.25))
        return Frame(wipe: 1 - back, scale: 1, draw: 1, markOpacity: 1 - back, ring: nil)
    }

    private func canvas(t: Double?) -> some View {
        Canvas { ctx, sz in
            var c = ctx
            let k = Double(min(sz.width, sz.height)) / 120
            c.scaleBy(x: k, y: k)
            let f = frame(t)
            let center = CGPoint(x: 60, y: 60)

            if let ring = f.ring {
                var r = c
                r.opacity = ring.opacity
                r.translateBy(x: 60, y: 60); r.scaleBy(x: ring.scale, y: ring.scale); r.translateBy(x: -60, y: -60)
                r.stroke(Path(ellipseIn: CGRect(x: 10, y: 10, width: 100, height: 100)),
                         with: .color(onColor), lineWidth: ring.width)
            }

            var s = c
            s.translateBy(x: center.x, y: center.y)
            s.scaleBy(x: f.scale, y: f.scale)
            s.translateBy(x: -center.x, y: -center.y)

            let outline: Path
            switch shape {
            case .seal: outline = Self.seal
            case .hex: outline = Self.hex
            case .flower: outline = Self.flower
            }
            let hexStroke = StrokeStyle(lineWidth: 8, lineCap: .round, lineJoin: .round)

            // Grey layer.
            s.fill(outline, with: .color(grey))
            if shape == .hex { s.stroke(outline, with: .color(grey), style: hexStroke) }

            // Coloured layer, wiped in from the left.
            if f.wipe > 0 {
                var w = s
                w.clip(to: Path(CGRect(x: -20, y: -20, width: 20 + 120 * f.wipe + (f.wipe >= 1 ? 20 : 0), height: 160)))
                w.fill(outline, with: .color(onColor))
                if shape == .hex { w.stroke(outline, with: .color(onColor), style: hexStroke) }
            }

            // Mark.
            guard f.markOpacity > 0 else { return }
            var m = s
            m.opacity = f.markOpacity
            switch shape {
            case .seal:
                if f.draw > 0 {
                    m.stroke(Self.check.trimmedPath(from: 0, to: f.draw), with: .color(markColor),
                             style: StrokeStyle(lineWidth: 7, lineCap: .round, lineJoin: .round))
                }
            case .hex, .flower:
                let mark = shape == .hex ? Self.bolt : Self.star
                let o = shape == .hex ? CGPoint(x: 61, y: 61) : CGPoint(x: 60, y: 59.25)
                if f.draw > 0 {
                    m.translateBy(x: o.x, y: o.y); m.scaleBy(x: f.draw, y: f.draw); m.translateBy(x: -o.x, y: -o.y)
                    m.fill(mark, with: .color(markColor))
                    m.stroke(mark, with: .color(markColor), style: StrokeStyle(lineWidth: 3, lineJoin: .round))
                }
            }
        }
    }
}

#Preview {
    struct Demo: View {
        @State private var on = false
        var body: some View {
            VStack(spacing: 20) {
                HStack(spacing: 16) {
                    BadgePop(shape: .seal, earned: on, size: 56)
                    BadgePop(shape: .hex, earned: on, tone: .accent, size: 56)
                    BadgePop(shape: .flower, earned: on, size: 56)
                }
                Button(on ? "Reset" : "Earn") { on.toggle() }
            }
            .padding()
            .background(Color("BgPrimary"))
        }
    }
    return Demo()
}
