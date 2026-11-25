import SwiftUI

/// The empty-state art (PLAN-v2 step 1.4; empty-art-v2 contract). Ported
/// exactly from handoffs/ui-polish-2026-11-24/reference/EmptyArt.reference.tsx
/// and empty-art-v2/spec.json: same 96 x 96 geometry, 1.5 stroke, same 3.4 s
/// loop and per-segment easing. One colour (TextPrimary); the "paper" fill
/// hides what a layer sits over. Reduce Motion draws the still frame and
/// nothing moves. The art is decorative: hidden from VoiceOver.
enum EmptyArtKind: CaseIterable {
    /// Containers: projects, files, artifacts.
    case sheets
    /// Caught up: nothing waits.
    case bell
    /// People, chats, messages.
    case envelope
    /// Money, usage, credits.
    case coins
    /// "No X match / found" after a search or filter (Gizzi scene).
    case search
    /// Linked phones, texts, phone apps.
    case phone
    /// Logs, events, steps, traces, activity, history, versions, deploys.
    case log
    /// API keys, SSH keys, tokens, secrets, permissions.
    case key
    /// Apps, skills, connectors, plugins, MCP, extensions, mini apps.
    case apps
    /// Memory, remembered notes, brains.
    case memory
    /// Models, providers, on-device models (Gizzi scene).
    case model
    /// Charts, usage graphs, analytics (S-ios-2).
    case chart
    /// People, contacts, teams (S-ios-2).
    case people
    /// Archive, trash, history kept (S-ios-2).
    case archive
    /// Flows, workflows, iterations (S-ios-2).
    case flow
    /// Network, connections, devices (S-ios-2).
    case network
    /// Calendar, schedules, events (S-ios-2).
    case calendar
    /// The real Gizzi mascot walking (agent and terminal screens).
    case gizzi
    /// The Connect sheet scene, fixed at `asleep` (no status call).
    case connectScene

    /// Scenes are wider than tall: width = round(size * 1.67).
    var isScene: Bool {
        switch self {
        case .search, .model, .connectScene: true
        default: false
        }
    }
}

/// The three treatments (CONTRACT.md): a whole pane is full, a block in a
/// list, card or dropdown is panel, a single line is inline.
enum EmptyArtSize {
    case full, panel, inline

    var points: CGFloat {
        switch self {
        case .full: 96
        case .panel: 64
        case .inline: 36
        }
    }
}

struct EmptyArt: View {
    let kind: EmptyArtKind
    var size: CGFloat = 96
    /// Fill for solid shapes. Pass the colour the art sits on (a filled
    /// card); the default is the page background.
    var paper: Color = Color("BgPrimary")

    /// Scene width over height (search, model, connect scene).
    static let sceneRatio: CGFloat = 1.67

    /// Width the art takes at `size`: round(size * 1.67) for scenes.
    static func width(_ kind: EmptyArtKind, size: CGFloat) -> CGFloat {
        kind.isScene ? (size * sceneRatio).rounded() : size
    }

    var body: some View {
        content
            .frame(width: Self.width(kind, size: size), height: size)
            .accessibilityHidden(true)
    }

    @ViewBuilder private var content: some View {
        switch kind {
        case .gizzi:
            GizziWalk(size: (size * 0.75).rounded(), box: size)
        case .connectScene:
            ConnectScene(state: .asleep)
        case .search, .model:
            EmptyArtGizziScene(kind: kind, size: size, paper: paper)
        default:
            EmptyArtDrawing(kind: kind, paper: paper)
        }
    }
}

extension EmptyArt {
    /// `EmptyArt(kind: .log, size: .panel)`: full 96, panel 64, inline 36.
    init(kind: EmptyArtKind, size: EmptyArtSize, paper: Color = Color("BgPrimary")) {
        self.init(kind: kind, size: size.points, paper: paper)
    }
}

/// One line drawing (viewBox 96) filling its frame: the Canvas and the loop
/// clock. Reduce Motion paints the still frame once.
private struct EmptyArtDrawing: View {
    let kind: EmptyArtKind
    let paper: Color

    @ReducedMotion private var reduceMotion

    var body: some View {
        if reduceMotion {
            canvas(t: nil)
        } else {
            TimelineView(.animation) { tl in
                canvas(t: tl.date.timeIntervalSinceReferenceDate)
            }
        }
    }

    private func canvas(t: Double?) -> some View {
        Canvas { ctx, sz in
            var c = ctx
            let k = Double(min(sz.width, sz.height)) / 96
            c.scaleBy(x: k, y: k)
            EmptyArtPainter(ink: Color("TextPrimary"), paper: paper, t: t).draw(kind, c)
        }
    }
}

/// search / model: Gizzi beside the drawing. viewBox 120 x 72 fitted to
/// round(size * 1.67) x size; Gizzi slot (2, 14) 40 x 50 with the mascot at 36,
/// centred; the drawing nested at (42, -3) 78 x 78. Native Gizzi shows the
/// emotion only (no attention target). SetupGizzi holds still under Reduce
/// Motion.
private struct EmptyArtGizziScene: View {
    let kind: EmptyArtKind
    let size: CGFloat
    let paper: Color

    var body: some View {
        let w = EmptyArt.width(kind, size: size)
        let k = min(w / 120, size / 72)
        let dx = (w - 120 * k) / 2, dy = (size - 72 * k) / 2
        ZStack(alignment: .topLeading) {
            SetupGizzi(size: 36 * k, emotion: kind == .model ? .focused : .curious)
                .position(x: dx + 22 * k, y: dy + 39 * k)
            EmptyArtDrawing(kind: kind, paper: paper)
                .frame(width: 78 * k, height: 78 * k)
                .position(x: dx + 81 * k, y: dy + 36 * k)
        }
        .frame(width: w, height: size)
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
    static let lin = Ease(x1: 0, y1: 0, x2: 1, y2: 1)

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

    /// Track value, or `still` for the still (reduced-motion) frame. `delay`
    /// is the CSS animation-delay (a phase shift of the loop); `dur` its
    /// duration.
    private func v(_ keys: [Key], still: Double, delay: Double = 0, dur: Double = EmptyArtPainter.loop) -> Double {
        guard let t else { return still }
        var x = ((t - delay) / dur).truncatingRemainder(dividingBy: 1)
        if x < 0 { x += 1 }
        return track(x * 100, keys)
    }

    /// Paints `kind` in the 96 x 96 box.
    func draw(_ kind: EmptyArtKind, _ c: GraphicsContext) {
        switch kind {
        case .sheets: sheets(c)
        case .bell: bell(c)
        case .envelope: envelope(c)
        case .coins: coins(c)
        case .search: search(c)
        case .phone: phone(c)
        case .log: log(c)
        case .key: key(c)
        case .apps: apps(c)
        case .memory: memory(c)
        case .model: model(c)
        // S-ios-2: port kinds 12-17 from spec.json (chart, people, archive,
        // flow, network, calendar). Until then each draws a stand-in.
        case .chart: coins(c) // S-ios-2
        case .people: envelope(c) // S-ios-2
        case .archive: sheets(c) // S-ios-2
        case .flow: log(c) // S-ios-2
        case .network: apps(c) // S-ios-2
        case .calendar: bell(c) // S-ios-2
        // Not line drawings: EmptyArt renders these as views.
        case .gizzi, .connectScene: break
        }
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
    private func solid(_ c: GraphicsContext, _ path: Path, dash: [CGFloat] = []) {
        c.fill(path, with: .color(paper))
        stroke(c, path, dash: dash)
    }

    /// Ink fill, no stroke (class ea-dot).
    private func dot(_ c: GraphicsContext, _ path: Path) {
        c.fill(path, with: .color(ink))
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

    // MARK: SVG shapes (v2 drawings), in SVG's own start point and direction
    // so dash patterns land where the reference draws them.

    /// SVG path data, copied verbatim from spec.json.
    private func svg(_ d: String) -> Path { SVGPathData.parse(d) }

    /// <circle>: starts at (cx + r, cy), clockwise.
    private func circle(_ cx: Double, _ cy: Double, _ r: Double) -> Path { oval(cx, cy, r, r) }

    /// <ellipse>: starts at (cx + rx, cy), clockwise.
    private func oval(_ cx: Double, _ cy: Double, _ rx: Double, _ ry: Double) -> Path {
        build {
            let quarter: [(Double, Double)] = [(cx, cy + ry), (cx - rx, cy), (cx, cy - ry), (cx + rx, cy)]
            var from = pt(cx + rx, cy)
            $0.move(to: from)
            for q in quarter {
                let to = pt(q.0, q.1)
                SVGPathData.arc(&$0, from: from, rx: rx, ry: ry, phi: 0, large: false, sweep: true, to: to)
                from = to
            }
            $0.closeSubpath()
        }
    }

    /// <rect>: starts at (x + rx, y), clockwise; ry defaults to rx (SVG).
    private func rect(_ x: Double, _ y: Double, _ w: Double, _ h: Double, rx: Double = 0, ry: Double? = nil) -> Path {
        let rx = min(rx, w / 2), ry = min(ry ?? rx, h / 2)
        return build {
            guard rx > 0, ry > 0 else {
                $0.addRect(CGRect(x: x, y: y, width: w, height: h))
                return
            }
            // M x+rx,y H x+w-rx A ... V y+h-ry A ... H x+rx A ... V y+ry A ... Z
            let sides: [(CGPoint, CGPoint)] = [
                (pt(x + w - rx, y), pt(x + w, y + ry)),
                (pt(x + w, y + h - ry), pt(x + w - rx, y + h)),
                (pt(x + rx, y + h), pt(x, y + h - ry)),
                (pt(x, y + ry), pt(x + rx, y)),
            ]
            $0.move(to: pt(x + rx, y))
            for (lineEnd, arcEnd) in sides {
                $0.addLine(to: lineEnd)
                SVGPathData.arc(&$0, from: lineEnd, rx: rx, ry: ry, phi: 0, large: false, sweep: true, to: arcEnd)
            }
            $0.closeSubpath()
        }
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

    // v2 drawings (spec.json), one function per kind, same pattern:
    // - one `v([(pct, value, ease)], still:, delay:)` track per animated
    //   property of a node (CSS keyframes that name the property; a keyframe's
    //   ease shapes the segment after it, nil = the animation's easing; a
    //   missing 0% / 100% uses the still value);
    // - the tree in document order: `solid` (ea-paper), `stroke`, `dot`
    //   (ea-dot, bobbing via `dotY`); shapes keep spec.json's attributes
    //   (`svg(d)`, `circle`, `oval`, `rect`);
    // - an animated node is a `layer` (translate, rotate, scale about the
    //   origin, opacity). The origin is the fill-box centre unless the node's
    //   still frame names a view-box transform-origin.

    // MARK: 5. Search

    /// every "No X match / found" after a search or filter (Gizzi scene)
    /// Loop: glass sweeps right 0.3-1.3, settles; "?" badge 1.5-2.95; glass back 2.6-3.4.
    /// Still frame: .es-scan opacity:0; .es-q opacity:1.
    func search(_ c: GraphicsContext) {
        let o = Ease.out, i = Ease.in, io = Ease.io, d = Ease.std
        // .es-glass  (es-glass 3.4s)
        let glassX = v([(0, 0, io), (8.8, 0, io), (38.2, 14, o), (44.1, 12, io), (76.5, 12, io), (100, 0, d)], still: 0)
        let glassY = v([(0, 0, io), (8.8, 0, io), (38.2, -2, o), (44.1, -1, io), (76.5, -1, io), (100, 0, d)], still: 0)
        // .es-scan  (es-scan 3.4s)
        let scanY = v([(0, -5, d), (8.8, -5, d), (26.5, 5, d), (38.2, -5, d), (44.1, -5, d), (100, -5, d)], still: 0)
        let scanO = v([(0, 0, d), (8.8, 0, d), (14.7, 1, d), (38.2, 1, d), (44.1, 0, d), (100, 0, d)], still: 0)
        // .es-q  (es-q 3.4s)
        let qY = v([(0, 4, o), (44.1, 4, o), (54.4, 0, i), (76.5, 0, i), (86.8, -3, d), (100, -3, d)], still: 0)
        let qO = v([(0, 0, o), (44.1, 0, o), (54.4, 1, i), (76.5, 1, i), (86.8, 0, d), (100, 0, d)], still: 1)

        solid(c, svg("M48 60 L72 70 L48 80 L24 70 Z"))
        stroke(c, svg("M24 70 V75 L48 85 V80"))
        stroke(c, svg("M72 70 V75 L48 85"))
        layer(c, opacity: 0.5) { c in
            stroke(c, svg("M40 66 l10 5 M44 64 l10 5"))
        }
        layer(c, tx: glassX, ty: glassY) { c in
            solid(c, svg("M49.2 52.8 L56.5 60.1 a2.2 2.2 0 0 0 3.1 -3.1 L52.3 49.7 Z"))
            solid(c, circle(40, 42, 13))
            stroke(c, svg("M33 38 a8 8 0 0 1 5 -5"))
            layer(c, ty: scanY, opacity: scanO) { c in
                stroke(c, svg("M31 44 h18"), dash: [2, 3])
            }
        }
        layer(c, ty: qY, opacity: qO) { c in
            solid(c, circle(75, 18, 8))
            stroke(c, svg("M72.4 15.6 a2.7 2.7 0 1 1 3.7 2.5 c-.8 .3 -1.1 .9 -1.1 1.6 v.4"))
            layer(c, ty: dotY(delay: 0)) { c in
                dot(c, circle(75, 22.6, 0.9))
            }
        }
    }

    // MARK: 6. Phone

    /// linked phones, texts, phone apps
    /// Loop: waves pulse out; two dashed (empty) bubbles rise; phone tilts 2.0-2.7 like the envelope.
    /// Still frame: .ep-w1 opacity:0.9; .ep-w2 opacity:0.9; .ep-w3 opacity:0.9.
    func phone(_ c: GraphicsContext) {
        let o = Ease.out, i = Ease.in, io = Ease.io, d = Ease.std
        // .ep-body  (ep-body 3.4s)
        let bodyR = v([(0, 0, d), (58.8, 0, d), (69.1, -4, io), (79.4, 3, io), (88.2, 0, d), (100, 0, d)], still: 0)
        // .ep-b.ep-b1  (ep-b 3.4s)
        let b1Y = v([(0, 4, o), (22.1, 4, o), (38.2, 0, i), (76.5, 0, i), (86.8, 0, d), (100, 0, d)], still: 0)
        let b1O = v([(0, 0, o), (22.1, 0, o), (38.2, 1, i), (76.5, 1, i), (86.8, 0, d), (100, 0, d)], still: 1)
        // .ep-b.ep-b2  (ep-b 3.4s, delay 0.25s)
        let b2Y = v([(0, 4, o), (22.1, 4, o), (38.2, 0, i), (76.5, 0, i), (86.8, 0, d), (100, 0, d)], still: 0, delay: 0.25)
        let b2O = v([(0, 0, o), (22.1, 0, o), (38.2, 1, i), (76.5, 1, i), (86.8, 0, d), (100, 0, d)], still: 1, delay: 0.25)
        // .ep-w.ep-w1  (ep-w 3.4s)
        let w1O = v([(0, 0.15, io), (8.8, 1, io), (26.5, 0.15, io), (100, 0.15, io)], still: 0.9)
        // .ep-w.ep-w2  (ep-w 3.4s, delay 0.2s)
        let w2O = v([(0, 0.15, io), (8.8, 1, io), (26.5, 0.15, io), (100, 0.15, io)], still: 0.9, delay: 0.2)
        // .ep-w.ep-w3  (ep-w 3.4s, delay 0.4s)
        let w3O = v([(0, 0.15, io), (8.8, 1, io), (26.5, 0.15, io), (100, 0.15, io)], still: 0.9, delay: 0.4)

        solid(c, oval(49, 66, 18, 4.5))
        layer(c, rot: bodyR, origin: pt(49, 64)) { c in
            solid(c, rect(36, 14, 26, 50, rx: 5))
            stroke(c, rect(39, 22, 20, 34, rx: 2))
            stroke(c, svg("M45 18 h8 M44 60 h10"))
            layer(c, ty: b1Y, opacity: b1O) { c in
                stroke(c, rect(41.5, 27, 13, 7, rx: 3.5), dash: [2, 2])
            }
            layer(c, ty: b2Y, opacity: b2O) { c in
                stroke(c, rect(43.5, 38, 13, 7, rx: 3.5), dash: [2, 2])
            }
        }
        layer(c, opacity: w1O) { c in
            stroke(c, svg("M66 21 a6 6 0 0 1 5 5"))
        }
        layer(c, opacity: w2O) { c in
            stroke(c, svg("M67 14 a12 12 0 0 1 11 11"))
        }
        layer(c, opacity: w3O) { c in
            stroke(c, svg("M68 7 a18 18 0 0 1 17 17"))
        }
    }

    // MARK: 7. Log

    /// logs, events, steps, traces, activity, history, versions, deploys
    /// Loop: a dot runs down the line 0.3-2.3; each node pops and its dashed row lights as the dot passes.
    /// Still frame: .el-r1 opacity:0.6; .el-r2 opacity:0.6; .el-r3 opacity:0.6; .el-dot opacity:0.
    func log(_ c: GraphicsContext) {
        let o = Ease.out, io = Ease.io, d = Ease.std, l = Ease.lin
        // .el-r.el-r1  (el-r 3.4s, delay 0.57s)
        let r1O = v([(0, 0.35, d), (5.3, 1, d), (29.4, 0.35, d), (100, 0.35, d)], still: 0.6, delay: 0.57)
        // .el-r.el-r2  (el-r 3.4s, delay 1.23s)
        let r2O = v([(0, 0.35, d), (5.3, 1, d), (29.4, 0.35, d), (100, 0.35, d)], still: 0.6, delay: 1.23)
        // .el-r.el-r3  (el-r 3.4s, delay 1.9s)
        let r3O = v([(0, 0.35, d), (5.3, 1, d), (29.4, 0.35, d), (100, 0.35, d)], still: 0.6, delay: 1.9)
        // .el-n.el-n1  (el-n 3.4s, delay 0.57s)
        let n1SX = v([(0, 1, o), (4.1, 1.4, io), (11.8, 1, d), (100, 1, d)], still: 1, delay: 0.57)
        let n1SY = v([(0, 1, o), (4.1, 1.4, io), (11.8, 1, d), (100, 1, d)], still: 1, delay: 0.57)
        // .el-n.el-n2  (el-n 3.4s, delay 1.23s)
        let n2SX = v([(0, 1, o), (4.1, 1.4, io), (11.8, 1, d), (100, 1, d)], still: 1, delay: 1.23)
        let n2SY = v([(0, 1, o), (4.1, 1.4, io), (11.8, 1, d), (100, 1, d)], still: 1, delay: 1.23)
        // .el-n.el-n3  (el-n 3.4s, delay 1.9s)
        let n3SX = v([(0, 1, o), (4.1, 1.4, io), (11.8, 1, d), (100, 1, d)], still: 1, delay: 1.9)
        let n3SY = v([(0, 1, o), (4.1, 1.4, io), (11.8, 1, d), (100, 1, d)], still: 1, delay: 1.9)
        // .el-dot  (el-dot 3.4s)
        let runnerY = v([(0, 0, l), (8.8, 0, l), (67.6, 58, d), (76.5, 58, d), (100, 58, d)], still: 0)
        let runnerO = v([(0, 0, l), (8.8, 0, l), (11.8, 1, l), (67.6, 1, d), (76.5, 0, d), (100, 0, d)], still: 0)

        stroke(c, svg("M30 16 V78"))
        layer(c, opacity: r1O) { c in
            stroke(c, rect(40, 21, 34, 10, rx: 3), dash: [3, 3])
        }
        layer(c, opacity: r2O) { c in
            stroke(c, rect(40, 41, 34, 10, rx: 3), dash: [3, 3])
        }
        layer(c, opacity: r3O) { c in
            stroke(c, rect(40, 61, 34, 10, rx: 3), dash: [3, 3])
        }
        layer(c, sx: n1SX, sy: n1SY, origin: pt(30, 26)) { c in
            solid(c, circle(30, 26, 3.2))
        }
        layer(c, sx: n2SX, sy: n2SY, origin: pt(30, 46)) { c in
            solid(c, circle(30, 46, 3.2))
        }
        layer(c, sx: n3SX, sy: n3SY, origin: pt(30, 66)) { c in
            solid(c, circle(30, 66, 3.2))
        }
        layer(c, ty: runnerY, opacity: runnerO) { c in
            layer(c, ty: dotY(delay: 0)) { c in
                dot(c, circle(30, 18, 2))
            }
        }
        stroke(c, svg("M26 78 h8"))
    }

    // MARK: 8. Key

    /// API keys, SSH keys, tokens, secrets, permissions
    /// Loop: slides in 0.4-1.15, turns (squash) 1.15-1.45, shackle opens 1.45-1.75, holds, closes 2.5-2.8, key out 2.8-3.4.
    /// Still frame: drawn as listed (no state differs from the loop start).
    func key(_ c: GraphicsContext) {
        let o = Ease.out, io = Ease.io, d = Ease.std
        // .ek-key  (ek-key 3.4s)
        let keyX = v([(0, 0, o), (11.8, 0, o), (33.8, 13, d), (38.2, 13, d), (42.6, 13, io), (82.4, 13, io), (100, 0, d)], still: 0)
        let keySY = v([(0, 1, o), (11.8, 1, o), (33.8, 1, d), (38.2, 0.35, d), (42.6, 1, io), (82.4, 1, io), (100, 1, d)], still: 1)
        // .ek-shackle  (ek-shackle 3.4s)
        let shackleY = v([(0, 0, o), (42.6, 0, o), (51.5, -5, io), (73.5, -5, io), (82.4, 0, d), (100, 0, d)], still: 0)

        solid(c, oval(50, 72, 30, 5))
        layer(c, tx: keyX, sy: keySY, origin: pt(32.5, 56)) { c in
            solid(c, circle(20, 56, 7))
            stroke(c, circle(20, 56, 2.5))
            stroke(c, svg("M27 56 H52 M44 56 v4 M48 56 v3"))
        }
        layer(c, ty: shackleY) { c in
            stroke(c, svg("M57 46 V38 a8 8 0 0 1 16 0 V46"))
        }
        solid(c, rect(52, 44, 26, 24, rx: 3))
        stroke(c, circle(65, 54, 2.4))
        stroke(c, svg("M65 56.4 V61"))
    }

    // MARK: 9. Apps

    /// apps, skills, connectors, plugins, MCP, extensions, mini apps
    /// Loop: a block slides into the empty dashed slot 0.5-1.3, impact lines, holds, lifts back out 2.6-3.4.
    /// Still frame: .eb-impact opacity:0; .eb-fly transform:translate(14,-12).
    func apps(_ c: GraphicsContext) {
        let o = Ease.out, io = Ease.io, d = Ease.std
        // .eb-impact  (eb-impact 3.4s)
        let impactSX = v([(0, 1, d), (38.2, 1, d), (44.1, 1.12, o), (51.5, 1.12, d), (100, 1.12, d)], still: 1)
        let impactSY = v([(0, 1, d), (38.2, 1, d), (44.1, 1.12, o), (51.5, 1.12, d), (100, 1.12, d)], still: 1)
        let impactO = v([(0, 0, d), (38.2, 0, d), (44.1, 1, o), (51.5, 0, d), (100, 0, d)], still: 0)
        // .eb-fly  (eb-fly 3.4s)
        let flyX = v([(0, 14, o), (14.7, 14, o), (38.2, 0, d), (41.2, 0, d), (45.6, 0, io), (76.5, 0, io), (100, 14, d)], still: 14)
        let flyY = v([(0, -12, o), (14.7, -12, o), (38.2, 0, d), (41.2, 1, d), (45.6, 0, io), (76.5, 0, io), (100, -12, d)], still: -12)
        // .eb-plus  (eb-plus 3.4s)
        let plusSX = v([(0, 0.8, io), (38.2, 0.8, io), (45.6, 1.15, io), (58.8, 0.8, io), (100, 0.8, io)], still: 1)
        let plusSY = v([(0, 0.8, io), (38.2, 0.8, io), (45.6, 1.15, io), (58.8, 0.8, io), (100, 0.8, io)], still: 1)
        let plusO = v([(0, 0.4, io), (38.2, 0.4, io), (45.6, 1, io), (58.8, 0.4, io), (100, 0.4, io)], still: 1)

        solid(c, svg("M36 49 L48 56 L36 63 L24 56 Z"))
        solid(c, svg("M24 56 V68 L36 75 V63"))
        solid(c, svg("M48 56 V68 L36 75"))
        solid(c, svg("M60 49 L72 56 L60 63 L48 56 Z"))
        solid(c, svg("M48 56 V68 L60 75 V63"))
        solid(c, svg("M72 56 V68 L60 75"))
        stroke(c, svg("M48 30 L60 37 L48 44 L36 37 Z"), dash: [3, 3])
        stroke(c, svg("M36 37 V49 L48 56 V44"), dash: [3, 3])
        stroke(c, svg("M60 37 V49 L48 56"), dash: [3, 3])
        layer(c, sx: impactSX, sy: impactSY, origin: pt(48, 44), opacity: impactO) { c in
            stroke(c, svg("M30 42 l-5 -2 M30 48 l-6 0 M66 42 l5 -2 M66 48 l6 0"))
        }
        layer(c, tx: flyX, ty: flyY) { c in
            solid(c, svg("M48 30 L60 37 L48 44 L36 37 Z"))
            solid(c, svg("M36 37 V49 L48 56 V44"))
            solid(c, svg("M60 37 V49 L48 56"))
        }
        layer(c, sx: plusSX, sy: plusSY, origin: pt(80, 21.5), opacity: plusO) { c in
            stroke(c, svg("M80 18 v7 M76.5 21.5 h7"))
        }
    }

    // MARK: 10. Memory

    /// memory, remembered notes, brains
    /// Loop: a page turns 0.4-1.1, thought dots and a cloud rise 1.0-1.5, hold, fade 2.7-3.1, page turns back.
    /// Still frame: drawn as listed (no state differs from the loop start).
    func memory(_ c: GraphicsContext) {
        let o = Ease.out, i = Ease.in, io = Ease.io, d = Ease.std
        // .em-page  (em-page 3.4s)
        let pageSX = v([(0, 1, io), (11.8, 1, io), (32.4, -1, io), (79.4, -1, io), (97.1, 1, d), (100, 1, d)], still: 1)
        // .em-t.em-t1  (em-t 3.4s)
        let t1Y = v([(0, 3, o), (29.4, 3, o), (36.8, 0, d), (79.4, 0, d), (100, 0, d)], still: 0)
        let t1O = v([(0, 0, o), (29.4, 0, o), (36.8, 1, d), (79.4, 1, d), (88.2, 0, d), (100, 0, d)], still: 1)
        // .em-t.em-t2  (em-t 3.4s, delay 0.15s)
        let t2Y = v([(0, 3, o), (29.4, 3, o), (36.8, 0, d), (79.4, 0, d), (100, 0, d)], still: 0, delay: 0.15)
        let t2O = v([(0, 0, o), (29.4, 0, o), (36.8, 1, d), (79.4, 1, d), (88.2, 0, d), (100, 0, d)], still: 1, delay: 0.15)
        // .em-cloud  (em-cloud 3.4s)
        let cloudSX = v([(0, 0.6, o), (38.2, 0.6, o), (47.1, 1, i), (79.4, 1, i), (89.7, 0.9, d), (100, 0.9, d)], still: 1)
        let cloudSY = v([(0, 0.6, o), (38.2, 0.6, o), (47.1, 1, i), (79.4, 1, i), (89.7, 0.9, d), (100, 0.9, d)], still: 1)
        let cloudO = v([(0, 0, o), (38.2, 0, o), (47.1, 1, i), (79.4, 1, i), (89.7, 0, d), (100, 0, d)], still: 1)

        solid(c, oval(48, 72, 30, 5))
        solid(c, svg("M48 70 L22 60 V36 L48 46 Z"))
        solid(c, svg("M48 70 L74 60 V36 L48 46 Z"))
        layer(c, opacity: 0.6) { c in
            stroke(c, svg("M27 44 l16 6 M27 50 l16 6 M27 56 l10 4"))
        }
        layer(c, opacity: 0.6) { c in
            stroke(c, svg("M53 50 l16 -6 M53 56 l16 -6"))
        }
        layer(c, sx: pageSX, origin: pt(48, 53)) { c in
            solid(c, svg("M48 70 L74 60 V36 L48 46 Z"))
        }
        layer(c, ty: t1Y, opacity: t1O) { c in
            solid(c, circle(58, 34, 1.8))
        }
        layer(c, ty: t2Y, opacity: t2O) { c in
            solid(c, circle(63, 27, 2.8))
        }
        layer(c, sx: cloudSX, sy: cloudSY, origin: pt(72.408, 16.111), opacity: cloudO) { c in
            solid(c, svg("M64 18 a4 4 0 0 1 4 -6 a5 5 0 0 1 9 0 a4 4 0 0 1 3 7 a3.5 3.5 0 0 1 -4 4 h-9 a3.5 3.5 0 0 1 -3 -5 Z"))
            stroke(c, svg("M71.5 12.5 v4 M69.5 14.5 h4"))
        }
    }

    // MARK: 11. Model

    /// models, providers, on-device models (Gizzi scene)
    /// Loop: an arrow drops into the chip 0.3-1.3, the core pops, the pins light.
    /// Still frame: drawn as listed (no state differs from the loop start).
    func model(_ c: GraphicsContext) {
        let o = Ease.out, i = Ease.in, io = Ease.io, d = Ease.std
        // .eM-pins  (eM-pins 3.4s)
        let pinsO = v([(0, 0.45, d), (41.2, 0.45, d), (47.1, 1, io), (76.5, 0.45, d), (100, 0.45, d)], still: 1)
        // .eM-core  (eM-core 3.4s)
        let coreSX = v([(0, 1, o), (38.2, 1, o), (45.6, 1.12, io), (55.9, 1, d), (100, 1, d)], still: 1)
        let coreSY = v([(0, 1, o), (38.2, 1, o), (45.6, 1.12, io), (55.9, 1, d), (100, 1, d)], still: 1)
        // .eM-arrow  (eM-arrow 3.4s)
        let arrowY = v([(0, -6, i), (8.8, -6, i), (38.2, 8, d), (44.1, 10, d), (100, 10, d)], still: 0)
        let arrowO = v([(0, 0, i), (8.8, 0, i), (17.6, 1, d), (38.2, 1, d), (44.1, 0, d), (100, 0, d)], still: 1)

        layer(c, opacity: pinsO) { c in
            stroke(c, svg("M38.00 34 v-5 M38.00 66 v5 M32 42.00 h-5 M64 42.00 h5 M44.67 34 v-5 M44.67 66 v5 M32 48.67 h-5 M64 48.67 h5 M51.33 34 v-5 M51.33 66 v5 M32 55.33 h-5 M64 55.33 h5 M58.00 34 v-5 M58.00 66 v5 M32 62.00 h-5 M64 62.00 h5"))
        }
        solid(c, rect(32, 34, 32, 32, rx: 4))
        layer(c, sx: coreSX, sy: coreSY, origin: pt(48, 50)) { c in
            stroke(c, rect(40, 42, 16, 16, rx: 2))
            stroke(c, svg("M48 46 l3 4 l-3 4 l-3 -4 Z"))
        }
        layer(c, ty: arrowY, opacity: arrowO) { c in
            stroke(c, svg("M48 8 V20 M43.5 15.5 L48 20 L52.5 15.5"))
        }
    }
}

/// Gizzi slot for agent and terminal empty states (style E). Draws the real
/// Gizzi mascot in a 96 x 96 box, centred, at 72, walking in place: a step
/// hop and a small rock, pacing side to side and turning at each end. The
/// app ships one mascot image (no pixel-pet walk row), so the walk is that
/// image in motion. Reduce Motion shows the idle frame. `box` scales the
/// whole walk (EmptyArt `.gizzi` passes its size; the mascot is 0.75 of it).
struct GizziWalk: View {
    var size: CGFloat = 72
    var box: CGFloat = 96
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
                    let k = Double(box) / 96
                    let x = ((right ? eased : 1 - eased) * 16 - 8) * k
                    let step = t * 2 * .pi * 2.4
                    mascot(x: x, hop: -abs(sin(step)) * 3.5 * k, rock: sin(step) * 4, facing: right ? 1 : -1)
                }
            }
        }
        .frame(width: box, height: box)
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

// MARK: - SVG path data

/// Parses SVG path data (M L H V C S Q T A Z, absolute and relative) into a
/// Path, so the v2 drawings keep spec.json's path strings verbatim. Arcs
/// become cubic curves (at most 90 degrees each).
private enum SVGPathData {
    private enum Token { case cmd(Character), num(Double) }

    static func parse(_ d: String) -> Path {
        let toks = tokens(d)
        var path = Path()
        var i = 0
        var cmd: Character = "M"
        var cur = CGPoint.zero, start = CGPoint.zero
        var lastCubic: CGPoint? = nil, lastQuad: CGPoint? = nil

        func isNum(_ j: Int) -> Bool {
            if j < toks.count, case .num = toks[j] { return true }
            return false
        }
        func n() -> Double {
            if i < toks.count, case .num(let v) = toks[i] { i += 1; return v }
            return 0 // missing number: leave the next command in place
        }

        while i < toks.count {
            if case .cmd(let c) = toks[i] {
                cmd = c
                i += 1
            } else if cmd == "Z" || cmd == "z" {
                i += 1 // stray number after Z
                continue
            }
            let rel = cmd.isLowercase
            let ox = rel ? cur.x : 0, oy = rel ? cur.y : 0
            var cubic: CGPoint? = nil, quad: CGPoint? = nil
            switch cmd {
            case "M", "m":
                cur = CGPoint(x: ox + n(), y: oy + n())
                start = cur
                path.move(to: cur)
                cmd = rel ? "l" : "L" // further pairs are lines
            case "L", "l":
                cur = CGPoint(x: ox + n(), y: oy + n())
                path.addLine(to: cur)
            case "H", "h":
                cur = CGPoint(x: ox + n(), y: cur.y)
                path.addLine(to: cur)
            case "V", "v":
                cur = CGPoint(x: cur.x, y: oy + n())
                path.addLine(to: cur)
            case "C", "c":
                let c1 = CGPoint(x: ox + n(), y: oy + n())
                let c2 = CGPoint(x: ox + n(), y: oy + n())
                cur = CGPoint(x: ox + n(), y: oy + n())
                path.addCurve(to: cur, control1: c1, control2: c2)
                cubic = c2
            case "S", "s":
                let c1 = lastCubic.map { CGPoint(x: 2 * cur.x - $0.x, y: 2 * cur.y - $0.y) } ?? cur
                let c2 = CGPoint(x: ox + n(), y: oy + n())
                cur = CGPoint(x: ox + n(), y: oy + n())
                path.addCurve(to: cur, control1: c1, control2: c2)
                cubic = c2
            case "Q", "q":
                let q = CGPoint(x: ox + n(), y: oy + n())
                cur = CGPoint(x: ox + n(), y: oy + n())
                path.addQuadCurve(to: cur, control: q)
                quad = q
            case "T", "t":
                let q = lastQuad.map { CGPoint(x: 2 * cur.x - $0.x, y: 2 * cur.y - $0.y) } ?? cur
                cur = CGPoint(x: ox + n(), y: oy + n())
                path.addQuadCurve(to: cur, control: q)
                quad = q
            case "A", "a":
                let rx = n(), ry = n(), phi = n(), large = n() != 0, sweep = n() != 0
                let to = CGPoint(x: ox + n(), y: oy + n())
                arc(&path, from: cur, rx: rx, ry: ry, phi: phi, large: large, sweep: sweep, to: to)
                cur = to
            case "Z", "z":
                path.closeSubpath()
                cur = start
            default:
                while isNum(i) { i += 1 } // unknown command: skip its numbers
            }
            lastCubic = cubic
            lastQuad = quad
        }
        return path
    }

    /// SVG elliptical arc (endpoint form, SVG 1.1 F.6.5) from `from` to `to`.
    static func arc(_ path: inout Path, from p1: CGPoint, rx: Double, ry: Double, phi: Double,
                    large: Bool, sweep: Bool, to p2: CGPoint) {
        if p1 == p2 { return }
        var rx = abs(rx), ry = abs(ry)
        if rx == 0 || ry == 0 { path.addLine(to: p2); return }
        let cp = cos(phi * .pi / 180), sp = sin(phi * .pi / 180)
        let dx = (p1.x - p2.x) / 2, dy = (p1.y - p2.y) / 2
        let x1p = cp * dx + sp * dy, y1p = -sp * dx + cp * dy
        let lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry)
        if lambda > 1 { rx *= lambda.squareRoot(); ry *= lambda.squareRoot() }
        let num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p
        let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p
        let coef = (den == 0 ? 0 : max(0, num / den).squareRoot()) * (large == sweep ? -1 : 1)
        let cxp = coef * rx * y1p / ry, cyp = coef * -ry * x1p / rx
        let cx = cp * cxp - sp * cyp + (p1.x + p2.x) / 2
        let cy = sp * cxp + cp * cyp + (p1.y + p2.y) / 2
        func angle(_ ux: Double, _ uy: Double, _ vx: Double, _ vy: Double) -> Double {
            atan2(ux * vy - uy * vx, ux * vx + uy * vy)
        }
        let t1 = angle(1, 0, (x1p - cxp) / rx, (y1p - cyp) / ry)
        var dt = angle((x1p - cxp) / rx, (y1p - cyp) / ry, (-x1p - cxp) / rx, (-y1p - cyp) / ry)
        if !sweep && dt > 0 { dt -= 2 * .pi }
        if sweep && dt < 0 { dt += 2 * .pi }
        let segs = max(1, Int((abs(dt) / (.pi / 2)).rounded(.up)))
        let delta = dt / Double(segs)
        let kappa = 4.0 / 3.0 * tan(delta / 4)
        func point(_ a: Double) -> CGPoint {
            CGPoint(x: cx + rx * cos(a) * cp - ry * sin(a) * sp, y: cy + rx * cos(a) * sp + ry * sin(a) * cp)
        }
        func slope(_ a: Double) -> CGPoint {
            CGPoint(x: -rx * sin(a) * cp - ry * cos(a) * sp, y: -rx * sin(a) * sp + ry * cos(a) * cp)
        }
        for s in 0..<segs {
            let a1 = t1 + Double(s) * delta, a2 = a1 + delta
            let q1 = point(a1), q2 = s == segs - 1 ? p2 : point(a2)
            let d1 = slope(a1), d2 = slope(a2)
            path.addCurve(to: q2,
                          control1: CGPoint(x: q1.x + kappa * d1.x, y: q1.y + kappa * d1.y),
                          control2: CGPoint(x: q2.x - kappa * d2.x, y: q2.y - kappa * d2.y))
        }
    }

    private static func tokens(_ d: String) -> [Token] {
        let b = Array(d.utf8)
        var out: [Token] = []
        var i = 0
        func digit(_ c: UInt8) -> Bool { c >= 48 && c <= 57 }
        while i < b.count {
            let c = b[i]
            let letter = (c >= 65 && c <= 90) || (c >= 97 && c <= 122)
            if letter && c != 101 && c != 69 { // e/E belong to numbers
                out.append(.cmd(Character(UnicodeScalar(c))))
                i += 1
            } else if c == 45 || c == 43 || c == 46 || digit(c) { // - + . 0-9
                var j = i
                if b[j] == 45 || b[j] == 43 { j += 1 }
                var dot = false, exp = false
                while j < b.count {
                    let x = b[j]
                    if digit(x) {
                        j += 1
                    } else if x == 46 && !dot && !exp {
                        dot = true
                        j += 1
                    } else if (x == 101 || x == 69) && !exp {
                        exp = true
                        j += 1
                        if j < b.count, b[j] == 45 || b[j] == 43 { j += 1 }
                    } else {
                        break
                    }
                }
                out.append(.num(Double(String(decoding: b[i..<j], as: UTF8.self)) ?? 0))
                i = max(j, i + 1)
            } else {
                i += 1 // separators
            }
        }
        return out
    }
}

#Preview("Line drawings") {
    let kinds: [EmptyArtKind] = EmptyArtKind.allCases.filter { !$0.isScene && $0 != .gizzi }
    ScrollView {
        LazyVGrid(columns: [GridItem(.adaptive(minimum: 96), spacing: 16)], spacing: 16) {
            ForEach(kinds, id: \.self) { EmptyArt(kind: $0) }
        }
        .padding()
    }
    .background(Color("BgPrimary"))
}

#Preview("Scenes and sizes") {
    VStack(spacing: 16) {
        HStack(spacing: 16) {
            EmptyArt(kind: .search)
            EmptyArt(kind: .model)
        }
        EmptyArt(kind: .connectScene)
        EmptyArt(kind: .gizzi)
        HStack(spacing: 16) {
            EmptyArt(kind: .log, size: .panel)
            EmptyArt(kind: .key, size: .inline)
            EmptyArt(kind: .search, size: .inline)
        }
    }
    .padding()
    .background(Color("BgPrimary"))
}
