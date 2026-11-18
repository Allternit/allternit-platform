import SwiftUI

/// Native Canvas port of the `thinking-orbs` web component (orbs.jakubantalik.com,
/// used on ai.allternit.com). Not a pixel-identical port — the web bundle's draw
/// routines are minified/obfuscated — but matches its states, sizing, monochrome
/// theming and accessibility behavior so mobile and web read as one visual language.
enum ThinkingOrbState: String, CaseIterable {
    case working, searching, solving, listening, connecting, weaving, composing, breathing, shaping

    /// Maps a tool-call name (as reported by the wire protocol, e.g. "Bash", "WebSearch")
    /// to the state that best represents it — mirrors the web app's tool-renderer mapping.
    static func forToolName(_ name: String) -> ThinkingOrbState {
        switch name {
        case "WebSearch", "Grep", "Glob", "NotebookQuery":
            return .searching
        case "Edit", "Write":
            return .composing
        case "PlanWrite":
            return .weaving
        case "TodoWrite":
            return .solving
        case "Task", "Agent":
            return .connecting
        case "Thinking":
            return .breathing
        default:
            return .working
        }
    }

    var accessibilityLabel: String {
        switch self {
        case .working: return "Working"
        case .searching: return "Searching"
        case .solving: return "Solving"
        case .listening: return "Listening"
        case .connecting: return "Connecting"
        case .weaving: return "Weaving"
        case .composing: return "Composing"
        case .breathing: return "Thinking"
        case .shaping: return "Shaping"
        }
    }
}

/// Two tuned presets, matching the web component: 64 (avatar scale), 20 (inline-text scale).
enum ThinkingOrbSize: CGFloat, CaseIterable {
    case avatar = 64
    case inline = 20

    fileprivate var dotCount: Int { self == .avatar ? 28 : 14 }
    fileprivate var dotRadius: CGFloat { self == .avatar ? 1.6 : 0.9 }
}

struct ThinkingOrb: View {
    var state: ThinkingOrbState = .working
    var size: ThinkingOrbSize = .avatar
    var speed: Double = 1
    var paused: Bool = false
    var label: String?

    @Environment(\.colorScheme) private var colorScheme
    @ReducedMotion private var reduceMotion

    private var ink: Color {
        colorScheme == .dark ? Color.white.opacity(0.85) : Color.black.opacity(0.78)
    }

    var body: some View {
        let dimension = size.rawValue
        Group {
            if reduceMotion || paused {
                Canvas { context, canvasSize in
                    draw(context: context, size: canvasSize, t: 0.35)
                }
            } else {
                TimelineView(.animation) { timeline in
                    Canvas { context, canvasSize in
                        let t = timeline.date.timeIntervalSinceReferenceDate * speed
                        draw(context: context, size: canvasSize, t: t)
                    }
                }
            }
        }
        .frame(width: dimension, height: dimension)
        .accessibilityLabel(label ?? state.accessibilityLabel)
        .accessibilityAddTraits(.updatesFrequently)
    }

    private func draw(context: GraphicsContext, size canvasSize: CGSize, t: Double) {
        let cx = Double(canvasSize.width) / 2
        let cy = Double(canvasSize.height) / 2
        let radius = Double(min(canvasSize.width, canvasSize.height)) / 2 - Double(size.dotRadius) * 2
        let dotCount = size.dotCount
        let dotRadius = size.dotRadius

        func dot(_ x: Double, _ y: Double, opacity: Double = 1) {
            let px = CGFloat(cx + x)
            let py = CGFloat(cy + y)
            let rect = CGRect(x: px - dotRadius, y: py - dotRadius, width: dotRadius * 2, height: dotRadius * 2)
            context.fill(Path(ellipseIn: rect), with: .color(ink.opacity(opacity)))
        }

        switch state {
        case .working:
            for ring in 0..<3 {
                let tilt = Double(ring) * 0.9
                let ringRadius = radius * (0.45 + 0.28 * Double(ring))
                let ringSpeed = 0.5 + Double(ring) * 0.22
                let perRing = max(4, dotCount / 3)
                for i in 0..<perRing {
                    let angle = (Double(i) / Double(perRing)) * 2 * .pi + t * ringSpeed
                    let x = cos(angle) * ringRadius
                    let y = sin(angle) * ringRadius * cos(tilt)
                    dot(x, y, opacity: 0.5 + 0.5 * cos(angle))
                }
            }

        case .searching:
            let latitudes = 5
            for lat in 0..<latitudes {
                let latFrac = Double(lat) / Double(latitudes - 1) - 0.5
                let latRadius = radius * cos(latFrac * .pi * 0.9)
                let y = sin(latFrac * .pi * 0.9) * radius
                let perLat = max(6, Int(Double(dotCount) * cos(latFrac * .pi * 0.4)))
                for i in 0..<perLat {
                    let angle = (Double(i) / Double(perLat)) * 2 * .pi
                    let x = cos(angle) * latRadius
                    dot(x, y, opacity: 0.35)
                }
            }
            let sweepAngle = t.truncatingRemainder(dividingBy: 2) / 2 * 2 * .pi
            for lat in 0..<latitudes {
                let latFrac = Double(lat) / Double(latitudes - 1) - 0.5
                let latRadius = radius * cos(latFrac * .pi * 0.9)
                let y = sin(latFrac * .pi * 0.9) * radius
                let x = cos(sweepAngle) * latRadius
                dot(x, y, opacity: 1)
            }

        case .solving:
            let bands = 4
            let quarterTurns = floor(t / 0.6)
            let settleProgress = min(1, (t / 0.6 - quarterTurns) / 0.4)
            for band in 0..<bands {
                let bandY = (Double(band) / Double(bands - 1) - 0.5) * radius * 1.6
                let scramble = sin(quarterTurns * 2.1 + Double(band) * 1.7) * .pi / 2
                let settled = round(scramble / (.pi / 2)) * (.pi / 2)
                let angleOffset = scramble + (settled - scramble) * settleProgress
                let perBand = max(4, dotCount / bands)
                for i in 0..<perBand {
                    let baseAngle = (Double(i) / Double(perBand)) * 2 * .pi
                    let x = cos(baseAngle + angleOffset) * radius * 0.85
                    dot(x, bandY, opacity: 0.7)
                }
            }

        case .listening:
            let rings = 4
            for ring in 0..<rings {
                let ringRadius = radius * (0.3 + 0.23 * Double(ring))
                let perRing = max(8, dotCount)
                for i in 0..<perRing {
                    let angle = (Double(i) / Double(perRing)) * 2 * .pi
                    let wave = sin(angle * 3 + t * 3 - Double(ring) * 0.6) * radius * 0.08
                    let r = ringRadius + wave
                    let x = cos(angle) * r
                    let y = sin(angle) * r
                    dot(x, y, opacity: 0.6)
                }
            }

        case .connecting:
            let nodeCount = max(6, dotCount / 2)
            var nodes: [(x: Double, y: Double)] = []
            for i in 0..<nodeCount {
                let angle = (Double(i) / Double(nodeCount)) * 2 * .pi + 0.3
                let r = radius * (0.55 + 0.35 * sin(Double(i) * 2.3))
                nodes.append((x: cos(angle) * r, y: sin(angle) * r))
            }
            for i in 0..<nodes.count {
                let a = nodes[i]
                let b = nodes[(i + 2) % nodes.count]
                var path = Path()
                path.move(to: CGPoint(x: CGFloat(cx + a.x), y: CGFloat(cy + a.y)))
                path.addLine(to: CGPoint(x: CGFloat(cx + b.x), y: CGFloat(cy + b.y)))
                context.stroke(path, with: .color(ink.opacity(0.18)), lineWidth: 0.6)
                dot(a.x, a.y, opacity: 0.6)
            }
            let packetProgress = t.truncatingRemainder(dividingBy: 1.4) / 1.4
            let fromIdx = Int(t / 1.4) % nodes.count
            let toIdx = (fromIdx + 2) % nodes.count
            let from = nodes[fromIdx]
            let to = nodes[toIdx]
            dot(from.x + (to.x - from.x) * packetProgress, from.y + (to.y - from.y) * packetProgress, opacity: 1)

        case .weaving:
            let strands = 3
            let pointsPerStrand = max(10, dotCount / strands)
            for strand in 0..<strands {
                let phase = Double(strand) / Double(strands) * 2 * .pi
                for i in 0..<pointsPerStrand {
                    let progress = Double(i) / Double(pointsPerStrand) * 2 * .pi
                    let x = sin(progress + t * 0.8 + phase) * radius * 0.85
                    let y = (progress / (2 * .pi) - 0.5) * radius * 1.7 * cos(t * 0.2)
                    let depthOpacity = 0.4 + 0.5 * cos(progress + t * 0.8 + phase)
                    dot(x, y, opacity: depthOpacity)
                }
            }

        case .composing:
            let bands = 4
            for band in 0..<bands {
                let bandFrac = Double(band) / Double(bands - 1) - 0.5
                let y = bandFrac * radius * 1.5
                let perBand = max(8, dotCount)
                for i in 0..<perBand {
                    let x = (Double(i) / Double(perBand - 1) - 0.5) * radius * 2
                    let undulation = sin(x * 0.15 + t * 1.4 + Double(band) * 0.8) * radius * 0.12
                    dot(x, y + undulation, opacity: 0.55)
                }
            }

        case .breathing:
            let breath = 0.75 + 0.25 * sin(t * 0.9)
            let perRing = max(10, dotCount)
            for i in 0..<perRing {
                let angle = (Double(i) / Double(perRing)) * 2 * .pi + t * 0.15
                let r = radius * breath
                let x = cos(angle) * r
                let y = sin(angle) * r
                dot(x, y, opacity: 0.5 + 0.3 * sin(t * 0.9))
            }

        case .shaping:
            let morphCycle = 3.0
            let phase = t.truncatingRemainder(dividingBy: morphCycle) / morphCycle
            let shapeIndex = Int(t / morphCycle) % 3
            let nextShapeIndex = (shapeIndex + 1) % 3
            let perOutline = max(16, dotCount)
            for i in 0..<perOutline {
                let progress = Double(i) / Double(perOutline)
                let from = outlinePoint(shape: shapeIndex, progress: progress, radius: radius)
                let to = outlinePoint(shape: nextShapeIndex, progress: progress, radius: radius)
                let eased = phase * phase * (3 - 2 * phase)
                let x = from.x + (to.x - from.x) * eased
                let y = from.y + (to.y - from.y) * eased
                dot(x, y, opacity: 0.7)
            }
        }
    }

    private func outlinePoint(shape: Int, progress: Double, radius: Double) -> (x: Double, y: Double) {
        let angle = progress * 2 * .pi
        switch shape {
        case 0: // circle
            return (cos(angle) * radius, sin(angle) * radius)
        case 1: // triangle
            return polygonPoint(sides: 3, angle: angle, radius: radius)
        default: // square
            return polygonPoint(sides: 4, angle: angle, radius: radius)
        }
    }

    private func polygonPoint(sides: Int, angle: Double, radius: Double) -> (x: Double, y: Double) {
        let sector = 2 * Double.pi / Double(sides)
        let sectorAngle = angle.truncatingRemainder(dividingBy: sector)
        let cornerAngle = sector / 2
        let edgeRadius = radius / cos(cornerAngle) * cos(sectorAngle - cornerAngle)
        return (cos(angle) * edgeRadius, sin(angle) * edgeRadius)
    }
}

#Preview {
    LazyVGrid(columns: Array(repeating: GridItem(.flexible()), count: 3)) {
        ForEach(ThinkingOrbState.allCases, id: \.self) { state in
            VStack(spacing: 8) {
                ThinkingOrb(state: state, size: .avatar)
                Text(state.rawValue).font(.caption2)
            }
        }
    }
    .padding()
}
