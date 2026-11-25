import SwiftUI

/// Pages inside the Team map sheet.
enum TeamMapRoute: Hashable {
    /// A person or bot (`p:` / `b:` / `v:` id).
    case node(String)
    /// The one-sentence rule editor, scoped to "org" or a node id.
    case newRule(scope: String)
}

/// The Team map (PLAN §2, mockup B-team-map-phones): people and bots as a
/// tree with reports-to and owns edges, amber where a bot waits on someone,
/// the asks waiting and the rules in force. Opened as a sheet from the Office
/// floor's menu, an approval row, or a bot project page; no rail tab. Tap a
/// node for its levels, reporting line and rules (`TeamNodeDetailView`).
/// Behaviour copies ai `OfficeTeamMapSheet` (#1316) and the editors (#1346).
struct TeamMapSheet: View {
    /// Open straight on this node (bot page → its node, approval → approver).
    var focus: String?
    /// The 0-bot empty state's "Add a bot" (nil hides the button).
    var onAddBot: (() -> Void)?

    @StateObject private var store = TeamMapStore()
    @State private var path: [TeamMapRoute] = []
    @State private var focused = false
    @State private var ruleError: String?
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack(path: $path) {
            content
                .navigationTitle("Team map")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .topBarTrailing) {
                        Button { dismiss() } label: {
                            Image(systemName: "xmark")
                                .font(.footnote.weight(.semibold))
                                .frame(width: 30, height: 30)
                                .background(Circle().fill(Color("FillNeutral")))
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Close")
                    }
                }
                .navigationDestination(for: TeamMapRoute.self) { route in
                    switch route {
                    case .node(let id):
                        TeamNodeDetailView(store: store, nodeId: id, path: $path)
                    case .newRule(let scope):
                        TeamRuleEditorView(store: store, scope: scope)
                    }
                }
        }
        .background(Color("BgPanel"))
        .task {
            await store.load()
            if !focused, let id = focusedNodeId {
                focused = true
                path = [.node(id)]
            }
        }
    }

    @ViewBuilder
    private var content: some View {
        switch store.phase {
        case .loading:
            ProgressView("Loading the team map…")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .failed(let error):
            TeamMapFailedView(error: error) { Task { await store.load() } }
        case .loaded:
            if let map = store.map {
                if map.botCount == 0 {
                    TeamMapEmptyView(map: map, onAddBot: addBotAction)
                } else {
                    loaded(map)
                }
            }
        }
    }

    /// `focus` on the map; a bot page passes `b:<id>`, which is `v:<id>` for a connected bot.
    private var focusedNodeId: String? {
        guard let focus, let map = store.map else { return nil }
        if map.node(focus) != nil { return focus }
        if focus.hasPrefix("b:"), map.node("v:" + focus.dropFirst(2)) != nil { return "v:" + focus.dropFirst(2) }
        return nil
    }

    /// Closes the sheet first, so the caller can present its own bot setup.
    private var addBotAction: (() -> Void)? {
        guard let onAddBot else { return nil }
        return {
            dismiss()
            onAddBot()
        }
    }

    private func loaded(_ map: TeamMap) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                header(map)
                ScrollView(.horizontal, showsIndicators: false) {
                    TeamTreeView(map: map) { path.append(.node($0)) }
                        .padding(.horizontal, 16)
                        .padding(.vertical, 12)
                }
                Text("Circles are people, squares are bots. Amber dashed lines wait on someone.")
                    .font(.caption)
                    .foregroundStyle(Color("TextSecondary"))
                    .padding(.horizontal, 16)
                asks(map)
                rules(map)
                if map.jobs.isEmpty {
                    Text("Jobs not available yet.")
                        .font(.caption)
                        .foregroundStyle(Color("TextTertiary"))
                        .padding(.horizontal, 16)
                }
            }
            .padding(.bottom, 24)
        }
        .refreshable { await store.load() }
    }

    private func header(_ map: TeamMap) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("\(map.org?.name ?? "Just you") · \(TeamPlural.count(map.peopleCount, "person", "people")) · \(TeamPlural.count(map.botCount, "bot"))")
                .font(.subheadline)
                .foregroundStyle(Color("TextSecondary"))
            if map.waitingCount > 0 {
                Label("\(map.waitingCount) waiting", systemImage: "hourglass")
                    .font(.footnote.weight(.semibold))
                    .foregroundStyle(TeamMapPalette.waiting)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 4)
                    .background(Capsule().fill(TeamMapPalette.waiting.opacity(0.14)))
            }
            if let note = TeamPartial.note(map.partial) {
                Text(note)
                    .font(.caption)
                    .foregroundStyle(Color("TextTertiary"))
            }
        }
        .padding(.horizontal, 16)
        .padding(.top, 8)
    }

    @ViewBuilder
    private func asks(_ map: TeamMap) -> some View {
        if !map.asks.isEmpty {
            VStack(alignment: .leading, spacing: 6) {
                Text("Asks (\(map.asks.count) waiting)")
                    .font(.footnote)
                    .foregroundStyle(Color("TextSecondary"))
                ForEach(map.asks) { ask in
                    TeamAskRow(ask: ask, map: map,
                               onDecide: { await store.decide(ask, approve: $0) },
                               onWhoApproves: ask.approverUserId.map { user in { path.append(.node("p:\(user)")) } })
                        .padding(12)
                        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Color("FillNeutral").opacity(0.6)))
                }
            }
            .padding(.horizontal, 16)
        }
    }

    private func rules(_ map: TeamMap) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text(TeamPlural.count(map.rules.count, "rule"))
                    .font(.footnote)
                    .foregroundStyle(Color("TextSecondary"))
                Spacer()
                if map.viewerIsAdmin {
                    Button { path.append(.newRule(scope: "org")) } label: {
                        Label("New rule", systemImage: "plus")
                            .font(.footnote.weight(.semibold))
                            .frame(minHeight: 44)
                    }
                }
            }
            if map.rules.isEmpty {
                Text("No rules yet. Each bot's level decides what it may do.")
                    .font(.footnote)
                    .foregroundStyle(Color("TextTertiary"))
            }
            ForEach(map.rules) { rule in
                TeamRuleRow(rule: rule, onDelete: map.viewerIsAdmin ? { deleteRule(rule) } : nil)
            }
            if let ruleError {
                Text(ruleError).font(.footnote).foregroundStyle(.red)
            }
        }
        .padding(.horizontal, 16)
    }

    private func deleteRule(_ rule: TeamRule) {
        ruleError = nil
        Task { ruleError = await store.deleteRule(id: rule.id) }
    }
}

// MARK: - Tree

/// Where each node sits, so the edges can be drawn behind them.
private struct TeamNodeAnchorKey: PreferenceKey {
    static var defaultValue: [String: Anchor<CGRect>] { [:] }
    static func reduce(value: inout [String: Anchor<CGRect>], nextValue: () -> [String: Anchor<CGRect>]) {
        value.merge(nextValue()) { $1 }
    }
}

/// A top-down tree: each subtree as wide as its leaves, parents centred over
/// their children (web `layoutTree`), built from stacks so it grows with
/// Dynamic Type. Edges are drawn from the nodes' anchors.
struct TeamTreeView: View {
    let map: TeamMap
    let onOpen: (String) -> Void

    var body: some View {
        HStack(alignment: .top, spacing: 20) {
            ForEach(map.roots) { root in
                TeamSubtree(map: map, node: root, depth: 0, onOpen: onOpen)
            }
        }
        .backgroundPreferenceValue(TeamNodeAnchorKey.self) { anchors in
            GeometryReader { proxy in
                edges(anchors: anchors, proxy: proxy)
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Team map")
    }

    private func edges(anchors: [String: Anchor<CGRect>], proxy: GeometryProxy) -> some View {
        let rects = anchors.mapValues { proxy[$0] }
        var tree = Path()
        var waiting = Path()
        for node in map.nodes {
            guard let child = rects[node.id] else { continue }
            let waitsOn = node.isBot ? map.waitingOn(node.id) : nil
            if let parentId = map.parent(of: node.id), let parent = rects[parentId] {
                var elbow = Path()
                let start = CGPoint(x: parent.midX, y: parent.maxY + 4)
                let end = CGPoint(x: child.midX, y: child.minY - 4)
                let midY = (start.y + end.y) / 2
                elbow.move(to: start)
                elbow.addLine(to: CGPoint(x: start.x, y: midY))
                elbow.addLine(to: CGPoint(x: end.x, y: midY))
                elbow.addLine(to: end)
                if waitsOn == parentId { waiting.addPath(elbow) } else { tree.addPath(elbow) }
            }
            if let waitsOn, waitsOn != map.parent(of: node.id), let target = rects[waitsOn] {
                waiting.move(to: CGPoint(x: child.midX, y: child.minY))
                waiting.addLine(to: CGPoint(x: target.midX, y: target.maxY))
            }
        }
        return ZStack {
            tree.stroke(TeamMapPalette.line, lineWidth: 1.5)
            waiting.stroke(TeamMapPalette.waiting, style: StrokeStyle(lineWidth: 2, dash: [5, 4]))
        }
        .accessibilityHidden(true)
    }
}

private struct TeamSubtree: View {
    let map: TeamMap
    let node: TeamNode
    let depth: Int
    let onOpen: (String) -> Void

    var body: some View {
        let kids = depth < 12 ? map.children(of: node.id) : []
        VStack(spacing: 28) {
            TeamNodeButton(map: map, node: node) { onOpen(node.id) }
                .anchorPreference(key: TeamNodeAnchorKey.self, value: .bounds) { [node.id: $0] }
            if !kids.isEmpty {
                HStack(alignment: .top, spacing: 12) {
                    ForEach(kids) { kid in
                        AnyView(TeamSubtree(map: map, node: kid, depth: depth + 1, onOpen: onOpen))
                    }
                }
            }
        }
    }
}

/// One node: avatar, name, subtitle (role, or bot kind · brain) and state.
struct TeamNodeButton: View {
    let map: TeamMap
    let node: TeamNode
    let action: () -> Void
    @ScaledMetric(relativeTo: .caption) private var width: CGFloat = 92

    var body: some View {
        let waitingOnMe = map.waitingOnMe(node.id).count
        let badge = node.isBot ? node.counts.asks : max(waitingOnMe, map.asks(for: node).filter { $0.actor != node.id }.count)
        let waits = node.isBot && map.waitingOn(node.id) != nil
        let state = map.stateText(node)
        Button(action: action) {
            VStack(spacing: 3) {
                TeamNodeAvatar(node: node, ring: TeamMapPalette.ring(node.state, waiting: waits || waitingOnMe > 0), badge: badge)
                Text(node.id == map.me ? "\(node.name) (you)" : node.name)
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(Color("TextPrimary"))
                    .lineLimit(2)
                    .multilineTextAlignment(.center)
                Text(node.subtitle)
                    .font(.caption2)
                    .foregroundStyle(Color("TextSecondary"))
                    .lineLimit(2)
                    .multilineTextAlignment(.center)
                if node.isBot || node.state != "idle" {
                    Text(state)
                        .font(.caption2.weight(.medium))
                        .foregroundStyle(waits || node.state == "needs_input" ? TeamMapPalette.waiting
                                         : (node.state == "working" ? TeamMapPalette.working : Color("TextTertiary")))
                        .lineLimit(2)
                        .multilineTextAlignment(.center)
                }
            }
            .frame(width: width)
            .padding(.vertical, 4)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(accessibilityText(state: state, waitingOnMe: waitingOnMe))
        .accessibilityHint("Shows levels, who they report to, and rules")
    }

    private func accessibilityText(state: String, waitingOnMe: Int) -> String {
        var parts = [node.id == map.me ? "\(node.name), you" : node.name, node.subtitle, state]
        if waitingOnMe > 0 { parts.append("\(waitingOnMe) waiting on them") }
        return parts.joined(separator: ", ")
    }
}

// MARK: - States

/// No computer / signed out / team store down: one plain sentence and a retry.
struct TeamMapFailedView: View {
    let error: TeamMapError
    let retry: () -> Void

    var body: some View {
        ContentUnavailableView {
            Label(title, systemImage: symbol)
        } description: {
            Text(error.errorDescription ?? "")
        } actions: {
            if error != .signedOut {
                Button("Try again", action: retry)
                    .buttonStyle(.borderedProminent)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var title: String {
        switch error {
        case .noRuntime: return "Connect a computer to see your team"
        case .signedOut: return "Sign in to see your team"
        default: return "Couldn't load the team map"
        }
    }

    private var symbol: String {
        switch error {
        case .noRuntime: return "desktopcomputer.trianglebadge.exclamationmark"
        case .signedOut: return "person.crop.circle.badge.questionmark"
        default: return "exclamationmark.triangle"
        }
    }
}

/// A 0-bot account: the viewer alone, a dashed "Add a bot" slot under them.
struct TeamMapEmptyView: View {
    let map: TeamMap
    var onAddBot: (() -> Void)?

    var body: some View {
        let me = map.viewer ?? TeamNode(id: map.me, kind: "person", name: "You", role: "owner", isMain: nil,
                                        brain: nil, state: "idle", counts: .init(asks: 0, jobs: 0))
        let others = map.peopleCount - (map.viewer == nil ? 0 : 1)
        ScrollView {
            VStack(spacing: 14) {
                VStack(spacing: 4) {
                    TeamNodeAvatar(node: me)
                    Text("You").font(.subheadline.weight(.semibold)).foregroundStyle(Color("TextPrimary"))
                    Text(me.subtitle).font(.caption).foregroundStyle(Color("TextSecondary"))
                }
                .accessibilityElement(children: .combine)
                Rectangle().fill(TeamMapPalette.line).frame(width: 1.5, height: 24).accessibilityHidden(true)
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .strokeBorder(Color("BorderStrong"), style: StrokeStyle(lineWidth: 1.5, dash: [4, 3]))
                    .frame(width: 44, height: 44)
                    .overlay(Image(systemName: "plus").foregroundStyle(Color("TextTertiary")))
                    .accessibilityHidden(true)
                Text(others > 0 ? "No bots yet: add one to see the map" : "Just you: add a bot to see the map")
                    .font(.headline)
                    .foregroundStyle(Color("TextPrimary"))
                    .multilineTextAlignment(.center)
                Text("Bots you create or connect appear under you, with the apps they can use and who approves what.")
                    .font(.subheadline)
                    .foregroundStyle(Color("TextSecondary"))
                    .multilineTextAlignment(.center)
                if let onAddBot {
                    Button("Add a bot", action: onAddBot)
                        .buttonStyle(.borderedProminent)
                }
            }
            .padding(24)
            .frame(maxWidth: .infinity)
        }
    }
}
