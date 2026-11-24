import SwiftUI

/// The numbers of the Allternit block pulse, ported from the web
/// `src/components/brand/BlockPulse.tsx` (`drawBlockPulse`, `pulseCell`,
/// `PULSE_TILE_S`, `RISE`): rounded squares on a grid pop in from an origin,
/// delayed by their distance from it, then fade. It is the wait for a picture
/// or a clip. One implementation; the view below only loops waves over it.
enum BlockPulseMath {
    /// PULSE_TILE_S: seconds one square takes to pop in and fade out.
    static let tileSeconds = 0.7
    /// RISE: the pop-in share of a square's life; the rest is the fade.
    static let rise = 0.35
    /// drawBlockPulse defaults.
    static let defaultSpread = 0.00045
    /// BlockPulse component defaults: seconds between waves, peak opacity of the squares.
    static let defaultPeriod = 0.9
    static let defaultStrength = 0.18
    /// The centre square (within 0.8 cells of the origin) wears the tint at this opacity.
    static let tintAlpha = 0.5
    /// The resting grid is the squares' opacity times this.
    static let restingShare = 0.22

    /// pulseDuration: how long a wave of this reach lasts, in seconds.
    static func duration(reach: Double, spread: Double = defaultSpread) -> Double {
        tileSeconds + reach * spread
    }

    /// pulseCell: the grid pitch for a box, about a dozen squares across, never tiny or huge.
    static func cell(width: Double, height: Double) -> Double {
        max(12, min(30, (min(width, height) / 10).rounded()))
    }

    /// The gap between squares for a pitch.
    static func gap(pitch: Double) -> Double { max(3, pitch * 0.27) }

    /// A ring that crosses the box in about half a second, whatever its size.
    static func spread(reach: Double) -> Double { reach > 0 ? 0.5 / reach : defaultSpread }

    /// Reach to the farthest corner, so the whole box fills.
    static func reach(origin: CGPoint, size: CGSize) -> Double {
        let x = Double(origin.x), y = Double(origin.y), w = Double(size.width), h = Double(size.height)
        return max(hypot(x, y), hypot(w - x, y), hypot(x, h - y), hypot(w - x, h - y))
    }

    /// One square's opacity share and scale at `since` seconds into a wave,
    /// for a square `distance` from the origin. Nil outside its life.
    static func tile(since: Double, distance: Double, spread: Double) -> (alpha: Double, scale: Double)? {
        let q = (since - distance * spread) / tileSeconds
        if q < 0 || q > 1 { return nil }
        let a = q < rise ? q / rise : 1 - (q - rise) / (1 - rise)
        let sc = q < rise ? 0.3 + 0.7 * (q / rise) : 1 - 0.15 * ((q - rise) / (1 - rise))
        return (a, sc)
    }

    /// The still frame reduced motion shows: mid-wave, the centre past its
    /// peak, the middle at it, the edge arriving.
    static func stillSince(reach: Double, spread: Double) -> Double {
        rise * tileSeconds + reach * spread * 0.5
    }
}

/// The looping pulse, filling its parent (give it a size). Decorative and
/// hidden from VoiceOver: the caller owns the status text. Reduced motion
/// draws one still frame. Colours come from the app's tokens, so it reads in
/// light and graphite dark mode.
struct BlockPulseView: View {
    var active = true
    /// Where waves start, as fractions of the box (default: the centre).
    var origin = CGPoint(x: 0.5, y: 0.5)
    var period = BlockPulseMath.defaultPeriod
    var strength = BlockPulseMath.defaultStrength

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var start = Date()

    /// The brand tint (`--block-pulse-tint` fallback on the web).
    private static let tint = Color(hex: "#26A0BE")

    var body: some View {
        Group {
            if reduceMotion || !active {
                Canvas { context, size in paint(context, size, elapsed: nil) }
            } else {
                TimelineView(.animation) { timeline in
                    Canvas { context, size in paint(context, size, elapsed: timeline.date.timeIntervalSince(start)) }
                }
            }
        }
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }

    private func paint(_ context: GraphicsContext, _ size: CGSize, elapsed: Double?) {
        guard active, size.width > 0, size.height > 0 else { return }
        let pitch = BlockPulseMath.cell(width: size.width, height: size.height)
        let gap = BlockPulseMath.gap(pitch: pitch)
        let ox = Double(size.width) * Double(origin.x), oy = Double(size.height) * Double(origin.y)
        let reach = BlockPulseMath.reach(origin: CGPoint(x: ox, y: oy), size: size)
        let spread = BlockPulseMath.spread(reach: reach)
        let ink = Color("TextPrimary")

        // The resting grid: every square faintly there, so the waves read as light passing over a field.
        let half = (pitch - gap) / 2
        var cy = 0
        while Double(cy) * pitch < Double(size.height) {
            var cx = 0
            while Double(cx) * pitch < Double(size.width) {
                let px = Double(cx) * pitch + pitch / 2, py = Double(cy) * pitch + pitch / 2
                let rect = CGRect(x: px - half, y: py - half, width: half * 2, height: half * 2)
                context.fill(Path(roundedRect: rect, cornerRadius: pitch / 6), with: .color(ink.opacity(strength * BlockPulseMath.restingShare)))
                cx += 1
            }
            cy += 1
        }

        // This wave and the one before it, which is still fading at the edges.
        let since: Double
        if let elapsed {
            since = elapsed - (elapsed / period).rounded(.down) * period
        } else {
            since = BlockPulseMath.stillSince(reach: reach, spread: spread)
        }
        for s in [since, since + period] {
            drawWave(context, ox: ox, oy: oy, since: s, reach: reach, cell: pitch, gap: gap, spread: spread, ink: ink)
        }
    }

    /// drawBlockPulse: one wave at one moment. A wave outside its life draws nothing.
    private func drawWave(_ context: GraphicsContext, ox: Double, oy: Double, since: Double, reach: Double, cell: Double, gap: Double, spread: Double, ink: Color) {
        if since < 0 || since > BlockPulseMath.duration(reach: reach, spread: spread) { return }
        let radius = cell / 6
        let yRange = Int(((oy - reach) / cell).rounded(.down))...Int(((oy + reach) / cell).rounded(.up))
        let xRange = Int(((ox - reach) / cell).rounded(.down))...Int(((ox + reach) / cell).rounded(.up))
        for cy in yRange {
            for cx in xRange {
                let px = Double(cx) * cell + cell / 2, py = Double(cy) * cell + cell / 2
                let d = hypot(px - ox, py - oy)
                if d > reach { continue }
                guard let t = BlockPulseMath.tile(since: since, distance: d, spread: spread) else { continue }
                let center = d < cell * 0.8
                let color = (center ? Self.tint : ink).opacity((center ? BlockPulseMath.tintAlpha : strength) * t.alpha)
                let half = ((cell - gap) / 2) * t.scale
                let rect = CGRect(x: px - half, y: py - half, width: half * 2, height: half * 2)
                context.fill(Path(roundedRect: rect, cornerRadius: radius * t.scale), with: .color(color))
            }
        }
    }
}
