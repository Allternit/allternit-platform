import SwiftUI

/// The memory map (SPEC-hermes-parity section 4), native. The web draws the
/// same data as columns; on a phone the groups stack top to bottom with the
/// focus card in the middle: Could change, Used by, focus card, Uses. Tap a
/// box to move focus. Forget (memory) and Turn off (skill) name what could
/// change before they act.

// MARK: - Kinds

enum ImpactKind {
    /// The dependency-map palette, exact values from the spec.
    static func hex(_ kind: String) -> String {
        switch kind {
        case "skill": return "#2457c5"
        case "memory": return "#0d8064"
        case "thread": return "#7a48c8"
        case "routine": return "#c4461b"
        case "artifact": return "#b8285c"
        case "soul": return "#9a6a06"
        case "entity": return "#0e7490"
        default: return "#66778a"
        }
    }
    static func color(_ kind: String) -> Color { Color(hex: hex(kind)) }

    static func title(_ kind: String) -> String {
        switch kind {
        case "skill": return "Skill"
        case "memory": return "Memory"
        case "thread": return "Thread"
        case "routine": return "Routine"
        case "artifact": return "Artifact"
        case "soul": return "Soul"
        case "entity": return "Entity"
        default: return kind.capitalized
        }
    }

    static func icon(_ kind: String) -> String {
        switch kind {
        case "skill": return "sparkles"
        case "memory": return "brain"
        case "thread": return "bubble.left.and.bubble.right"
        case "routine": return "clock.arrow.circlepath"
        case "artifact": return "square.grid.2x2"
        case "soul": return "heart.text.square"
        case "entity": return "person.crop.square"
        default: return "circle.grid.2x2"
        }
    }
}

// MARK: - Graph

/// Direction: a link's `from` depends on its `to`.
struct ImpactGraph: Sendable {
    let items: [MemoryMapItem]
    private let byId: [String: MemoryMapItem]
    private let usesOf: [String: [String]]
    private let usedByOf: [String: [String]]

    init(map: MemoryMap) {
        items = map.items
        var index: [String: MemoryMapItem] = [:]
        for item in map.items { index[item.id] = item }
        byId = index
        var uses: [String: [String]] = [:]
        var usedBy: [String: [String]] = [:]
        var seen = Set<String>()
        for link in map.links where index[link.from] != nil && index[link.to] != nil && link.from != link.to {
            guard seen.insert("\(link.from)>\(link.to)").inserted else { continue }
            uses[link.from, default: []].append(link.to)
            usedBy[link.to, default: []].append(link.from)
        }
        usesOf = uses
        usedByOf = usedBy
    }

    func item(_ id: String) -> MemoryMapItem? { byId[id] }

    private func sorted(_ ids: [String]) -> [MemoryMapItem] {
        ids.compactMap { byId[$0] }.sorted {
            if ($0.uses ?? 0) != ($1.uses ?? 0) { return ($0.uses ?? 0) > ($1.uses ?? 0) }
            return $0.label.localizedCaseInsensitiveCompare($1.label) == .orderedAscending
        }
    }

    func uses(_ id: String) -> [MemoryMapItem] { sorted(usesOf[id] ?? []) }
    func usedBy(_ id: String) -> [MemoryMapItem] { sorted(usedByOf[id] ?? []) }

    /// Everything that depends on this item two or more hops away.
    func couldChange(_ id: String) -> [MemoryMapItem] {
        let direct = Set(usedByOf[id] ?? [])
        var seen: Set<String> = direct.union([id])
        var frontier = Array(direct)
        var far: [String] = []
        while !frontier.isEmpty {
            var next: [String] = []
            for node in frontier {
                for parent in usedByOf[node] ?? [] where seen.insert(parent).inserted {
                    far.append(parent)
                    next.append(parent)
                }
            }
            frontier = next
        }
        return sorted(far)
    }

    /// Most recently used; else most used; else the first.
    var defaultFocus: MemoryMapItem? {
        let dated = items.filter { $0.lastAt.flatMap(BotTime.date) != nil }
        if let latest = dated.max(by: { (BotTime.date($0.lastAt ?? "") ?? .distantPast) < (BotTime.date($1.lastAt ?? "") ?? .distantPast) }) {
            return latest
        }
        return items.max { ($0.uses ?? 0) < ($1.uses ?? 0) } ?? items.first
    }
}

// MARK: - View

struct ImpactMapView: View {
    /// nil = the whole account.
    let botId: String?
    /// Returns what Open does for an item, or nil when it cannot be opened.
    var openAction: (MemoryMapItem) -> (() -> Void)? = { _ in nil }
    /// Called after a Forget or Turn off so the host can refresh its list.
    var onChanged: () -> Void = {}

    private enum Phase { case loading, loaded, failed(String) }

    @State private var phase: Phase = .loading
    @State private var unavailable = false
    @State private var graph = ImpactGraph(map: MemoryMap(items: [], links: [], truncated: false))
    @State private var truncated = false
    @State private var focusId: String?
    @State private var expanded: Set<String> = []
    @State private var confirming: MemoryMapItem?
    @State private var busy = false
    @State private var actionError: String?
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private static let groupLimit = 5

    var body: some View {
        // A computer on an older runtime has no map route yet: show nothing.
        if !unavailable { mapContent }
    }

    private var mapContent: some View {
        VStack(alignment: .leading, spacing: 10) {
            header
            switch phase {
            case .loading:
                loadingCard
            case .failed(let message):
                FriendlyInlineStateView(
                    style: .error, icon: "exclamationmark.triangle",
                    title: "Couldn't load the map", message: message,
                    actionTitle: "Retry", action: { Task { await load() } }
                )
            case .loaded:
                if graph.items.isEmpty {
                    emptyCard
                } else if let focus = focusItem {
                    mapStack(focus)
                }
            }
        }
        .task(id: botId) { await load() }
        .confirmationDialog(confirmTitle, isPresented: Binding(
            get: { confirming != nil }, set: { if !$0 { confirming = nil } }
        ), titleVisibility: .visible, presenting: confirming) { item in
            Button(item.kind == "skill" ? "Turn off" : "Forget", role: .destructive) {
                Task { await perform(item) }
            }
            Button("Cancel", role: .cancel) {}
        } message: { item in
            Text(confirmMessage(item))
        }
    }

    // MARK: Pieces

    private var header: some View {
        HStack(spacing: 8) {
            Text("Memory map")
                .font(.headline)
                .foregroundColor(Color("TextPrimary"))
                .accessibilityAddTraits(.isHeader)
            Spacer(minLength: 0)
            if case .loaded = phase, !graph.items.isEmpty {
                Text("\(graph.items.count) items")
                    .font(.caption)
                    .foregroundColor(Color("TextTertiary"))
            }
        }
    }

    private var loadingCard: some View {
        HStack(spacing: 10) {
            ProgressView()
            Text("Mapping what your bots know")
                .font(.subheadline)
                .foregroundColor(Color("TextSecondary"))
        }
        .frame(maxWidth: .infinity, minHeight: 96)
        .background(card)
        .accessibilityElement(children: .combine)
    }

    private var emptyCard: some View {
        VStack(spacing: 6) {
            Image(systemName: "point.3.connected.trianglepath.dotted")
                .font(.title2)
                .foregroundColor(Color("TextTertiary"))
                .accessibilityHidden(true)
            Text("Nothing to map yet")
                .font(.subheadline.weight(.semibold))
                .foregroundColor(Color("TextPrimary"))
            Text("As your bots learn things, use skills and run routines, you will see here what depends on what.")
                .font(.footnote)
                .foregroundColor(Color("TextSecondary"))
                .multilineTextAlignment(.center)
        }
        .padding(18)
        .frame(maxWidth: .infinity)
        .background(card)
        .accessibilityElement(children: .combine)
    }

    private var card: some View {
        RoundedRectangle(cornerRadius: 16, style: .continuous)
            .fill(Color("BgPanel"))
            .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).stroke(Color("BorderSubtle")))
    }

    private var focusItem: MemoryMapItem? {
        if let focusId, let item = graph.item(focusId) { return item }
        return graph.defaultFocus
    }

    private func mapStack(_ focus: MemoryMapItem) -> some View {
        let usedBy = graph.usedBy(focus.id)
        let far = graph.couldChange(focus.id)
        let uses = graph.uses(focus.id)
        return VStack(spacing: 12) {
            if !far.isEmpty { group("Could change", items: far, key: "far") }
            if !usedBy.isEmpty { group("Used by", items: usedBy, key: "by") }
            focusCard(focus, usedBy: usedBy.count, far: far.count, uses: uses.count)
            if !uses.isEmpty { group("Uses", items: uses, key: "uses") }
            if usedBy.isEmpty && uses.isEmpty {
                Text("Nothing is linked to this yet.")
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            if truncated {
                Text("Showing the first 500 items.")
                    .font(.caption)
                    .foregroundColor(Color("TextTertiary"))
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            if let actionError {
                Text(actionError)
                    .font(.footnote)
                    .foregroundColor(Theme.statusError)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .animation(reduceMotion ? nil : .easeOut(duration: 0.18), value: focus.id)
    }

    private func group(_ title: String, items: [MemoryMapItem], key: String) -> some View {
        let open = expanded.contains(key)
        let shown = open ? items : Array(items.prefix(Self.groupLimit))
        return VStack(alignment: .leading, spacing: 6) {
            Text("\(title) · \(items.count)")
                .font(.caption.weight(.semibold))
                .foregroundColor(Color("TextSecondary"))
                .textCase(.uppercase)
                .accessibilityAddTraits(.isHeader)
            VStack(spacing: 6) {
                ForEach(shown) { item in chip(item) }
                if items.count > Self.groupLimit {
                    Button {
                        if open { expanded.remove(key) } else { expanded.insert(key) }
                    } label: {
                        Text(open ? "Show less" : "+\(items.count - Self.groupLimit) more")
                            .font(.footnote.weight(.medium))
                            .foregroundColor(Color("AccentPrimary"))
                            .frame(maxWidth: .infinity, minHeight: 36)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(open ? "Show fewer \(title) items" : "Show \(items.count - Self.groupLimit) more \(title) items")
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func chip(_ item: MemoryMapItem) -> some View {
        Button { focusId = item.id } label: {
            HStack(spacing: 10) {
                RoundedRectangle(cornerRadius: 2).fill(ImpactKind.color(item.kind)).frame(width: 4, height: 22)
                Text(item.label)
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(2)
                    .multilineTextAlignment(.leading)
                Spacer(minLength: 6)
                Text(ImpactKind.title(item.kind))
                    .font(.caption)
                    .foregroundColor(Color("TextTertiary"))
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
            .background(card)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(item.label), \(ImpactKind.title(item.kind))")
        .accessibilityHint("Moves focus to this item")
    }

    private func focusCard(_ item: MemoryMapItem, usedBy: Int, far: Int, uses: Int) -> some View {
        let color = ImpactKind.color(item.kind)
        let open = openAction(item)
        let canAct = item.kind == "memory" || item.kind == "skill"
        return VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Image(systemName: ImpactKind.icon(item.kind))
                    .foregroundColor(color)
                    .accessibilityHidden(true)
                Text(ImpactKind.title(item.kind).uppercased())
                    .font(.caption.weight(.bold))
                    .foregroundColor(color)
                Spacer(minLength: 0)
                if let at = item.lastAt, !at.isEmpty, !BotTime.short(at).isEmpty {
                    Text("Used \(BotTime.short(at)) ago")
                        .font(.caption)
                        .foregroundColor(Color("TextTertiary"))
                }
            }
            Text(item.label)
                .font(.title3.weight(.semibold))
                .foregroundColor(Color("TextPrimary"))
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 14) {
                stat(uses, "uses")
                stat(usedBy, "used by")
                stat(far, "could change")
                if let n = item.uses { stat(n, n == 1 ? "time used" : "times used") }
            }
            if open != nil || canAct {
                HStack(spacing: 10) {
                    if let open {
                        actionButton("Open", icon: "arrow.up.right.square", tint: Color("AccentPrimary"), action: open)
                    }
                    if canAct {
                        actionButton(item.kind == "skill" ? "Turn off" : "Forget",
                                     icon: item.kind == "skill" ? "power" : "trash",
                                     tint: Theme.statusError) { confirming = item }
                    }
                }
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: 16, style: .continuous)
                .fill(color.opacity(0.08))
                .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).stroke(color.opacity(0.55), lineWidth: 1.5))
        )
        .accessibilityElement(children: .contain)
    }

    private func stat(_ value: Int, _ label: String) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("\(value)").font(.subheadline.weight(.semibold)).monospacedDigit().foregroundColor(Color("TextPrimary"))
            Text(label).font(.caption2).foregroundColor(Color("TextSecondary"))
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(value) \(label)")
    }

    private func actionButton(_ title: String, icon: String, tint: Color, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Label(title, systemImage: icon)
                .font(.subheadline.weight(.medium))
                .foregroundColor(tint)
                .padding(.horizontal, 14)
                .frame(minHeight: 40)
                .background(Capsule().fill(Color("BgPanel")))
                .overlay(Capsule().stroke(tint.opacity(0.4)))
        }
        .buttonStyle(.plain)
        .disabled(busy)
    }

    // MARK: Confirm

    private var confirmTitle: String {
        guard let item = confirming else { return "" }
        return item.kind == "skill" ? "Turn off \(item.label)?" : "Forget this?"
    }

    private func confirmMessage(_ item: MemoryMapItem) -> String {
        let affected = graph.usedBy(item.id) + graph.couldChange(item.id)
        let names = affected.prefix(4).map(\.label).joined(separator: ", ")
        let more = affected.count > 4 ? " and \(affected.count - 4) more" : ""
        let verb = item.kind == "skill" ? "Turning it off" : "Forgetting it"
        if affected.isEmpty { return "\(verb) does not change anything else on the map." }
        return "\(verb) could change: \(names)\(more)."
    }

    // MARK: Loading and actions

    private func load() async {
        if case .loaded = phase {} else { phase = .loading }
        do {
            let map = try await HermesClient.shared.memoryMap(botId: botId)
            graph = ImpactGraph(map: map)
            truncated = map.truncated
            if let focusId, graph.item(focusId) == nil { self.focusId = nil }
            phase = .loaded
        } catch is CancellationError {
        } catch APIError.httpError(let status, _) where status == 404 {
            unavailable = true
        } catch {
            phase = .failed(FriendlyErrorMessage.from(error.localizedDescription))
        }
    }

    private func perform(_ item: MemoryMapItem) async {
        busy = true
        actionError = nil
        defer { busy = false }
        do {
            if item.kind == "skill" {
                let skills = try await CapabilitiesClient.shared.scanSkills()
                guard let skill = skills.first(where: { $0.name.caseInsensitiveCompare(item.label) == .orderedSame }) else {
                    actionError = "That skill is not on this computer."
                    return
                }
                try await CapabilitiesClient.shared.setEnabled(skill, enabled: false)
            } else {
                var id = item.ref ?? item.id
                if id.hasPrefix("memory:") { id = String(id.dropFirst("memory:".count)) }
                if id.hasPrefix("fact:") {
                    try await HermesClient.shared.forgetFact(id: String(id.dropFirst("fact:".count)))
                } else {
                    try await BotModeClient.shared.forgetMemory(id: id)
                }
            }
            focusId = nil
            await load()
            onChanged()
        } catch {
            actionError = "Couldn't do that: \(FriendlyErrorMessage.from(error.localizedDescription))"
        }
    }
}
