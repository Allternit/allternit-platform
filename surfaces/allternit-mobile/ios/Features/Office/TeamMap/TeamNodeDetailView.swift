import SwiftUI

/// A person or bot on the Team map: state, the apps a bot may use and at
/// which level (Browser · App · Full, capped by the ceiling), who a person
/// reports to, the asks, and the rules about them with "New rule".
/// Mirrors ai `TeamMapDetail` + `TeamLevelPicker` + `ReportsToPicker`.
struct TeamNodeDetailView: View {
    @ObservedObject var store: TeamMapStore
    let nodeId: String
    @Binding var path: [TeamMapRoute]

    @State private var reportsError: String?
    @State private var savingReports = false
    @State private var ruleError: String?

    var body: some View {
        Group {
            if let map = store.map, let node = map.node(nodeId) {
                List {
                    summary(map, node)
                    if node.isBot {
                        levels(map, node)
                    } else {
                        reportsTo(map, node)
                    }
                    asks(map, node)
                    rules(map, node)
                }
                .listStyle(.insetGrouped)
                .refreshable { await store.load() }
                .navigationTitle(node.name)
            } else {
                ContentUnavailableView("Not on the team map", systemImage: "person.crop.circle.badge.questionmark",
                                       description: Text("This person or bot isn't on the map anymore."))
            }
        }
        .navigationBarTitleDisplayMode(.inline)
    }

    // MARK: Sections

    private func summary(_ map: TeamMap, _ node: TeamNode) -> some View {
        Section {
            HStack(spacing: 12) {
                TeamNodeAvatar(node: node, ring: TeamMapPalette.ring(node.state, waiting: map.waitingOn(node.id) != nil))
                VStack(alignment: .leading, spacing: 2) {
                    Text(node.id == map.me ? "\(node.name) (you)" : node.name)
                        .font(.headline)
                        .foregroundStyle(Color("TextPrimary"))
                    Text(node.subtitle)
                        .font(.subheadline)
                        .foregroundStyle(Color("TextSecondary"))
                    Text(map.stateText(node))
                        .font(.footnote)
                        .foregroundStyle(map.waitingOn(node.id) != nil ? TeamMapPalette.waiting : Color("TextSecondary"))
                }
            }
            .accessibilityElement(children: .combine)
            if node.isBot, let owner = map.owner(of: node.id), let ownerNode = map.node(owner) {
                Button { path.append(.node(owner)) } label: {
                    LabeledContent("Owner", value: ownerNode.name)
                }
                .accessibilityHint("Opens \(ownerNode.name) on the team map")
            }
            let waiting = map.waitingOnMe(node.id)
            if !waiting.isEmpty {
                Text("\(waiting.map(\.name).joined(separator: ", ")) \(waiting.count == 1 ? "is" : "are") waiting on \(node.id == map.me ? "you" : node.name).")
                    .font(.footnote)
                    .foregroundStyle(TeamMapPalette.waiting)
            }
        }
    }

    @ViewBuilder
    private func levels(_ map: TeamMap, _ node: TeamNode) -> some View {
        let grants = map.grants(for: node.id)
        Section {
            if grants.isEmpty {
                Text("No apps yet. Give this bot an app from its settings.")
                    .font(.footnote)
                    .foregroundStyle(Color("TextSecondary"))
            }
            ForEach(grants) { grant in
                TeamLevelPicker(store: store, grant: grant, canEdit: map.canEditLevels(of: node.id))
            }
        } header: {
            Text("Apps and levels")
        } footer: {
            Text("Browser opens the app and reads. App calls its tools; changes follow the rules. Full runs long tasks, and each call still asks to confirm.")
        }
    }

    @ViewBuilder
    private func reportsTo(_ map: TeamMap, _ node: TeamNode) -> some View {
        let current = map.parent(of: node.id)
        let canEdit = map.org != nil && map.viewerIsAdmin
        let choices = map.people.filter { $0.id != node.id }
        Section {
            if canEdit {
                Picker("Reports to", selection: Binding(
                    get: { current ?? "" },
                    set: { setManager(node, to: $0.isEmpty ? nil : $0) }
                )) {
                    Text("No one (top of the team)").tag("")
                    ForEach(choices) { person in
                        Text(person.name).tag(person.id)
                    }
                }
                .disabled(savingReports)
            } else {
                LabeledContent("Reports to", value: map.name(current) ?? "No one")
            }
            if let reportsError {
                Text(reportsError).font(.footnote).foregroundStyle(.red)
            }
        } header: {
            Text("Reporting line")
        } footer: {
            if map.org == nil {
                Text("Make a team to set who reports to whom.")
            } else if !canEdit {
                Text("Only the owner or an admin can change this.")
            } else {
                Text("\"The manager\" in a rule means this person's manager.")
            }
        }
    }

    @ViewBuilder
    private func asks(_ map: TeamMap, _ node: TeamNode) -> some View {
        let list = map.asks(for: node)
        if !list.isEmpty {
            Section("Asks (\(list.count) waiting)") {
                ForEach(list) { ask in
                    TeamAskRow(ask: ask, map: map, onDecide: { await store.decide(ask, approve: $0) },
                               onWhoApproves: whoApproves(ask, map: map))
                }
            }
        }
    }

    private func rules(_ map: TeamMap, _ node: TeamNode) -> some View {
        let list = map.rules(for: node)
        return Section {
            if list.isEmpty {
                Text("No rules about \(node.id == map.me ? "you" : node.name) yet.")
                    .font(.footnote)
                    .foregroundStyle(Color("TextSecondary"))
            }
            ForEach(list) { rule in
                TeamRuleRow(rule: rule, onDelete: map.viewerIsAdmin ? { deleteRule(rule) } : nil)
            }
            if let ruleError {
                Text(ruleError).font(.footnote).foregroundStyle(.red)
            }
            if map.viewerIsAdmin {
                Button { path.append(.newRule(scope: node.id)) } label: {
                    Label("New rule", systemImage: "plus")
                }
            }
        } header: {
            Text("\(node.name) · \(TeamPlural.count(map.rules(inScope: node.id).count, "rule")) today")
        } footer: {
            Text(map.viewerIsAdmin ? "Rules win over the level." : "Only the owner or an admin can change rules.")
        }
    }

    // MARK: Actions

    private func whoApproves(_ ask: TeamAsk, map: TeamMap) -> (() -> Void)? {
        guard let user = ask.approverUserId, "p:\(user)" != nodeId, map.node("p:\(user)") != nil else { return nil }
        return { path.append(.node("p:\(user)")) }
    }

    private func setManager(_ node: TeamNode, to managerId: String?) {
        guard let userId = node.userId else { return }
        let managerUserId = managerId.flatMap { $0.hasPrefix("p:") ? String($0.dropFirst(2)) : nil }
        savingReports = true
        reportsError = nil
        Task {
            reportsError = await store.setReportsTo(userId: userId, managerUserId: managerUserId)
            savingReports = false
        }
    }

    private func deleteRule(_ rule: TeamRule) {
        ruleError = nil
        Task { ruleError = await store.deleteRule(id: rule.id) }
    }
}

/// One app's level for a bot: Browser · App · Full, levels above the ceiling
/// locked with the reason under them (ai `TeamLevelPicker` wording).
struct TeamLevelPicker: View {
    @ObservedObject var store: TeamMapStore
    let grant: TeamGrant
    let canEdit: Bool

    @State private var saving: String?
    @State private var error: String?

    private struct Level: Identifiable {
        let id: String
        let desc: String
    }

    private static let levels: [Level] = [
        Level(id: "browser", desc: "Open the app and read. No changes."),
        Level(id: "app", desc: "Call its tools. Changes follow the rules."),
        Level(id: "full", desc: "Long tasks. Each call still asks to confirm."),
    ]

    private static func rank(_ level: String) -> Int {
        switch level {
        case "browser": return 0
        case "app": return 1
        case "full": return 2
        default: return -1
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Image(systemName: "square.grid.2x2").foregroundStyle(Color("TextSecondary")).accessibilityHidden(true)
                Text(grant.app.displayName)
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(Color("TextPrimary"))
                Spacer()
                Text(TeamLevel.label(grant.level))
                    .font(.footnote)
                    .foregroundStyle(Color("TextSecondary"))
            }
            .accessibilityElement(children: .combine)
            HStack(spacing: 6) {
                ForEach(Self.levels) { level in
                    levelButton(level.id, desc: level.desc)
                }
            }
            if let reason = ceilingReason {
                Text(reason)
                    .font(.caption)
                    .foregroundStyle(Color("TextSecondary"))
                    .fixedSize(horizontal: false, vertical: true)
            }
            if !canEdit {
                Text("Only the bot's owner or an admin can change its level.")
                    .font(.caption)
                    .foregroundStyle(Color("TextTertiary"))
            }
            if let error {
                Text(error).font(.footnote).foregroundStyle(.red).fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(.vertical, 4)
    }

    private func allowed(_ level: String) -> Bool {
        guard let ceiling = grant.ceiling else { return false }
        return Self.rank(level) <= Self.rank(ceiling)
    }

    private func levelButton(_ level: String, desc: String) -> some View {
        let on = grant.level == level
        let ok = allowed(level)
        return Button {
            save(level)
        } label: {
            HStack(spacing: 4) {
                if saving == level { ProgressView().controlSize(.mini) }
                Text(TeamLevel.label(level)).font(.footnote.weight(on ? .semibold : .regular))
                if !ok { Image(systemName: "lock.fill").font(.caption2) }
            }
            .frame(maxWidth: .infinity, minHeight: 44)
            .foregroundStyle(on ? Color("OnAccent") : (ok ? Color("TextPrimary") : Color("TextTertiary")))
            .background(RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(on ? Color("AccentPrimary") : Color("FillNeutral")))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!canEdit || !ok || on || saving != nil)
        .accessibilityLabel("\(TeamLevel.label(level)). \(desc)\(ok ? "" : " Locked.")")
        .accessibilityAddTraits(on ? .isSelected : [])
    }

    /// Why a level is out of reach (ai `ceilingReason`).
    private var ceilingReason: String? {
        let app = grant.app.displayName
        guard let ceiling = grant.ceiling else {
            return "\(app) is not installed for this bot's owner, so its level cannot change."
        }
        guard ceiling != "full" else { return nil }
        let top = TeamLevel.label(ceiling)
        return grant.app.kind == "vendor"
            ? "Ceiling: \(top). \(app) is a vendor app, and vendor apps stop at \(top)."
            : "Ceiling: \(top). The owner's plan allows \(app) up to \(top)."
    }

    private func save(_ level: String) {
        saving = level
        error = nil
        Task {
            error = await store.setLevel(grant, to: level)
            saving = nil
        }
    }
}
