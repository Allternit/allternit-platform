import SwiftUI

/// The four empty-state drawings (PLAN-v2 step 1.4). Ported exactly from
/// handoffs/ui-polish-2026-11-24/reference/EmptyArt.reference.tsx: same
/// 96 x 96 geometry, 1.5 stroke, same 3.4 s loop and per-segment easing.
/// One colour (TextPrimary); the "paper" fill hides what a layer sits over.
/// Reduce Motion draws the still frame and nothing moves.
enum EmptyArtKind {
    /// Containers: projects, files, artifacts.
    case sheets
    /// Caught up: nothing waits.
    case bell
    /// People, chats, messages.
    case envelope
    /// Money, usage, credits.
    case coins
}

struct EmptyArt: View {
    let kind: EmptyArtKind
    var size: CGFloat = 96
    /// Fill for solid shapes. Pass the colour the art sits on.
    var paper: Color = Color("BgPrimary")

    @ReducedMotion private var reduceMotion

    var body: some View {
        Group {
            if reduceMotion {
                canvas(t: nil)
            } else {
                TimelineView(.animation) { tl in
                    canvas(t: tl.date.timeIntervalSinceReferenceDate)
                }
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }

    private func canvas(t: Double?) -> some View {
        Canvas { ctx, sz in
            var c = ctx
            let k = Double(min(sz.width, sz.height)) / 96
            c.scaleBy(x: k, y: k)
            let art = EmptyArtPainter(ink: Color("TextPrimary"), paper: paper, t: t)
            switch kind {
            case .sheets: art.sheets(c)
            case .bell: art.bell(c)
            case .envelope: art.envelope(c)
            case .coins: art.coins(c)
            }
        }
    }
}

// MARK: - Easing and keyframes

/// CSS cubic-bezier easing. A keyframe's ease shapes the segment that starts
/// at that keyframe; the CSS default (`ease`) is used where none is named.
private struct Ease {
    let x1: Double, y1: Double, x2: Double, y2: Double

    static let out = Ease(x1: 0.16, y1: 1, x2: 0.3, y2: 1)
    static let `in` = Ease(x1: 0.7, y1: 0, x2: 0.84, y2: 0)
    static let io = Ease(x1: 0.65, y1: 0, x2: 0.35, y2: 1)
    static let std = Ease(x1: 0.25, y1: 0.1, x2: 0.25, y2: 1)

    func apply(_ x: Double) -> Double {
        if x <= 0 { return 0 }
        if x >= 1 { return 1 }
        func bez(_ a: Double, _ b: Double, _ t: Double) -> Double {
            let u = 1 - t
            return 3 * u * u * t * a + 3 * u * t * t * b + t * t * t
        }
        var lo = 0.0, hi = 1.0
        for _ in 0..<32 {
            let mid = (lo + hi) / 2
            if bez(x1, x2, mid) < x { lo = mid } else { hi = mid }
        }
        return bez(y1, y2, (lo + hi) / 2)
    }
}

private typealias Key = (p: Double, v: Double, e: Ease)

/// Value of a keyframe track at `p` percent of the loop.
private func track(_ p: Double, _ keys: [Key]) -> Double {
    guard let first = keys.first else { return 0 }
    if p <= first.p { return first.v }
    for i in 1..<keys.count {
        let a = keys[i - 1], b = keys[i]
        if p <= b.p {
            let span = b.p - a.p
            let x = span <= 0 ? 1 : (p - a.p) / span
            return a.v + (b.v - a.v) * a.e.apply(x)
        }
    }
    return keys[keys.count - 1].v
}

// MARK: - Painter

private struct EmptyArtPainter {
    let ink: Color
    let paper: Color
    /// Seconds (any clock), or nil for the still frame.
    let t: Double?

    private static let loop = 3.4
    private var p: Double? { t.map { ($0.truncatingRemainder(dividingBy: Self.loop) / Self.loop) * 100 } }

    /// Track value, or `still` for the still (reduced-motion) frame.
    private func v(_ keys: [Key], still: Double) -> Double {
        guard let p else { return still }
        return track(p, keys)
    }

    private static let sw: CGFloat = 1.5
    private var style: StrokeStyle { StrokeStyle(lineWidth: Self.sw, lineCap: .round, lineJoin: .round) }

    // MARK: Drawing helpers

    private func stroke(_ c: GraphicsContext, _ path: Path, dash: [CGFloat] = []) {
        var s = style
        s.dash = dash
        c.stroke(path, with: .color(ink), style: s)
    }

    /// Paper fill, then stroke (an SVG element with class ea-paper).
    private func solid(_ c: GraphicsContext, _ path: Path) {
        c.fill(path, with: .color(paper))
        stroke(c, path)
    }

    /// One CSS transform layer: translate, rotate, scale about `origin`.
    private func layer(_ c: GraphicsContext, tx: Double = 0, ty: Double = 0, rot: Double = 0,
                       sx: Double = 1, sy: Double = 1, origin: CGPoint = .zero, opacity: Double = 1,
                       _ draw: (GraphicsContext) -> Void) {
        var l = c
        l.opacity = c.opacity * max(0, min(1, opacity))
        l.translateBy(x: origin.x, y: origin.y)
        l.translateBy(x: tx, y: ty)
        if rot != 0 { l.rotate(by: .degrees(rot)) }
        l.scaleBy(x: sx, y: sy)
        l.translateBy(x: -origin.x, y: -origin.y)
        draw(l)
    }

    private func build(_ f: (inout Path) -> Void) -> Path {
        var path = Path()
        f(&path)
        return path
    }

    private func pt(_ x: Double, _ y: Double) -> CGPoint { CGPoint(x: x, y: y) }

    /// Elliptical arc as a polyline (SVG angles: y down, increasing = clockwise).
    private func arc(_ path: inout Path, cx: Double, cy: Double, rx: Double, ry: Double, from a0: Double, to a1: Double, move: Bool) {
        let n = 48
        for i in 0...n {
            let a = (a0 + (a1 - a0) * Double(i) / Double(n)) * .pi / 180
            let q = pt(cx + rx * cos(a), cy + ry * sin(a))
            if i == 0 && move { path.move(to: q) } else { path.addLine(to: q) }
        }
    }

    private func ellipse(_ cx: Double, _ cy: Double, _ rx: Double, _ ry: Double) -> Path {
        Path(ellipseIn: CGRect(x: cx - rx, y: cy - ry, width: rx * 2, height: ry * 2))
    }

    /// Iso slab at height y: top diamond and two side faces dropping 5.
    private func slab(_ c: GraphicsContext, _ y: Double) {
        let top = build {
            $0.move(to: pt(48, y - 10)); $0.addLine(to: pt(72, y)); $0.addLine(to: pt(48, y + 10)); $0.addLine(to: pt(24, y)); $0.closeSubpath()
        }
        let left = build {
            $0.move(to: pt(24, y)); $0.addLine(to: pt(24, y + 5)); $0.addLine(to: pt(48, y + 15)); $0.addLine(to: pt(48, y + 10))
        }
        let right = build {
            $0.move(to: pt(72, y)); $0.addLine(to: pt(72, y + 5)); $0.addLine(to: pt(48, y + 15))
        }
        solid(c, top)
        stroke(c, left)
        stroke(c, right)
    }

    private func segs(_ c: GraphicsContext, _ list: [(Double, Double, Double, Double)]) {
        let path = build { for s in list { $0.move(to: pt(s.0, s.1)); $0.addLine(to: pt(s.2, s.3)) } }
        stroke(c, path)
    }

    // MARK: 1. Sheets

    func sheets(_ c: GraphicsContext) {
        let o = Ease.out, i = Ease.in, io = Ease.io, d = Ease.std
        let ghostY = v([(0, -6, o), (29.4, -6, o), (41.2, -14, i), (52, 16, d), (57.9, 14, io), (64.7, 16, io), (100, -6, d)], still: -6)
        let stackY = v([(0, 0, d), (52, 0, d), (57.9, 2, io), (64.7, 0, d), (100, 0, d)], still: 0)
        let impactO = v([(0, 0, d), (52, 0, d), (57.9, 1, o), (64.7, 0, d), (100, 0, d)], still: 0)
        let impactS = v([(0, 1, d), (52, 1, d), (57.9, 1.12, o), (64.7, 1.12, d), (100, 1.12, d)], still: 1)

        layer(c, ty: stackY) { c in
            slab(c, 70); slab(c, 62); slab(c, 54)
        }
        segs(c, [(18, 76, 23, 79), (74, 79, 79, 76)])
        layer(c, sx: impactS, sy: impactS, origin: pt(48, 62), opacity: impactO) { c in
            segs(c, [(20, 56, 15, 54), (20, 62, 14, 62), (20, 68, 15, 70),
                     (76, 56, 81, 54), (76, 62, 82, 62), (76, 68, 81, 70)])
        }
        layer(c, ty: ghostY) { c in
            let g = 38.0
            let top = build {
                $0.move(to: pt(48, g - 10)); $0.addLine(to: pt(72, g)); $0.addLine(to: pt(48, g + 10)); $0.addLine(to: pt(24, g)); $0.closeSubpath()
            }
            stroke(c, top, dash: [3, 3])
            segs(c, [(42, 35, 54, 41), (54, 35, 42, 41)])
        }
    }

    // MARK: 2. Bell

    func bell(_ c: GraphicsContext) {
        let o = Ease.out, i = Ease.in, io = Ease.io, d = Ease.std
        let domeY = v([(0, 0, i), (14.7, 0, i), (22, 3, o), (36.8, 0, d), (100, 0, d)], still: 0)
        let rippleO = v([(0, 0.6, d), (14.7, 0.6, d), (36.8, 0, o), (100, 0, d)], still: 0)
        let rippleS = v([(0, 1, d), (14.7, 1, d), (36.8, 1.45, o), (100, 1.45, d)], still: 1)
        let raysS = v([(0, 1, d), (14.7, 1, d), (29, 1.18, io), (44.1, 1, d), (100, 1, d)], still: 1)
        let raysO = v([(0, 0.9, d), (14.7, 0.9, d), (29, 1, io), (44.1, 0.9, d), (100, 0.9, d)], still: 1)
        let checkO = v([(0, 0, o), (51.5, 0, o), (57.4, 1, i), (75, 1, i), (88.2, 0, d), (100, 0, d)], still: 1)
        let checkY = v([(0, 4, o), (51.5, 4, o), (57.4, 0, i), (75, 0, i), (88.2, -3, d), (100, -3, d)], still: 0)

        solid(c, ellipse(48, 70, 26, 8))
        layer(c, sx: rippleS, sy: rippleS, origin: pt(48, 70), opacity: rippleO) { c in
            stroke(c, ellipse(48, 70, 26, 8))
        }
        layer(c, ty: domeY) { c in
            let dome = build {
                arc(&$0, cx: 48, cy: 66, rx: 20, ry: 20, from: 180, to: 360, move: true)
                $0.closeSubpath()
            }
            solid(c, dome)
            // Highlight: r 12 arc from (36,56) to (44,47), clockwise, about (47.76, 58.4).
            let hl = build { arc(&$0, cx: 47.76, cy: 58.4, rx: 12, ry: 12, from: 191.5, to: 251.8, move: true) }
            stroke(c, hl)
            segs(c, [(48, 46, 48, 42)])
            solid(c, ellipse(48, 40, 3, 3))
        }
        layer(c, sx: raysS, sy: raysS, origin: pt(48, 40), opacity: raysO) { c in
            segs(c, [(48, 26, 48, 20), (34, 30, 30, 25), (62, 30, 66, 25), (24, 40, 18, 37), (72, 40, 78, 37)])
        }
        layer(c, ty: checkY, opacity: checkO) { c in
            solid(c, ellipse(48, 10, 7, 7))
            let check = build { $0.move(to: pt(44.5, 10)); $0.addLine(to: pt(47, 12.5)); $0.addLine(to: pt(52, 7.5)) }
            stroke(c, check)
        }
    }

    // MARK: 3. Envelope

    func envelope(_ c: GraphicsContext) {
        let o = Ease.out, i = Ease.in, io = Ease.io, d = Ease.std
        let flap = v([(0, 1, o), (22, 1, o), (36.8, -1, io), (85.3, -1, io), (97, 1, d), (100, 1, d)], still: -1)
        let bubO = v([(0, 0, o), (22, 0, o), (39.7, 1, i), (85.3, 1, i), (94.1, 0, d), (100, 0, d)], still: 1)
        let bubY = v([(0, 10, o), (22, 10, o), (39.7, 0, i), (85.3, 0, i), (94.1, 0, d), (100, 0, d)], still: 0)
        let tilt = v([(0, 0, d), (58.8, 0, d), (69, -4, io), (79.4, 3, io), (100, 0, d)], still: 0)
        let s1x = v([(0, 0, d), (58.8, 0, d), (69, 2, d), (100, 0, d)], still: 0)
        let s1y = v([(0, 0, d), (58.8, 0, d), (69, -4, d), (100, 0, d)], still: 0)
        let sO = v([(0, 0.4, d), (58.8, 0.4, d), (69, 1, d), (100, 0.4, d)], still: 1)

        // Tray: two slabs under translate(12 19) scale(0.75).
        var tray = c
        tray.translateBy(x: 12, y: 19)
        tray.scaleBy(x: 0.75, y: 0.75)
        slab(tray, 76); slab(tray, 70)

        layer(c, rot: tilt, origin: pt(48, 70)) { c in
            solid(c, build { $0.addPath(Path(roundedRect: CGRect(x: 22, y: 40, width: 52, height: 30), cornerRadius: 3)) })
            stroke(c, build { $0.move(to: pt(22, 43)); $0.addLine(to: pt(48, 60)); $0.addLine(to: pt(74, 43)) })
            layer(c, sy: flap, origin: pt(48, 40)) { c in
                solid(c, build { $0.move(to: pt(22, 40)); $0.addLine(to: pt(48, 58)); $0.addLine(to: pt(74, 40)); $0.closeSubpath() })
            }
        }
        layer(c, ty: bubY, opacity: bubO) { c in
            solid(c, build { $0.addPath(Path(roundedRect: CGRect(x: 34, y: 14, width: 28, height: 14), cornerRadius: 7)) })
            for (n, x) in [42.0, 48.0, 54.0].enumerated() {
                let dy = dotY(delay: 0.12 * Double(n))
                layer(c, ty: dy) { c in
                    c.fill(ellipse(x, 21, 1.6, 1.6), with: .color(ink))
                }
            }
        }
        layer(c, tx: s1x, ty: s1y, opacity: sO) { c in
            segs(c, [(80, 27, 80, 33), (77, 30, 83, 30)])
        }
        layer(c, tx: -s1x, ty: -s1y, opacity: sO) { c in
            segs(c, [(14, 47, 14, 53), (11, 50, 17, 50)])
        }
    }

    /// Typing dots: 0.5 s period, ease, bob up 2.5.
    private func dotY(delay: Double) -> Double {
        guard let t else { return 0 }
        var x = ((t - delay) / 0.5).truncatingRemainder(dividingBy: 1)
        if x < 0 { x += 1 }
        return track(x * 100, [(0, 0, .std), (50, -2.5, .std), (100, 0, .std)])
    }

    // MARK: 4. Coins

    private func coinEdge(_ cx: Double, _ cy: Double, _ rx: Double, _ ry: Double) -> Path {
        build {
            $0.move(to: pt(cx - rx, cy))
            $0.addLine(to: pt(cx - rx, cy + 5))
            arc(&$0, cx: cx, cy: cy + 5, rx: rx, ry: ry, from: 180, to: 0, move: false)
            $0.addLine(to: pt(cx + rx, cy))
        }
    }

    func coins(_ c: GraphicsContext) {
        let o = Ease.out, i = Ease.in, d = Ease.std
        let flipY = v([(0, 0, o), (14.7, 0, o), (25, -20, d), (35.3, -20, i), (55.9, 0, d), (100, 0, d)], still: 0)
        let flipS = v([(0, 1, o), (14.7, 1, o), (25, 0.05, d), (35.3, -1, i), (55.9, 1, d), (100, 1, d)], still: 1)
        let stackY = v([(0, 0, d), (55.9, 0, d), (61, 2, o), (64.7, -1, d), (67.6, 0, d), (100, 0, d)], still: 0)
        let lean = v([(0, -18, d), (55.9, -18, d), (61, -14, o), (67.6, -18, d), (100, -18, d)], still: -18)
        let spX = v([(0, 0, d), (33, 4, d), (66, -2, d), (100, 0, d)], still: 0)
        let spY = v([(0, 0, d), (33, 4, d), (66, 6, d), (100, 0, d)], still: 0)
        let spR = v([(0, 0, d), (33, 30, d), (66, 60, d), (100, 90, d)], still: 0)
        let spO = v([(0, 0.5, d), (33, 1, d), (66, 0.6, d), (100, 0.5, d)], still: 1)

        layer(c, rot: lean, origin: pt(74, 70)) { c in
            stroke(c, coinEdge(72, 66, 13, 5))
            solid(c, ellipse(72, 66, 13, 5))
        }
        layer(c, ty: stackY) { c in
            for cy in [68.0, 62.0] {
                solid(c, coinEdge(40, cy, 16, 6))
                solid(c, ellipse(40, cy, 16, 6))
            }
        }
        // The flipping coin turns about its own centre (fill box y 50...67).
        layer(c, ty: flipY, sy: flipS, origin: pt(40, 58.5)) { c in
            solid(c, coinEdge(40, 56, 16, 6))
            solid(c, ellipse(40, 56, 16, 6))
            segs(c, [(37, 56, 43, 56)])
        }
        layer(c, tx: spX, ty: spY, rot: spR, origin: pt(80, 26), opacity: spO) { c in
            stroke(c, build {
                $0.move(to: pt(80, 22)); $0.addLine(to: pt(83, 26)); $0.addLine(to: pt(80, 30)); $0.addLine(to: pt(77, 26)); $0.closeSubpath()
            })
        }
    }
}

/// Gizzi slot for agent and terminal empty states (style E). Draws the real
/// Gizzi mascot in a 96 x 96 box, centred, at 72, walking in place: a step
/// hop and a small rock, pacing side to side and turning at each end. The
/// app ships one mascot image (no pixel-pet walk row), so the walk is that
/// image in motion. Reduce Motion shows the idle frame.
struct GizziWalk: View {
    var size: CGFloat = 72
    @ReducedMotion private var reduceMotion

    var body: some View {
        Group {
            if reduceMotion {
                mascot(x: 0, hop: 0, rock: 0, facing: 1)
            } else {
                TimelineView(.animation) { tl in
                    let t = tl.date.timeIntervalSinceReferenceDate
                    // One lap = 6 s: 3 s right, 3 s left, eased at the turns.
                    let lap = t.truncatingRemainder(dividingBy: 6) / 6
                    let leg = lap < 0.5 ? lap * 2 : (lap - 0.5) * 2
                    let eased = 0.5 - 0.5 * cos(leg * .pi)
                    let right = lap < 0.5
                    let x = (right ? eased : 1 - eased) * 16 - 8
                    let step = t * 2 * .pi * 2.4
                    mascot(x: x, hop: -abs(sin(step)) * 3.5, rock: sin(step) * 4, facing: right ? 1 : -1)
                }
            }
        }
        .frame(width: 96, height: 96)
        .accessibilityLabel("Gizzi")
    }

    private func mascot(x: Double, hop: Double, rock: Double, facing: Double) -> some View {
        Image("GizziMascot")
            .resizable()
            .interpolation(.high)
            .scaledToFit()
            .frame(width: size, height: size)
            .scaleEffect(x: facing, y: 1)
            .rotationEffect(.degrees(rock))
            .offset(x: x, y: hop)
    }
}

#Preview {
    HStack(spacing: 16) {
        EmptyArt(kind: .sheets)
        EmptyArt(kind: .bell)
        EmptyArt(kind: .envelope)
        EmptyArt(kind: .coins)
    }
    .padding()
    .background(Color("BgPrimary"))
}
