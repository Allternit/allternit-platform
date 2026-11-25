import SwiftUI

/// The connect sheet's line scene (PLAN-v2 2.15): Gizzi asleep, connecting,
/// then cables and a check. Ported from `ConnectScene.reference.tsx`
/// (handoffs/ui-polish-2026-11-24/reference): same viewBox (120 x 72), paths,
/// timings and easings.
///
/// Driven ONLY by the real connection state the caller passes in. The scene
/// never advances to `.connected` on its own, and a failure goes back to
/// `.asleep`, so a check is never shown unearned.
///
/// Reduce Motion: no loops, no travelling pulse, no transitions. Each state is
/// its still frame.
enum ConnectSceneState: Equatable {
    case asleep, connecting, connected
}

struct ConnectScene: View {
    var state: ConnectSceneState

    @ReducedMotion private var reduced
    @State private var from = Vals.target(.asleep)
    @State private var current: ConnectSceneState = .asleep
    @State private var changedAt = Date.distantPast

    private static let aspect: CGFloat = 0.6

    var body: some View {
        GeometryReader { geo in
            let s = geo.size.width / 120
            let settled = reduced || (current != .connecting && current != .asleep && Date().timeIntervalSince(changedAt) > 1.0)
            TimelineView(.animation(minimumInterval: 1.0 / 60, paused: settled)) { tl in
                let t = tl.date
                let v = reduced ? Vals.target(current) : values(at: t)
                ZStack(alignment: .topLeading) {
                    SetupGizzi(size: 44 * s, emotion: emotion)
                        .frame(width: 44 * s, height: 44 * s)
                        .opacity(v.gizzi)
                        .position(x: 24 * s, y: 38 * s)
                    Canvas { ctx, size in
                        draw(&ctx, v, t, size.width / 120)
                    }
                }
            }
        }
        .aspectRatio(1 / Self.aspect, contentMode: .fit)
        .frame(maxWidth: 240)
        .frame(maxWidth: .infinity)
        .accessibilityHidden(true)
        .onAppear { current = state; from = Vals.target(state); changedAt = .distantPast }
        .onChange(of: state) { _, new in
            guard new != current else { return }
            from = reduced ? Vals.target(new) : values(at: Date())
            current = new
            changedAt = Date()
        }
    }

    private var emotion: SetupGizzi.Emotion {
        switch current {
        case .asleep: return .focused      // steady: dim + still + z marks
        case .connecting: return .curious
        case .connected: return .pleased
        }
    }

    // MARK: values

    struct Vals {
        var gizzi: Double, cable: Double, solid: Double, badge: Double, z: Double
        static func target(_ s: ConnectSceneState) -> Vals {
            switch s {
            case .asleep: return Vals(gizzi: 0.55, cable: 0.35, solid: 0, badge: 0, z: 1)
            case .connecting: return Vals(gizzi: 1, cable: 1, solid: 0, badge: 0, z: 0)
            case .connected: return Vals(gizzi: 1, cable: 1, solid: 1, badge: 1, z: 0)
            }
        }
    }

    private struct Chan { var dur: Double; var delay: Double; var ease: Ease }
    private enum Ease { case out, inOut }

    /// Per-channel duration, delay and easing for a transition into `to`.
    private func chans(_ to: ConnectSceneState) -> (gizzi: Chan, cable: Chan, solid: Chan, badge: Chan, z: Chan) {
        switch to {
        case .connecting:
            return (Chan(dur: 0.4, delay: 0, ease: .out), Chan(dur: 0.4, delay: 0, ease: .out),
                    Chan(dur: 0.25, delay: 0, ease: .out), Chan(dur: 0.25, delay: 0, ease: .inOut),
                    Chan(dur: 0.16, delay: 0, ease: .out))
        case .connected:
            return (Chan(dur: 0.4, delay: 0, ease: .out), Chan(dur: 0.25, delay: 0, ease: .out),
                    Chan(dur: 0.25, delay: 0, ease: .out), Chan(dur: 0.3, delay: 0.25, ease: .out),
                    Chan(dur: 0.16, delay: 0, ease: .out))
        case .asleep:
            let c = Chan(dur: 0.25, delay: 0, ease: .inOut)
            return (c, c, c, c, c)
        }
    }

    private func values(at date: Date) -> Vals {
        let el = date.timeIntervalSince(changedAt)
        let to = Vals.target(current)
        let c = chans(current)
        func mix(_ a: Double, _ b: Double, _ ch: Chan) -> Double {
            let p = min(max((el - ch.delay) / ch.dur, 0), 1)
            return a + (b - a) * Self.ease(p, ch.ease)
        }
        return Vals(gizzi: mix(from.gizzi, to.gizzi, c.gizzi), cable: mix(from.cable, to.cable, c.cable),
                    solid: mix(from.solid, to.solid, c.solid), badge: mix(from.badge, to.badge, c.badge),
                    z: mix(from.z, to.z, c.z))
    }

    /// cubic-bezier(.16,1,.3,1) and cubic-bezier(.65,0,.35,1).
    private static func ease(_ x: Double, _ e: Ease) -> Double {
        let (x1, y1, x2, y2): (Double, Double, Double, Double) = e == .out ? (0.16, 1, 0.3, 1) : (0.65, 0, 0.35, 1)
        func b(_ t: Double, _ a: Double, _ c: Double) -> Double {
            let u = 1 - t
            return 3 * u * u * t * a + 3 * u * t * t * c + t * t * t
        }
        var lo = 0.0, hi = 1.0
        for _ in 0..<24 {
            let mid = (lo + hi) / 2
            if b(mid, x1, x2) < x { lo = mid } else { hi = mid }
        }
        return b((lo + hi) / 2, y1, y2)
    }

    // MARK: drawing

    private func draw(_ ctx: inout GraphicsContext, _ v: Vals, _ date: Date, _ s: CGFloat) {
        ctx.scaleBy(x: s, y: s)
        let ink = Color("TextPrimary")
        let paper = Color("BgPrimary")
        let style = StrokeStyle(lineWidth: 1.5, lineCap: .round, lineJoin: .round)
        let clock = date.timeIntervalSinceReferenceDate
        let loops = !reduced

        func stroke(_ p: Path, _ color: Color = ink, _ opacity: Double = 1) {
            ctx.stroke(p, with: .color(color.opacity(opacity)), style: style)
        }

        // z marks (asleep only; fade out 0.16 s on leaving)
        if v.z > 0.001 {
            let zs: [(Path, Double)] = [
                (Self.zPath1, 0), (Self.zPath2, 1.2)
            ]
            for (p, delay) in zs {
                var dx = 0.0, dy = 0.0, op = 0.6
                if loops {
                    let ph = clock - delay
                    // animation-delay: the mark rests at .6 until its delay passes.
                    if ph >= 0 && fmod(clock, 2.4) >= 0 {
                        let f = fmod(ph, 2.4) / 2.4
                        let m = Self.ease(f, .inOut)
                        dx = 5 * m; dy = -8 * m
                        op = f < 0.3 ? Self.ease(f / 0.3, .inOut) : 1 - Self.ease((f - 0.3) / 0.7, .inOut)
                    }
                }
                stroke(p.applying(CGAffineTransform(translationX: dx, y: dy)), ink, op * v.z)
            }
        }

        // cables
        for (i, p) in [Self.cable1, Self.cable2].enumerated() {
            if v.solid > 0.001 {
                stroke(p.trimmedPath(from: 0, to: min(v.solid, 1)), ink, v.cable)
            }
            guard v.solid < 0.999 else { continue }
            let travelling = current == .connecting || (v.cable > 0.36 && current != .asleep)
            if travelling {
                if loops {
                    let t = fmod(clock - (i == 1 ? 0.6 : 0), 1.2) / 1.2
                    let t0 = t < 0 ? t + 1 : t
                    stroke(p.trimmedPath(from: t0, to: min(t0 + 0.22, 1)), ink, v.cable)
                    if t0 > 0.78 { stroke(p.trimmedPath(from: 0, to: t0 - 0.78), ink, v.cable) }
                } else {
                    stroke(p.trimmedPath(from: 0, to: 0.22), ink, v.cable)
                }
            } else {
                // dotted stubs at both ends
                stroke(p.trimmedPath(from: 0, to: 0.001), ink, v.cable)
                stroke(p.trimmedPath(from: 0.999, to: 1), ink, v.cable)
            }
        }

        // device
        let screenOpacity: Double = {
            guard loops, current == .connected else { return 1 }
            let p = min(max(Date().timeIntervalSince(changedAt) / 0.4, 0), 1)
            return 0.5 + 0.5 * Self.ease(p, .out)
        }()
        let body = Path(roundedRect: CGRect(x: 78, y: 18, width: 36, height: 30), cornerRadius: 4)
        ctx.fill(body, with: .color(paper))
        stroke(body)
        stroke(Path(roundedRect: CGRect(x: 82, y: 22, width: 28, height: 18), cornerRadius: 2), ink, screenOpacity)
        var stand = Path()
        stand.move(to: CGPoint(x: 96, y: 48)); stand.addLine(to: CGPoint(x: 96, y: 54))
        stand.move(to: CGPoint(x: 88, y: 54)); stand.addLine(to: CGPoint(x: 104, y: 54))
        stroke(stand)
        let socket = Path(ellipseIn: CGRect(x: 78 - 2.6, y: 33 - 2.6, width: 5.2, height: 5.2))
        ctx.fill(socket, with: .color(paper))
        stroke(socket)

        // socket ring (connecting loop)
        if loops && current == .connecting {
            let f = Self.ease(fmod(clock, 1.2) / 1.2, .out)
            let r = 2.6 * (1 + 0.8 * f)
            stroke(Path(ellipseIn: CGRect(x: 78 - r, y: 33 - r, width: 2 * r, height: 2 * r)), ink, 0.6 * (1 - f))
        }

        // check badge
        if v.badge > 0.001 {
            let sc = 0.6 + 0.4 * v.badge
            var bctx = ctx
            bctx.translateBy(x: 96, y: 33)
            bctx.scaleBy(x: sc, y: sc)
            bctx.translateBy(x: -96, y: -33)
            let disc = Path(ellipseIn: CGRect(x: 87, y: 24, width: 18, height: 18))
            bctx.fill(disc, with: .color(paper.opacity(v.badge)))
            bctx.stroke(disc, with: .color(ink.opacity(v.badge)), style: style)
            bctx.stroke(Self.tick, with: .color(Theme.statusSuccess.opacity(v.badge)), style: style)
        }
    }

    // MARK: paths (viewBox 120 x 72)

    private static let zPath1: Path = {
        var p = Path()
        p.move(to: CGPoint(x: 12, y: 10)); p.addLine(to: CGPoint(x: 17, y: 10))
        p.addLine(to: CGPoint(x: 12, y: 16)); p.addLine(to: CGPoint(x: 17, y: 16))
        return p
    }()
    private static let zPath2: Path = {
        var p = Path()
        p.move(to: CGPoint(x: 20, y: 4)); p.addLine(to: CGPoint(x: 23.5, y: 4))
        p.addLine(to: CGPoint(x: 20, y: 8)); p.addLine(to: CGPoint(x: 23.5, y: 8))
        return p
    }()
    private static let cable1: Path = {
        var p = Path()
        p.move(to: CGPoint(x: 42, y: 30))
        p.addCurve(to: CGPoint(x: 75.4, y: 33), control1: CGPoint(x: 56, y: 30), control2: CGPoint(x: 60, y: 26))
        return p
    }()
    private static let cable2: Path = {
        var p = Path()
        p.move(to: CGPoint(x: 42, y: 44))
        p.addCurve(to: CGPoint(x: 75.4, y: 33), control1: CGPoint(x: 56, y: 44), control2: CGPoint(x: 60, y: 40))
        return p
    }()
    private static let tick: Path = {
        var p = Path()
        p.move(to: CGPoint(x: 91.5, y: 33)); p.addLine(to: CGPoint(x: 94.5, y: 36.2)); p.addLine(to: CGPoint(x: 101, y: 29.4))
        return p
    }()
}
