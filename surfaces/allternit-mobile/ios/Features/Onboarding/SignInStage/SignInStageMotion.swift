import SwiftUI

/// Motion for the sign-in stage, ported from the web beat sheet
/// (allternit-ai src/components/auth/sign-in-stage/sign-in-stage.css).
///
/// Nothing here runs a SwiftUI animation. Every piece is a pure function of
/// `stageTime`: seconds since its slide became current. The stage drives that
/// clock from a `TimelineView`, so a scene can be drawn at any moment, and
/// Reduce Motion simply draws every scene at its finished time.

extension EnvironmentValues {
    /// Seconds since the slide this view belongs to became current.
    @Entry var stageTime: Double = StageMotion.finished
    /// True under Reduce Motion: no drift, bob or other idle loops.
    @Entry var stageStill: Bool = false
}

enum StageMotion {
    /// A time well past every beat: the finished scene.
    static let finished: Double = 60

    static func clamp(_ x: Double, _ lo: Double = 0, _ hi: Double = 1) -> Double {
        min(hi, max(lo, x))
    }

    /// 0…1 progress of a beat that starts at `delay` and runs `duration`.
    static func progress(_ t: Double, delay: Double, duration: Double) -> Double {
        clamp((t - delay) / duration)
    }

    static func lerp(_ a: Double, _ b: Double, _ e: Double) -> Double {
        a + (b - a) * e
    }
}

/// CSS `cubic-bezier(x1, y1, x2, y2)`, solved for y at a given x.
struct StageCurve: Sendable {
    let x1: Double, y1: Double, x2: Double, y2: Double

    static let easeOut = StageCurve(x1: 0, y1: 0, x2: 0.58, y2: 1)
    static let easeInOut = StageCurve(x1: 0.42, y1: 0, x2: 0.58, y2: 1)
    static let pop = StageCurve(x1: 0.34, y1: 1.5, x2: 0.64, y2: 1)
    static let rise = StageCurve(x1: 0.34, y1: 1.35, x2: 0.64, y2: 1)
    static let slide = StageCurve(x1: 0.2, y1: 0.8, x2: 0.2, y2: 1)
    static let tilt = StageCurve(x1: 0.34, y1: 1.4, x2: 0.64, y2: 1)
    static let push = StageCurve(x1: 0.32, y1: 0.72, x2: 0, y2: 1)
    static let leave = StageCurve(x1: 0.4, y1: 0, x2: 1, y2: 1)
    static let away = StageCurve(x1: 0.4, y1: 0, x2: 0.2, y2: 1)
    static let burst = StageCurve(x1: 0.2, y1: 0.7, x2: 0.3, y2: 1)
    static let fly = StageCurve(x1: 0.3, y1: 1.55, x2: 0.6, y2: 1)
    static let cursor = StageCurve(x1: 0.65, y1: 0, x2: 0.35, y2: 1)

    func callAsFunction(_ x: Double) -> Double {
        if x <= 0 { return 0 }
        if x >= 1 { return 1 }
        // Newton on x(s), falling back to bisection.
        var s = x
        for _ in 0..<8 {
            let dx = sample(s, x1, x2) - x
            if abs(dx) < 1e-5 { return sample(s, y1, y2) }
            let d = slope(s, x1, x2)
            if abs(d) < 1e-6 { break }
            s -= dx / d
        }
        var lo = 0.0, hi = 1.0
        s = x
        for _ in 0..<24 {
            let v = sample(s, x1, x2)
            if abs(v - x) < 1e-5 { break }
            if v < x { lo = s } else { hi = s }
            s = (lo + hi) / 2
        }
        return sample(s, y1, y2)
    }

    private func sample(_ s: Double, _ a: Double, _ b: Double) -> Double {
        let u = 1 - s
        return 3 * u * u * s * a + 3 * u * s * s * b + s * s * s
    }

    private func slope(_ s: Double, _ a: Double, _ b: Double) -> Double {
        let u = 1 - s
        return 3 * u * u * a + 6 * u * s * (b - a) + 3 * s * s * (1 - b)
    }
}

/// The stage's animation vocabulary (web class in brackets).
enum StageBeat {
    /// Spring up from 12 pt below at 92 % (ast-a).
    case pop
    /// Fade in (ast-f).
    case fade
    /// Rise from 26 pt below (ast-r).
    case rise
    /// Slide in from 8 pt left (ast-l).
    case slideIn
    /// Grow from the leading edge (ast-g).
    case grow
    /// Fly in tilted and settle at −2.5° (ast-tilt).
    case tilt
    /// Squash and spring back, for a button being pressed (ast-press).
    case press
    /// Fade out (ast-f-out).
    case fadeOut
    /// Lift away and blur out as the result takes over (astx-away).
    case away
    /// The Home composer docking to the bottom (astx-dock).
    case dock
    /// The caption body under the stage (ast-pop, ease-out).
    case caption
}

struct StageFrame {
    var opacity: Double = 1
    var dx: Double = 0
    var dy: Double = 0
    var scaleX: Double = 1
    var scaleY: Double = 1
    var rotation: Double = 0
    var blur: Double = 0
    var anchor: UnitPoint = .center

    static func of(_ beat: StageBeat, at t: Double, delay: Double) -> StageFrame {
        var f = StageFrame()
        func p(_ duration: Double) -> Double { StageMotion.progress(t, delay: delay, duration: duration) }
        switch beat {
        case .pop, .caption:
            let e = beat == .pop ? StageCurve.pop(p(0.55)) : StageCurve.easeOut(p(0.45))
            f.opacity = e
            f.dy = 12 * (1 - e)
            f.scaleX = 0.92 + 0.08 * e
            f.scaleY = f.scaleX
        case .fade:
            f.opacity = StageCurve.easeOut(p(0.45))
        case .rise:
            let e = StageCurve.rise(p(0.6))
            f.opacity = e
            f.dy = 26 * (1 - e)
            f.scaleX = 0.96 + 0.04 * e
            f.scaleY = f.scaleX
        case .slideIn:
            let e = StageCurve.slide(p(0.32))
            f.opacity = e
            f.dx = -8 * (1 - e)
        case .grow:
            f.scaleX = StageCurve.slide(p(0.7))
            f.anchor = .leading
        case .tilt:
            let e = StageCurve.tilt(p(0.75))
            f.opacity = e
            f.dx = 44 * (1 - e)
            f.dy = 18 * (1 - e)
            f.rotation = StageMotion.lerp(9, -2.5, e)
            f.scaleX = 0.94 + 0.06 * e
            f.scaleY = f.scaleX
        case .press:
            let s = StageBeatKeyframes.press(p(0.4))
            f.scaleX = s
            f.scaleY = s
        case .fadeOut:
            f.opacity = 1 - StageCurve.easeOut(p(0.22))
        case .away:
            let e = StageCurve.away(p(0.5))
            f.opacity = 1 - e
            f.dy = -26 * e
            f.blur = 3 * e
        case .dock:
            f.dy = 92 * StageCurve.push(p(0.55))
        }
        f.opacity = StageMotion.clamp(f.opacity)
        return f
    }
}

enum StageBeatKeyframes {
    /// ast-press: 1 → .84 (40 %) → 1.08 (75 %) → 1, ease-out per segment.
    static func press(_ p: Double) -> Double {
        segments(p, [(0, 1), (0.4, 0.84), (0.75, 1.08), (1, 1)])
    }

    /// Piecewise keyframes with ease-out inside each segment (CSS default).
    static func segments(_ p: Double, _ keys: [(Double, Double)], curve: StageCurve = .easeOut) -> Double {
        guard let first = keys.first else { return 0 }
        if p <= first.0 { return first.1 }
        for i in 1..<keys.count where p <= keys[i].0 {
            let (a, va) = keys[i - 1]
            let (b, vb) = keys[i]
            return StageMotion.lerp(va, vb, curve((p - a) / (b - a)))
        }
        return keys[keys.count - 1].1
    }
}

private struct StageBeatModifier: ViewModifier {
    let beat: StageBeat
    let delay: Double
    @Environment(\.stageTime) private var t

    func body(content: Content) -> some View {
        let f = StageFrame.of(beat, at: t, delay: delay)
        content
            .scaleEffect(x: f.scaleX, y: f.scaleY, anchor: f.anchor)
            .rotationEffect(.degrees(f.rotation))
            .blur(radius: f.blur)
            .offset(x: f.dx, y: f.dy)
            .opacity(f.opacity)
    }
}

extension View {
    /// Plays one beat of the stage vocabulary, starting `delay` seconds into the slide.
    func beat(_ beat: StageBeat, at delay: Double) -> some View {
        modifier(StageBeatModifier(beat: beat, delay: delay))
    }

    /// Places a view at canvas coordinates (CSS `left` / `top`).
    func place(_ x: CGFloat, _ y: CGFloat) -> some View {
        offset(x: x, y: y)
    }
}
