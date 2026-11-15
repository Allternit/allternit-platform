import SwiftUI

// The Factory graphs, the way the web draws them (lib/factory/layout.ts,
// views/TeamGraph.tsx, views/FlowGraph.tsx): the same input always lands in
// the same place. The web's React Flow pane becomes a pan-and-pinch canvas.

// MARK: - Layout

struct ProjectGraphEdge: Identifiable, Hashable {
    enum Style: Hashable {
        case delegates, checks, directs, blockedBy, onFail
    }

    let id: String
    let from: String
    let to: String
    let style: Style
}

struct ProjectGraphPlaced<Item> {
    let id: String
    let item: Item
    let column: Int
    let row: Int
    let origin: CGPoint
}

struct ProjectGraphLayout<Item> {
    let nodes: [ProjectGraphPlaced<Item>]
    let edges: [ProjectGraphEdge]
    let card: CGSize
    let contentSize: CGSize

    func node(_ id: String) -> ProjectGraphPlaced<Item>? { nodes.first { $0.id == id } }
}

struct ProjectFlowStep: Hashable {
    let card: FactoryNodeCard
    let gateKind: String?
}

enum ProjectGraphLayouts {
    static let teamCard = CGSize(width: 236, height: 132)
    static let teamGap = CGSize(width: 72, height: 28)
    static let flowCard = CGSize(width: 220, height: 92)
    static let flowGap = CGSize(width: 84, height: 24)
    /// Room under the rows for the on-fail routes (the web adds 40 to the pane).
    private static let failRoom: CGFloat = 40

    private static func isCoordinator(_ agent: FactoryAgent) -> Bool {
        (agent.role ?? "").lowercased() == "coordinator"
    }

    /// layoutTeam: the coordinator column first, then each bot one column right
    /// of whoever delegates to (or directs) it, at its shortest depth. `checks`
    /// never moves a bot. Bots no edge reaches sit in the first column after
    /// the coordinators. Rows sort by role, address, id.
    static func team(agents: [FactoryAgent], edges rawEdges: [FactoryTeam.Edge]) -> ProjectGraphLayout<FactoryAgent> {
        let ids = Set(agents.map(\.id))
        let edges = rawEdges
            .filter { ids.contains($0.from) && ids.contains($0.to) && $0.from != $0.to }
            .map { edge -> ProjectGraphEdge in
                let style: ProjectGraphEdge.Style = edge.kind == "checks" ? .checks : edge.kind == "directs" ? .directs : .delegates
                return ProjectGraphEdge(id: "\(edge.kind):\(edge.from)->\(edge.to)", from: edge.from, to: edge.to, style: style)
            }
            .sorted { $0.id < $1.id }

        var down: [String: [String]] = [:]
        var incoming = Set<String>()
        for edge in edges where edge.style != .checks {
            down[edge.from, default: []].append(edge.to)
            incoming.insert(edge.to)
        }
        for key in down.keys { down[key]?.sort() }

        let roots = agents
            .filter { isCoordinator($0) || (!incoming.contains($0.id) && down[$0.id] != nil) }
            .map(\.id)
            .sorted()

        var depth: [String: Int] = [:]
        var queue = roots
        for id in roots { depth[id] = 0 }
        var cursor = 0
        while cursor < queue.count {
            let id = queue[cursor]
            cursor += 1
            let here = depth[id] ?? 0
            for next in down[id] ?? [] where depth[next] == nil {
                depth[next] = here + 1
                queue.append(next)
            }
        }
        let loose = roots.isEmpty ? 0 : 1
        for agent in agents where depth[agent.id] == nil { depth[agent.id] = loose }

        let columns = Dictionary(grouping: agents) { depth[$0.id] ?? 0 }
        let sortedColumns = columns.keys.sorted()
        let tallest = columns.values.map(\.count).max() ?? 0
        let pitchX = teamCard.width + teamGap.width
        let pitchY = teamCard.height + teamGap.height

        var nodes: [ProjectGraphPlaced<FactoryAgent>] = []
        for (index, column) in sortedColumns.enumerated() {
            let list = (columns[column] ?? []).sorted {
                ($0.role ?? "", $0.address, $0.id) < ($1.role ?? "", $1.address, $1.id)
            }
            let offset = CGFloat(tallest - list.count) * pitchY / 2
            for (row, agent) in list.enumerated() {
                nodes.append(ProjectGraphPlaced(id: agent.id, item: agent, column: index, row: row,
                                                origin: CGPoint(x: CGFloat(index) * pitchX, y: offset + CGFloat(row) * pitchY)))
            }
        }
        let size = CGSize(width: max(0, CGFloat(sortedColumns.count) * pitchX - teamGap.width),
                          height: max(0, CGFloat(tallest) * pitchY - teamGap.height))
        return ProjectGraphLayout(nodes: nodes, edges: edges, card: teamCard, contentSize: size)
    }

    /// layoutFlow: left to right by depth, rows by id inside a column. A gate
    /// comes from the node, or from a node waiting on a person (flowFromDag).
    static func flow(_ flow: FactoryFlow) -> ProjectGraphLayout<ProjectFlowStep> {
        let ids = Set(flow.nodes.map(\.nodeId))
        let groups = Dictionary(grouping: flow.nodes) { max(0, $0.depth ?? 0) }
        let sortedDepths = groups.keys.sorted()
        let tallest = groups.values.map(\.count).max() ?? 0
        let pitchX = flowCard.width + flowGap.width
        let pitchY = flowCard.height + flowGap.height

        var nodes: [ProjectGraphPlaced<ProjectFlowStep>] = []
        for (column, depth) in sortedDepths.enumerated() {
            let list = (groups[depth] ?? []).sorted { $0.nodeId < $1.nodeId }
            let offset = CGFloat(tallest - list.count) * pitchY / 2
            for (row, card) in list.enumerated() {
                let gate = card.gate?.kind ?? ((card.needsYou == true || card.status == "needs_you") ? "human" : nil)
                nodes.append(ProjectGraphPlaced(id: card.nodeId, item: ProjectFlowStep(card: card, gateKind: gate), column: column, row: row,
                                                origin: CGPoint(x: CGFloat(column) * pitchX, y: offset + CGFloat(row) * pitchY)))
            }
        }

        var seen = Set<String>()
        let edges = flow.edges
            .filter { ids.contains($0.from) && ids.contains($0.to) }
            .compactMap { edge -> ProjectGraphEdge? in
                let type = edge.type ?? "blocked_by"
                let id = "\(type):\(edge.from)->\(edge.to)"
                guard seen.insert(id).inserted else { return nil }
                return ProjectGraphEdge(id: id, from: edge.from, to: edge.to, style: type == "on_fail" ? .onFail : .blockedBy)
            }
            .sorted { $0.id < $1.id }

        let hasFail = edges.contains { $0.style == .onFail }
        let size = CGSize(width: max(0, CGFloat(sortedDepths.count) * pitchX - flowGap.width),
                          height: max(0, CGFloat(tallest) * pitchY - flowGap.height) + (hasFail ? failRoom : 0))
        return ProjectGraphLayout(nodes: nodes, edges: edges, card: flowCard, contentSize: size)
    }
}

// MARK: - Canvas

/// A surface you drag and pinch, like the web's graph pane: fits the graph on
/// arrival, zooms 0.3–1.5x around the fingers, and stays put across live
/// refreshes. Cards inside use tap gestures, so a drag that starts on a card
/// moves the canvas instead of opening the card.
struct ProjectGraphCanvas<Content: View>: View {
    let contentSize: CGSize
    @ViewBuilder let content: () -> Content

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var scale: CGFloat = 1
    @State private var offset: CGSize = .zero
    @State private var viewport: CGSize = .zero
    @State private var dragBase: CGSize?
    @State private var pinchBase: PinchBase?
    /// Once you've moved it, a resize or refresh no longer refits.
    @State private var touched = false

    private struct PinchBase {
        let scale: CGFloat
        let offset: CGSize
    }

    private let minZoom: CGFloat = 0.3
    private let maxZoom: CGFloat = 1.5
    private let fitInset: CGFloat = 16
    /// The fit never goes below this, so the cards stay readable on a phone;
    /// a graph wider than that starts at its left edge and pans.
    private let minFitZoom: CGFloat = 0.5

    var body: some View {
        GeometryReader { geometry in
            Color.clear
                .overlay(alignment: .topLeading) {
                    content()
                        .frame(width: contentSize.width, height: contentSize.height, alignment: .topLeading)
                        .scaleEffect(scale, anchor: .topLeading)
                        .offset(offset)
                }
                .contentShape(Rectangle())
                .simultaneousGesture(drag)
                .simultaneousGesture(pinch)
                .clipped()
                .overlay(alignment: .bottomTrailing) { controls.padding(10) }
                .onAppear {
                    viewport = geometry.size
                    fit(animated: false)
                }
                .onChange(of: geometry.size) { _, size in
                    viewport = size
                    if !touched { fit(animated: false) }
                }
                .onChange(of: contentSize) { _, _ in
                    if !touched { fit(animated: false) }
                }
        }
    }

    private var drag: some Gesture {
        DragGesture(minimumDistance: 6)
            .onChanged { value in
                guard pinchBase == nil else { return }
                let base = dragBase ?? offset
                dragBase = base
                touched = true
                offset = clamped(CGSize(width: base.width + value.translation.width,
                                        height: base.height + value.translation.height), scale: scale)
            }
            .onEnded { _ in dragBase = nil }
    }

    private var pinch: some Gesture {
        MagnifyGesture()
            .onChanged { value in
                let base = pinchBase ?? PinchBase(scale: scale, offset: offset)
                pinchBase = base
                dragBase = nil
                touched = true
                let next = min(maxZoom, max(minZoom, base.scale * value.magnification))
                offset = clamped(zoomedOffset(from: base.offset, baseScale: base.scale, to: next, around: value.startLocation), scale: next)
                scale = next
            }
            .onEnded { _ in pinchBase = nil }
    }

    /// Keeps the point under `anchor` still while the scale changes.
    private func zoomedOffset(from offset: CGSize, baseScale: CGFloat, to next: CGFloat, around anchor: CGPoint) -> CGSize {
        let ratio = next / baseScale
        return CGSize(width: anchor.x - (anchor.x - offset.width) * ratio,
                      height: anchor.y - (anchor.y - offset.height) * ratio)
    }

    /// Part of the graph always stays on screen, so it can't be lost.
    private func clamped(_ proposed: CGSize, scale: CGFloat) -> CGSize {
        let keep: CGFloat = 80
        let width = contentSize.width * scale
        let height = contentSize.height * scale
        return CGSize(width: min(viewport.width - keep, max(keep - width, proposed.width)),
                      height: min(viewport.height - keep, max(keep - height, proposed.height)))
    }

    private func fit(animated: Bool) {
        guard viewport.width > 0, viewport.height > 0, contentSize.width > 0, contentSize.height > 0 else { return }
        let available = CGSize(width: max(1, viewport.width - fitInset * 2), height: max(1, viewport.height - fitInset * 2))
        let fitted = min(1, max(minFitZoom, min(available.width / contentSize.width, available.height / contentSize.height)))
        let width = contentSize.width * fitted
        let height = contentSize.height * fitted
        let target = CGSize(width: max(fitInset, (viewport.width - width) / 2),
                            height: max(fitInset, (viewport.height - height) / 2))
        withAnimation(animated && !reduceMotion ? .easeOut(duration: 0.2) : nil) {
            scale = fitted
            offset = target
        }
    }

    private func zoom(by factor: CGFloat) {
        let next = min(maxZoom, max(minZoom, scale * factor))
        let center = CGPoint(x: viewport.width / 2, y: viewport.height / 2)
        touched = true
        withAnimation(reduceMotion ? nil : .easeOut(duration: 0.15)) {
            offset = clamped(zoomedOffset(from: offset, baseScale: scale, to: next, around: center), scale: next)
            scale = next
        }
    }

    private var controls: some View {
        HStack(spacing: 0) {
            control("minus", label: "Zoom out") { zoom(by: 0.8) }.disabled(scale <= minZoom)
            control("plus", label: "Zoom in") { zoom(by: 1.25) }.disabled(scale >= maxZoom)
            control("arrow.up.left.and.down.right.magnifyingglass", label: "Fit to screen") {
                touched = false
                fit(animated: true)
            }
        }
        .background(Capsule().fill(Color("BgPanel")))
        .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
        .shadow(color: .black.opacity(0.12), radius: 6, y: 2)
    }

    private func control(_ systemName: String, label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: systemName)
                .font(.system(size: 14, weight: .medium))
                .foregroundColor(Color("TextPrimary"))
                .frame(width: 44, height: 44)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }
}

// MARK: - Edges

/// Forward edges curve from a card's right side to the next one's left; an
/// on-fail route runs back under the cards, bottom to bottom.
private struct ProjectGraphEdgeShape: Shape {
    let from: CGPoint
    let to: CGPoint
    let underneath: Bool

    func path(in rect: CGRect) -> Path {
        var path = Path()
        path.move(to: from)
        if underneath {
            let lane = max(from.y, to.y) + 24
            path.addLine(to: CGPoint(x: from.x, y: lane))
            path.addLine(to: CGPoint(x: to.x, y: lane))
            path.addLine(to: to)
        } else {
            let pull = max(abs(to.x - from.x) / 2, 30)
            path.addCurve(to: to, control1: CGPoint(x: from.x + pull, y: from.y), control2: CGPoint(x: to.x - pull, y: to.y))
        }
        return path
    }
}

/// The arrowhead at the end of an edge, pointing right or (on-fail) up.
private struct ProjectGraphArrowShape: Shape {
    let tip: CGPoint
    let up: Bool

    func path(in rect: CGRect) -> Path {
        var path = Path()
        path.move(to: tip)
        if up {
            path.addLine(to: CGPoint(x: tip.x - 5, y: tip.y + 8))
            path.addLine(to: CGPoint(x: tip.x + 5, y: tip.y + 8))
        } else {
            path.addLine(to: CGPoint(x: tip.x - 8, y: tip.y - 5))
            path.addLine(to: CGPoint(x: tip.x - 8, y: tip.y + 5))
        }
        path.closeSubpath()
        return path
    }
}

struct ProjectGraphEdges<Item>: View {
    let layout: ProjectGraphLayout<Item>
    var arrows = false

    private func stroke(_ style: ProjectGraphEdge.Style) -> StrokeStyle {
        switch style {
        case .delegates, .blockedBy: return StrokeStyle(lineWidth: 1.5)
        case .checks: return StrokeStyle(lineWidth: 1.5, dash: [6, 4])
        case .directs: return StrokeStyle(lineWidth: 1.5, lineCap: .round, dash: [1.5, 4])
        case .onFail: return StrokeStyle(lineWidth: 1.5, dash: [5, 4])
        }
    }

    var body: some View {
        ZStack(alignment: .topLeading) {
            ForEach(layout.edges) { edge in
                if let source = layout.node(edge.from), let target = layout.node(edge.to) {
                    let fail = edge.style == .onFail
                    let tint = fail ? Theme.statusWarning : Color("TextTertiary")
                    let start = fail
                        ? CGPoint(x: source.origin.x + layout.card.width / 2, y: source.origin.y + layout.card.height)
                        : CGPoint(x: source.origin.x + layout.card.width, y: source.origin.y + layout.card.height / 2)
                    let end = fail
                        ? CGPoint(x: target.origin.x + layout.card.width / 2, y: target.origin.y + layout.card.height)
                        : CGPoint(x: target.origin.x, y: target.origin.y + layout.card.height / 2)
                    ProjectGraphEdgeShape(from: start, to: end, underneath: fail)
                        .stroke(tint, style: stroke(edge.style))
                    if arrows {
                        ProjectGraphArrowShape(tip: end, up: fail).fill(tint)
                    }
                    if fail {
                        Text("on fail")
                            .font(.system(size: 11))
                            .foregroundColor(Theme.statusWarning)
                            .padding(.horizontal, 3)
                            .background(Color("BgPanel"))
                            .position(x: (start.x + end.x) / 2, y: max(start.y, end.y) + 24)
                    }
                }
            }
        }
        .frame(width: layout.contentSize.width, height: layout.contentSize.height, alignment: .topLeading)
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }
}

/// "Edge key" under a graph.
struct ProjectGraphLegend: View {
    struct Item: Identifiable {
        let label: String
        let dash: [CGFloat]
        var warning = false
        var id: String { label }
    }

    let items: [Item]
    var showsGate = false

    var body: some View {
        ProjectChipWrap(spacing: 12) {
            ForEach(items) { item in
                HStack(spacing: 6) {
                    Path { path in
                        path.move(to: CGPoint(x: 0, y: 3))
                        path.addLine(to: CGPoint(x: 22, y: 3))
                    }
                    .stroke(item.warning ? Theme.statusWarning : Color("TextTertiary"), style: StrokeStyle(lineWidth: 1.5, dash: item.dash))
                    .frame(width: 22, height: 6)
                    Text(item.label)
                }
            }
            if showsGate {
                HStack(spacing: 4) {
                    Image(systemName: "hand.raised").font(.system(size: 11))
                    Text("human gate")
                }
            }
        }
        .font(.system(size: 11.5))
        .foregroundColor(Color("TextTertiary"))
    }
}

// MARK: - Shared pieces (views/parts.tsx)

enum ProjectFactoryTone {
    case ok, run, warn, err, idle

    static func node(_ status: String) -> ProjectFactoryTone {
        switch status {
        case "working", "checking": return .run
        case "needs_you": return .warn
        case "done": return .ok
        case "failed": return .err
        default: return .idle
        }
    }

    static func agent(_ state: String?) -> ProjectFactoryTone {
        switch state {
        case "working": return .run
        case "needs_you": return .warn
        case "done": return .ok
        case "failed": return .err
        default: return .idle
        }
    }

    var color: Color {
        switch self {
        case .ok: return Theme.statusSuccess
        case .run: return Theme.statusInfo
        case .warn: return Theme.statusWarning
        case .err: return Theme.statusError
        case .idle: return Color("TextTertiary")
        }
    }
}

/// The status pill: a dot in the state's colour on the neutral fill, so the
/// words read in both themes.
struct ProjectFactoryPill: View {
    let label: String
    let tone: ProjectFactoryTone

    var body: some View {
        HStack(spacing: 4) {
            Circle().fill(tone.color).frame(width: 6, height: 6)
            Text(label)
                .font(.system(size: 11))
                .foregroundColor(Color("TextSecondary"))
                .lineLimit(1)
        }
        .padding(.horizontal, 6)
        .padding(.vertical, 1)
        .background(Capsule().fill(Color("FillNeutral")))
    }
}

/// `proof k/n`, or `proof —` when nothing has a proof contract yet.
struct ProjectProofCount: View {
    let proof: FactoryProof?

    var body: some View {
        Group {
            if let proof, proof.total > 0 {
                Text("proof \(proof.proven)/\(proof.total)")
                    .foregroundColor(Color("TextSecondary"))
                    .accessibilityLabel("\(proof.proven) of \(proof.total) proven")
            } else {
                Text("proof —").foregroundColor(Color("TextTertiary"))
            }
        }
        .font(.system(size: 11.5).monospacedDigit())
        .lineLimit(1)
    }
}

/// A bot as a figure standing on the canvas: its real avatar, on its own
/// shadow, with the working / needs-you dot. Falls back to the same generated
/// face the rest of the app gives a bot it can't resolve yet.
struct ProjectBotFigure: View {
    let botId: String
    var size: CGFloat = 40
    var state: String? = nil

    @ObservedObject private var store = BotModeStore.shared

    var body: some View {
        ZStack(alignment: .bottom) {
            Ellipse()
                .fill(Color.black.opacity(0.22))
                .frame(width: size * 0.78, height: size * 0.16)
                .blur(radius: 1.5)
                .offset(y: size * 0.08)
            face
                .shadow(color: .black.opacity(0.18), radius: 2, y: 1.5)
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }

    @ViewBuilder private var face: some View {
        if let bot = store.bot(botId) {
            BotStatusAvatar(bot: bot, size: size, working: state == "working", waiting: state == "needs_you", tiled: true)
        } else {
            ZStack {
                RoundedRectangle(cornerRadius: size * 0.325, style: .continuous).fill(Color("BgSecondary"))
                AgentIdenticonView(seed: botId, size: size * 0.62)
            }
            .frame(width: size, height: size)
        }
    }
}

/// A wrapping row of small chips.
struct ProjectChipWrap: Layout {
    var spacing: CGFloat = 6

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        arrange(width: proposal.width ?? .infinity, subviews: subviews).size
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        let places = arrange(width: bounds.width, subviews: subviews).places
        for (index, place) in places.enumerated() {
            subviews[index].place(at: CGPoint(x: bounds.minX + place.x, y: bounds.minY + place.y), proposal: .unspecified)
        }
    }

    private func arrange(width: CGFloat, subviews: Subviews) -> (places: [CGPoint], size: CGSize) {
        var places: [CGPoint] = []
        var x: CGFloat = 0
        var y: CGFloat = 0
        var rowHeight: CGFloat = 0
        var widest: CGFloat = 0
        for subview in subviews {
            let size = subview.sizeThatFits(.unspecified)
            if x > 0, x + size.width > width {
                x = 0
                y += rowHeight + spacing
                rowHeight = 0
            }
            places.append(CGPoint(x: x, y: y))
            x += size.width + spacing
            widest = max(widest, x - spacing)
            rowHeight = max(rowHeight, size.height)
        }
        return (places, CGSize(width: widest, height: y + rowHeight))
    }
}

/// Graph · List, Lanes · Graph · List (TeamViewSwitch).
struct ProjectViewSwitch<Option: Hashable>: View {
    let options: [Option]
    @Binding var selection: Option
    let label: (Option) -> String

    var body: some View {
        HStack(spacing: 2) {
            ForEach(options, id: \.self) { option in
                Button { selection = option } label: {
                    Text(label(option))
                        .font(.system(size: 12))
                        .foregroundColor(selection == option ? Color("TextPrimary") : Color("TextTertiary"))
                        .padding(.horizontal, 10)
                        .frame(minHeight: 36)
                        .background(RoundedRectangle(cornerRadius: 5).fill(selection == option ? Color("BgPanel") : Color.clear))
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(selection == option ? .isSelected : [])
            }
        }
        .padding(2)
        .background(RoundedRectangle(cornerRadius: 7).fill(Color("FillNeutral")))
    }
}

// MARK: - Factory states (FactorySkeleton, FactoryEmpty, FactoryQueryError)

struct ProjectFactoryFailure: Equatable {
    let fact: String
    let action: String

    /// errorFromResponse: no engine behind the route reads as the engine
    /// being off, never as empty data.
    init(_ error: Error) {
        switch error as? APIError {
        case .httpError(let code, _)? where code == 401:
            fact = "You are signed out."
            action = "Sign in again, then retry."
        case .httpError(let code, _)? where code != 404 && code != 502 && code != 503:
            fact = "The Factory answered \(code)."
            action = "Try again."
        case .decoding?, .invalidResponse?:
            fact = "Couldn't load this from the Factory."
            action = "Try again."
        default:
            fact = "The Factory engine isn't running on this computer."
            action = "Start Allternit Desktop or Gizzi on this computer, then try again."
        }
    }
}

struct ProjectFactoryFailureView: View {
    let failure: ProjectFactoryFailure
    let retry: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(failure.fact)
                .font(.system(size: 13, weight: .medium))
                .foregroundColor(Color("TextPrimary"))
            Text(failure.action)
                .font(.system(size: 12.5))
                .foregroundColor(Color("TextSecondary"))
            Button("Try again", action: retry)
                .font(.system(size: 12.5))
                .foregroundColor(Color("TextPrimary"))
                .frame(minHeight: 44)
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Color("BorderSubtle"), style: StrokeStyle(lineWidth: 1, dash: [4])))
        .fixedSize(horizontal: false, vertical: true)
    }
}

struct ProjectFactoryEmptyView: View {
    let title: String
    var detail: String? = nil

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title)
                .font(.system(size: 13))
                .foregroundColor(Color("TextPrimary"))
            if let detail {
                Text(detail)
                    .font(.system(size: 12.5))
                    .foregroundColor(Color("TextSecondary"))
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Color("BorderSubtle"), style: StrokeStyle(lineWidth: 1, dash: [4])))
        .fixedSize(horizontal: false, vertical: true)
    }
}

struct ProjectFactorySkeleton: View {
    var rows = 3
    let label: String

    var body: some View {
        VStack(spacing: 8) {
            ForEach(0..<rows, id: \.self) { index in
                RoundedRectangle(cornerRadius: 10)
                    .fill(Color("FillNeutral"))
                    .frame(height: 56)
                    .opacity(1 - Double(index) * 0.18)
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(label)
    }
}
