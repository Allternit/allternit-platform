import SwiftUI

/// Renders content in a fixed W×H coordinate space, scaled to fit (and
/// centred in) the available box — the web FitBox, so the graphs keep the
/// prototype's exact coordinates at any phone size.
struct SetupFitBox<Content: View>: View {
    var width: CGFloat
    var height: CGFloat
    var maxScale: CGFloat = 1.15
    @ViewBuilder var content: Content

    var body: some View {
        GeometryReader { geo in
            let scale = max(0.01, min(geo.size.width / width, geo.size.height / height, maxScale))
            content
                .frame(width: width, height: height, alignment: .topLeading)
                .scaleEffect(scale)
                .frame(width: geo.size.width, height: geo.size.height)
        }
    }
}

/// "Your workspace" — the graph that builds as setup steps complete
/// (allternit-ai `setup/SetupGraph.tsx`; a 694×648 canvas with the A:// core
/// at its centre). Computers appear first; brains join on the Your AI step.
/// Nodes that were actually set up are lit; everything else reads "Available".
struct SetupGraphView: View {
    var state: SetupState
    var orb: AllternitOrbState
    var showBrains: Bool

    private static let w: CGFloat = 694
    private static let h: CGFloat = 648
    private static let center = CGPoint(x: 347, y: 316)
    private static let computersAnchor = CGPoint(x: 120, y: 86)

    private struct BrainNode { let id: BrainChoice; let name: String; let mono: String; let at: CGPoint }
    private static let brainNodes: [BrainNode] = [
        BrainNode(id: .allternit, name: "Allternit Cloud", mono: "A", at: CGPoint(x: 347, y: 188)),
        BrainNode(id: .claude, name: "Claude", mono: "C", at: CGPoint(x: 469, y: 276)),
        BrainNode(id: .chatgpt, name: "ChatGPT", mono: "G", at: CGPoint(x: 422, y: 420)),
        BrainNode(id: .gemini, name: "Gemini", mono: "G", at: CGPoint(x: 272, y: 420)),
        BrainNode(id: .local, name: "Local model", mono: "L", at: CGPoint(x: 225, y: 276)),
    ]
    private static let otherNode = CGPoint(x: 520, y: 180)

    private static func tag(_ status: OutcomeStatus?) -> String {
        switch status ?? .idle {
        case .paired: return "Paired"
        case .detected: return "Detected"
        case .requested: return "Requested"
        case .waiting: return "Waiting"
        case .working: return "Working"
        case .notFound: return "Not found"
        case .error: return "Error"
        default: return "Chosen"
        }
    }

    var body: some View {
        SetupFitBox(width: Self.w, height: Self.h) {
            ZStack(alignment: .topLeading) {
                edges
                computers.position(x: 20 + 104, y: 20 + 62)
                if showBrains { brains }
                core
            }
            .frame(width: Self.w, height: Self.h, alignment: .topLeading)
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Your workspace")
    }

    // MARK: Edges + packets

    private var edges: some View {
        ZStack {
            SetupEdge(from: Self.center, to: Self.computersAnchor, delay: 0)
            if state.choices.where != nil && state.outcomes[.where]?.status == .paired {
                SetupPackets(from: Self.center, to: Self.computersAnchor)
            }
            if showBrains {
                ForEach(Array(Self.brainNodes.enumerated()), id: \.offset) { i, b in
                    SetupEdge(from: Self.center, to: b.at, delay: 0.1 + Double(i) * 0.08)
                }
                if let brain = state.choices.ai, let b = Self.brainNodes.first(where: { $0.id == brain }) {
                    SetupPackets(from: Self.center, to: b.at)
                }
            }
        }
        .frame(width: Self.w, height: Self.h)
    }

    // MARK: Computers card

    private var computers: some View {
        VStack(alignment: .leading, spacing: 7) {
            Text("COMPUTERS")
                .font(SetupPalette.mono(9.5, .semibold))
                .tracking(1.1)
                .foregroundStyle(SetupPalette.mut)
                .padding(.leading, 2)
            VStack(spacing: 5) {
                ForEach(WhereChoice.allCases, id: \.self) { id in
                    let on = state.choices.where == id
                    HStack(spacing: 10) {
                        Text(id.graphName).font(.system(size: 11.5, weight: .semibold))
                        Spacer(minLength: 4)
                        Text(on ? Self.tag(state.outcomes[.where]?.status) : "Available")
                            .font(SetupPalette.mono(9.5))
                            .foregroundStyle(on ? SetupPalette.coral : SetupPalette.mut)
                    }
                    .foregroundStyle(on ? SetupPalette.ink : SetupPalette.sub)
                    .padding(.horizontal, 9)
                    .frame(width: 188, height: 28)
                    .background(SetupPalette.card, in: RoundedRectangle(cornerRadius: 8))
                    .overlay(nodeBorder(on: on, radius: 8))
                    .animation(.easeOut(duration: 0.2), value: on)
                }
            }
        }
        .padding(10)
        .background(SetupPalette.cardGlass, in: RoundedRectangle(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).stroke(SetupPalette.line))
        .shadow(color: .black.opacity(0.06), radius: 10, y: 6)
        .modifier(SetupPopIn(delay: 0))
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Computers")
    }

    // MARK: Brains

    @ViewBuilder
    private var brains: some View {
        ForEach(Array(Self.brainNodes.enumerated()), id: \.offset) { i, b in
            brainChip(mono: b.mono, name: b.name, lit: state.choices.ai == b.id, tag: state.choices.ai == b.id ? "Default" : nil)
                .position(b.at)
                .modifier(SetupPopIn(delay: 0.1 + Double(i) * 0.08))
        }
        if let brain = state.choices.ai, !Self.brainNodes.contains(where: { $0.id == brain }) {
            // Subscription brains, Phone Link and "More brains" join the ring as one extra node.
            let mono = brain.isSubscription ? "S" : brain == .phoneApps ? "P" : "+"
            let name: String = brain.isSubscription ? "\(brain.shortName) sub" : (state.outcomes[.ai]?.label ?? "Other brain")
            // Phone Link isn't a default brain: show its real status instead.
            let tag = brain == .phoneApps ? Self.tag(state.outcomes[.ai]?.status) : "Default"
            brainChip(mono: mono, name: name, lit: true, tag: tag, fixedWidth: false)
                .position(Self.otherNode)
                .modifier(SetupPopIn(delay: 0))
        }
    }

    private func brainChip(mono: String, name: String, lit: Bool, tag: String?, fixedWidth: Bool = true) -> some View {
        HStack(spacing: 7) {
            Text(mono)
                .font(SetupPalette.mono(9, .bold))
                .foregroundStyle(lit ? .white : SetupPalette.sub)
                .frame(width: 16, height: 16)
                .background(lit ? SetupPalette.coral : SetupPalette.fill2, in: RoundedRectangle(cornerRadius: 4))
            Text(name).font(.system(size: 11.5, weight: .semibold)).lineLimit(1)
            if let tag {
                Spacer(minLength: 2)
                Text(tag).font(SetupPalette.mono(9)).foregroundStyle(SetupPalette.coral)
            }
        }
        .foregroundStyle(lit ? SetupPalette.ink : SetupPalette.sub)
        .padding(.horizontal, 9)
        .frame(width: fixedWidth ? 118 : nil, height: 32)
        .frame(minWidth: 118)
        .background(SetupPalette.card, in: RoundedRectangle(cornerRadius: 9))
        .overlay(nodeBorder(on: lit, radius: 9))
        .animation(.easeOut(duration: 0.2), value: lit)
    }

    @ViewBuilder
    private func nodeBorder(on: Bool, radius: CGFloat) -> some View {
        if on {
            RoundedRectangle(cornerRadius: radius).stroke(SetupPalette.ink, lineWidth: 1)
                .background(RoundedRectangle(cornerRadius: radius + 3).stroke(SetupPalette.coralHalo, lineWidth: 3).padding(-2))
        } else {
            RoundedRectangle(cornerRadius: radius).stroke(SetupPalette.lineDash, style: StrokeStyle(lineWidth: 1, dash: [3, 2]))
        }
    }

    // MARK: Core

    private var core: some View {
        ZStack {
            SetupCoreRing(size: 116)
            AllternitOrbView(state: orb, size: 64, label: "A:// is \(orb.rawValue)")
            (Text("A:// ") + Text("·").foregroundColor(SetupPalette.coral) + Text(" \(orb.rawValue)"))
                .font(SetupPalette.mono(10.5, .semibold))
                .tracking(0.8)
                .textCase(.uppercase)
                .foregroundStyle(SetupPalette.mut)
                .frame(width: 220)
                .offset(y: 32 + 20)
        }
        .position(Self.center)
        .modifier(SetupPopIn(delay: 0))
    }
}

/// Node pop-in with a light spring (cubic-bezier(.2,.9,.3,1.3), 500ms).
struct SetupPopIn: ViewModifier {
    var delay: Double
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var shown = false

    func body(content: Content) -> some View {
        content
            .scaleEffect(shown || reduceMotion ? 1 : 0.4)
            .opacity(shown || reduceMotion ? 1 : 0)
            .onAppear {
                guard !reduceMotion else { return }
                withAnimation(.setupPop.delay(delay)) { shown = true }
            }
    }
}

/// An edge from the core that draws itself (900ms), with the coral dashed
/// flow running along it.
struct SetupEdge: View {
    var from: CGPoint
    var to: CGPoint
    var delay: Double
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var drawn: CGFloat = 0

    var body: some View {
        let line = Path { p in p.move(to: from); p.addLine(to: to) }
        ZStack {
            line.trim(from: 0, to: reduceMotion ? 1 : drawn)
                .stroke(SetupPalette.edge, lineWidth: 1.2)
            if reduceMotion {
                line.stroke(SetupPalette.coral.opacity(0.55), style: StrokeStyle(lineWidth: 1.4, dash: [2, 10]))
            } else {
                TimelineView(.animation(minimumInterval: 1.0 / 30)) { tl in
                    let phase = CGFloat(tl.date.timeIntervalSinceReferenceDate.truncatingRemainder(dividingBy: 1)) * -12
                    line.stroke(SetupPalette.coral.opacity(0.55), style: StrokeStyle(lineWidth: 1.4, dash: [2, 10], dashPhase: -phase))
                }
            }
        }
        .allowsHitTesting(false)
        .onAppear {
            guard !reduceMotion else { return }
            withAnimation(.timingCurve(0.3, 0.7, 0.2, 1, duration: 0.9).delay(delay)) { drawn = 1 }
        }
    }
}

/// Two coral packets looping along an edge (2.6s, one reversed). Hidden
/// under Reduce Motion.
struct SetupPackets: View {
    var from: CGPoint
    var to: CGPoint
    var size: CGFloat = 7
    var period: Double = 2.6
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        if !reduceMotion {
            TimelineView(.animation) { tl in
                let t = tl.date.timeIntervalSinceReferenceDate
                ZStack {
                    packet(progress(t, delay: 0.6), forward: true)
                    packet(progress(t, delay: 1.9), forward: false)
                }
            }
            .allowsHitTesting(false)
        }
    }

    private func progress(_ t: Double, delay: Double) -> Double {
        let v = (t - delay).truncatingRemainder(dividingBy: period) / period
        return v < 0 ? v + 1 : v
    }

    private func packet(_ p: Double, forward: Bool) -> some View {
        // cubic-bezier(.5,0,.5,1) ≈ smoothstep; fade at both ends.
        let e = p * p * (3 - 2 * p)
        let k = forward ? e : 1 - e
        let alpha = p < 0.1 ? p / 0.1 : p > 0.9 ? (1 - p) / 0.1 : 1
        return RoundedRectangle(cornerRadius: 2)
            .fill(SetupPalette.coral)
            .frame(width: size, height: size)
            .position(x: from.x + (to.x - from.x) * k, y: from.y + (to.y - from.y) * k)
            .opacity(alpha)
    }
}

/// The expanding coral ring behind the core (2s loop). Hidden under Reduce Motion.
struct SetupCoreRing: View {
    var size: CGFloat
    var period: Double = 2
    var minScale: CGFloat = 0.6
    var maxScale: CGFloat = 1.35
    var lineWidth: CGFloat = 1
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        if !reduceMotion {
            TimelineView(.animation(minimumInterval: 1.0 / 30)) { tl in
                let p = CGFloat(tl.date.timeIntervalSinceReferenceDate.truncatingRemainder(dividingBy: period) / period)
                let e = 1 - (1 - p) * (1 - p)
                Circle()
                    .stroke(SetupPalette.coral.opacity(0.4), lineWidth: lineWidth)
                    .frame(width: size, height: size)
                    .scaleEffect(minScale + (maxScale - minScale) * e)
                    .opacity(Double(1 - e))
            }
            .allowsHitTesting(false)
        }
    }
}
