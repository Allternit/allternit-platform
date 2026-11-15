import SwiftUI
import VoiceRingKit

/// Live mode's speaker (allternit-ai src/components/voice/VoiceSpeaker.tsx):
/// the bot's avatar at full size with its speaker look around it.
///
/// - Ring: voice-ring's SwiftUI port. It is drawn once, over the face, with a
///   cut-out around the head and chest, so its lower arc passes in front of
///   the body and the rest sits behind. A palette tints it (greyscale, then
///   the palette's colour); Spectrum keeps its own colours.
/// - Blocks, ASCII, Glow: our own looks, drawn behind the face.
/// - Still: the face alone; it lifts a little while the bot talks.
///
/// Levels: the mic while you talk, the bot's spoken audio while it speaks
/// (`VoiceLevels`). The system speech voice has no audio to measure, so the
/// look follows a gentle speaking motion then. Reduce Motion stills it all;
/// the screen's status line says the state in words.
struct VoiceSpeakerView<Face: View>: View {
    enum Phase: Equatable { case connecting, idle, listening, thinking, speaking }

    let phase: Phase
    let muted: Bool
    let style: SpeakerStyle
    let palette: RingPalette
    let botColor: Color?
    /// The face's size; the look is drawn around it.
    let size: CGFloat
    /// The docked bar's small version: no cut-out, no shadow.
    var compact: Bool = false
    /// Settings tiles: hold still.
    var still: Bool = false
    @ViewBuilder let face: () -> Face

    @Environment(\.colorScheme) private var colorScheme
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    static var scale: CGFloat { 1.78 }

    private var outer: CGFloat { (size * Self.scale).rounded() }
    private var paused: Bool { still || reduceMotion }
    private var drawn: SpeakerStyle { style }
    private var colors: [Color]? {
        if muted { return RingPalette.idle }
        return palette.colors(bot: botColor)
    }
    private var light: Color { colors?.first ?? botColor ?? Color(hex: "#8C8E94") }
    /// The Ring is a flat circle bigger than the face: the face stands in it.
    private var faceDy: CGFloat { drawn == .ring && !compact ? (size * 0.15).rounded() : 0 }

    private var ringState: VoiceState {
        if muted { return .idle }
        switch phase {
        case .speaking: return .speaking
        case .listening: return .listening
        case .thinking, .connecting: return .processing
        case .idle: return .idle
        }
    }

    /// The level the look follows this frame.
    private func level(at t: TimeInterval) -> Double {
        if paused || muted { return 0 }
        switch phase {
        case .listening: return VoiceLevels.shared.micLevel()
        case .speaking:
            let measured = VoiceLevels.shared.outputLevel()
            if measured > 0.02 { return measured }
            // The system speech voice: a speaking motion, not a measurement.
            let syl = max(0, sin(t * 11) * 0.5 + sin(t * 3.1) * 0.35 + 0.25)
            return min(1, syl * (sin(t * 0.9) > -0.6 ? 0.8 : 0.1))
        default: return 0
        }
    }

    var body: some View {
        TimelineView(.animation(minimumInterval: 1 / 30, paused: paused)) { context in
            let t = context.date.timeIntervalSinceReferenceDate
            let lv = level(at: t)
            ZStack {
                if drawn == .blocks || drawn == .ascii || drawn == .glow {
                    OwnLookCanvas(kind: drawn, size: outer, level: lv, working: ringState == .processing, colors: colors ?? RingPalette.spectrumColors, time: paused ? 0 : t)
                }
                faceLayer(lv: lv, t: t)
                if drawn == .ring || drawn.orbType != nil { ring }
            }
            .frame(width: outer, height: outer)
        }
        .frame(width: outer, height: outer)
        .padding(compact ? -size * 0.39 : 0)
        .accessibilityHidden(true)
    }

    private func faceLayer(lv: Double, t: TimeInterval) -> some View {
        let lift = drawn == .still && phase == .speaking && !paused
        return ZStack(alignment: .bottom) {
            if !compact {
                Ellipse()
                    .fill(RadialGradient(colors: [light.opacity(0.45), .clear], center: .center, startRadius: 0, endRadius: size * 0.3))
                    .frame(width: size * 0.56 * (1 + lv * 0.15), height: size * 0.09)
                    .blur(radius: 4)
                    .offset(y: -size * 0.04)
            }
            face()
                .frame(width: size, height: size)
                .scaleEffect(1 + lv * 0.035, anchor: UnitPoint(x: 0.5, y: 0.85))
                .offset(y: lift ? -4 * CGFloat(abs(sin(t * 3.5))) : 0)
                .shadow(color: light.opacity(muted ? 0.15 : 0.6), radius: (compact ? 2 : 6) + lv * (compact ? 6 : 16))
                .opacity(muted ? 0.7 : 1)
        }
        .frame(width: size, height: size)
        .offset(y: faceDy)
    }

    private var ringStyle: VoiceStyle {
        var style = VoiceStyle()
        // A tinted look draws as light ink, so the palette colour (multiplied
        // in below) comes out as itself instead of darkening to brown.
        style.theme = colors != nil || colorScheme == .dark ? .dark : .light
        style.paused = paused
        style.sensitivity = 0.9
        return style
    }

    @ViewBuilder
    private var ring: some View {
        let source = VoiceSource.level({ [self] in level(at: Date().timeIntervalSinceReferenceDate) })
        let base = Group {
            if let type = drawn.orbType {
                OrbView(type: type, state: ringState, source: source, style: ringStyle, size: outer)
            } else {
                VoiceRing(state: ringState, source: source, style: ringStyle, size: outer)
            }
        }
        Group {
            if let colors {
                base.saturation(0).colorMultiply(colors[0])
            } else {
                base
            }
        }
        // Over the face, minus the head and chest: the lower arc crosses the body.
        .mask {
            if compact {
                Rectangle()
            } else {
                Rectangle()
                    .overlay(
                        Ellipse()
                            .frame(width: size * 0.92, height: size * 0.82)
                            .offset(y: faceDy - size * 0.1)
                            .blur(radius: size * 0.04)
                            .blendMode(.destinationOut)
                    )
                    .compositingGroup()
            }
        }
    }
}

/// Blocks, ASCII and Glow, drawn from the same level each frame.
private struct OwnLookCanvas: View {
    let kind: SpeakerStyle
    let size: CGFloat
    let level: Double
    let working: Bool
    let colors: [Color]
    let time: TimeInterval

    var body: some View {
        Canvas { ctx, canvasSize in
            let cx = canvasSize.width / 2, cy = canvasSize.height / 2
            let R = size * 0.39
            let col: (Int, Int) -> Color = { i, n in colors[(i * colors.count / max(n, 1)) % colors.count] }
            switch kind {
            case .blocks:
                let n = 24
                let b = size * 0.026
                for i in 0..<n {
                    let ang = Double(i) / Double(n) * .pi * 2 - .pi / 2 + (working ? time * 0.8 : 0)
                    let wob = 0.5 + 0.5 * sin(time * 6 + Double(i) * 1.7)
                    let amp = working ? 0.3 + 0.7 * ((sin(time * 4 - Double(i) * 0.5) + 1) / 2) : 0.25 + level * 1.6 * wob
                    let r = R * 0.86 + amp * size * 0.035
                    let s = b * (0.75 + amp * 0.6)
                    var c = ctx
                    c.translateBy(x: cx + cos(ang) * r, y: cy + sin(ang) * r)
                    c.rotate(by: .radians(ang))
                    c.opacity = 0.35 + min(0.65, amp)
                    let rect = CGRect(x: -s / 2, y: -s / 2, width: s, height: s)
                    c.fill(Path(roundedRect: rect, cornerRadius: s * 0.18), with: .color(i % 8 == 0 ? Color(hex: "#D97757") : col(i, n)))
                }
            case .ascii:
                let chars = Array(" .:-=+*#%@")
                let n = 40
                for i in 0..<n {
                    let ang = Double(i) / Double(n) * .pi * 2 + time * (working ? 0.6 : 0.12)
                    let e = working ? (sin(time * 5 - Double(i) * 0.6) + 1) / 2 : min(1, 0.15 + level * 1.8 * (0.5 + 0.5 * sin(time * 7 + Double(i) * 2.3)))
                    let r = R + e * size * 0.05
                    var c = ctx
                    c.opacity = 0.35 + e * 0.65
                    let ch = String(chars[max(1, Int((e * Double(chars.count - 1)).rounded()))])
                    c.draw(Text(ch).font(.system(size: size * 0.05, weight: .semibold, design: .monospaced)).foregroundColor(col(i, n)),
                           at: CGPoint(x: cx + cos(ang) * r, y: cy + sin(ang) * r))
                }
            default:
                let k = working ? 0.5 + 0.2 * sin(time * 3) : 0.35 + level * 0.9
                let base = colors.first ?? .gray
                let grad = Gradient(stops: [
                    .init(color: base.opacity(0), location: 0.5),
                    .init(color: base.opacity(min(1, k) * 0.65), location: 0.72),
                    .init(color: base.opacity(0), location: 1),
                ])
                ctx.fill(Path(ellipseIn: CGRect(x: 0, y: 0, width: canvasSize.width, height: canvasSize.height)),
                         with: .radialGradient(grad, center: CGPoint(x: cx, y: cy), startRadius: 0, endRadius: R * (1.05 + k * 0.25)))
            }
        }
        .frame(width: size, height: size)
    }
}
