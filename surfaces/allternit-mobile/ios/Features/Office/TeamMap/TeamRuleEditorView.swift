import SwiftUI

/// "New rule" as one sentence with blanks (ai `TeamRuleEditor`, #1346):
/// "When [bot] uses [app · tool] and [field] is [over] [€250], ask [approver] first."
/// Above it, the rules already in force for that scope (lock + "Always on"
/// for built-in rules); under it, "Try it: €325 asks Noah · €83 runs without asking."
struct TeamRuleEditorView: View {
    @ObservedObject var store: TeamMapStore
    /// "org" or the node id the editor was opened from.
    let scope: String

    @State private var draft: Draft
    @State private var saving = false
    @State private var error: String?
    @Environment(\.dismiss) private var dismiss

    init(store: TeamMapStore, scope: String) {
        self.store = store
        self.scope = scope
        _draft = State(initialValue: Draft(scope: scope))
    }

    struct Draft: Equatable {
        var scope: String
        /// "kind:ref", or "*" for any app.
        var app = "*"
        var tool = ""
        var field = ""
        var op = "gt"
        var value = ""
        var unit = ""
        /// "owner", "manager" or "u:<user_id>".
        var approver = "owner"
    }

    private struct Op: Identifiable {
        let op: String
        let label: String
        var id: String { op }
    }

    private static let ops: [Op] = [
        Op(op: "always", label: "always"), Op(op: "gt", label: "over"), Op(op: "gte", label: "at least"),
        Op(op: "eq", label: "equals"), Op(op: "in", label: "is one of"),
    ]

    var body: some View {
        Group {
            if let map = store.map {
                form(map)
            } else {
                ProgressView()
            }
        }
        .navigationTitle("New rule")
        .navigationBarTitleDisplayMode(.inline)
    }

    private func form(_ map: TeamMap) -> some View {
        let current = map.rules(inScope: draft.scope)
        return List {
            Section {
                if current.isEmpty {
                    Text("No rules yet.")
                        .font(.footnote)
                        .foregroundStyle(Color("TextSecondary"))
                }
                ForEach(current) { TeamRuleRow(rule: $0) }
            } header: {
                Text("\(scopeName(map)) · \(TeamPlural.count(current.count, "rule")) today")
            }
            Section {
                sentence(map)
                    .padding(.vertical, 6)
                if let line = tryLine(map) {
                    Label(line, systemImage: "checkmark")
                        .font(.footnote)
                        .foregroundStyle(Color("TextPrimary"))
                        .labelStyle(TeamHintLabelStyle(tint: .green))
                }
                if let problem {
                    Text(problem).font(.footnote).foregroundStyle(Color("TextSecondary"))
                }
                if let error {
                    Text(error).font(.footnote).foregroundStyle(.red).fixedSize(horizontal: false, vertical: true)
                }
            } header: {
                Text("New rule")
            } footer: {
                Text("Rules win over the level.")
            }
            Section {
                Button {
                    save(map)
                } label: {
                    HStack {
                        Spacer()
                        if saving { ProgressView().padding(.trailing, 6) }
                        Text(saving ? "Saving…" : "Save rule").bold()
                        Spacer()
                    }
                    .frame(minHeight: 44)
                }
                .disabled(problem != nil || saving)
            }
        }
        .listStyle(.insetGrouped)
    }

    // MARK: Sentence

    private func sentence(_ map: TeamMap) -> some View {
        let always = draft.op == "always"
        return TeamSentenceFlow(spacing: 6, lineSpacing: 8) {
            word("When")
            pick("Who", text: scopeName(map), selection: $draft.scope) {
                Text("Anyone").tag("org")
                Section("People") {
                    ForEach(map.people) { Text($0.name).tag($0.id) }
                }
                Section("Bots") {
                    ForEach(map.bots) { Text($0.name).tag($0.id) }
                }
            }
            word("uses")
            pick("App", text: appName(map), selection: $draft.app) {
                Text("Any app").tag("*")
                ForEach(map.apps, id: \.self) { app in
                    Text(app.displayName).tag("\(app.kind):\(app.ref)")
                }
            }
            word("·")
            blankField("Tool", text: $draft.tool, placeholder: "any tool", minWidth: 80)
            if always {
                word(",")
                pick("Condition", text: "always", selection: $draft.op) { opOptions }
            } else {
                word("and")
                blankField("Field", text: $draft.field, placeholder: "pick a field", minWidth: 96)
                word("is")
                pick("Condition", text: opLabel, selection: $draft.op) { opOptions }
                blankField("Unit (optional)", text: $draft.unit, placeholder: map.currencyMark.isEmpty ? "unit" : map.currencyMark, minWidth: 40)
                blankField("Value", text: $draft.value, placeholder: draft.op == "in" ? "a, b, c" : "250", minWidth: 60,
                           keyboard: draft.op == "gt" || draft.op == "gte" ? .decimalPad : .default)
                word(",")
            }
            word("ask")
            pick("Approver", text: approverShort(map), selection: $draft.approver) {
                Text("The owner").tag("owner")
                Text("The manager").tag("manager")
                Section("A person") {
                    ForEach(map.people) { person in
                        Text(firstName(person.name)).tag("u:\(person.userId ?? "")")
                    }
                }
            }
            word("first.")
        }
        .onChange(of: draft) { _, _ in error = nil }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Rule sentence")
    }

    @ViewBuilder
    private var opOptions: some View {
        ForEach(Self.ops) { Text($0.label).tag($0.op) }
    }

    private func word(_ text: String) -> some View {
        Text(text)
            .font(.body)
            .foregroundStyle(Color("TextPrimary"))
            .padding(.vertical, 6)
            .accessibilityHidden(true)
    }

    /// A blank that picks from a list: a chip that opens a menu on tap.
    private func pick<Content: View>(_ label: String, text: String, selection: Binding<String>,
                                     @ViewBuilder content: () -> Content) -> some View {
        Menu {
            Picker(label, selection: selection) { content() }
        } label: {
            HStack(spacing: 4) {
                Text(text).lineLimit(1)
                Image(systemName: "chevron.down").font(.caption2.weight(.semibold))
            }
            .font(.body.weight(.medium))
            .foregroundStyle(Color("TextPrimary"))
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .frame(minHeight: 36)
            .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("FillNeutral")))
            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(Color("BorderSubtle"), lineWidth: 1))
        }
        .accessibilityLabel("\(label): \(text)")
    }

    /// A typed blank; dashed while empty so it never looks filled.
    private func blankField(_ label: String, text: Binding<String>, placeholder: String, minWidth: CGFloat,
                            keyboard: UIKeyboardType = .default) -> some View {
        let empty = text.wrappedValue.trimmingCharacters(in: .whitespaces).isEmpty
        return TextField(placeholder, text: text)
            .font(.body)
            .keyboardType(keyboard)
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
            .fixedSize()
            .frame(minWidth: minWidth)
            .padding(.horizontal, 8)
            .padding(.vertical, 6)
            .frame(minHeight: 36)
            .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(empty ? Color.clear : Color("FillNeutral")))
            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous)
                .strokeBorder(Color("BorderStrong"), style: StrokeStyle(lineWidth: 1, dash: empty ? [4, 3] : [])))
            .accessibilityLabel(label)
    }

    // MARK: Wording

    private func scopeName(_ map: TeamMap) -> String {
        draft.scope == "org" ? "Anyone" : (map.name(draft.scope) ?? "Someone")
    }

    private func appName(_ map: TeamMap) -> String {
        guard draft.app != "*" else { return "any app" }
        return map.apps.first { "\($0.kind):\($0.ref)" == draft.app }?.displayName ?? draft.app
    }

    private var opLabel: String { Self.ops.first { $0.op == draft.op }?.label ?? "over" }

    private func approverShort(_ map: TeamMap) -> String {
        switch draft.approver {
        case "owner": return "the owner"
        case "manager": return "the manager"
        default: return firstName(map.name("p:\(draft.approver.dropFirst(2))") ?? "someone")
        }
    }

    private func firstName(_ name: String) -> String {
        name.split(separator: " ").first.map(String.init) ?? name
    }

    private static func number(_ text: String) -> Double? {
        let t = text.trimmingCharacters(in: .whitespaces)
        guard !t.isEmpty, let n = Double(t), n.isFinite else { return nil }
        return n
    }

    /// The typed value as JSON: numbers stay numbers, "is one of" becomes a list. Nil = not valid yet.
    private var parsedValue: JSONValue?? {
        let t = draft.value.trimmingCharacters(in: .whitespaces)
        switch draft.op {
        case "always": return .some(nil)
        case "gt", "gte":
            guard let n = Self.number(t) else { return nil }
            return .some(.number(n))
        case "eq":
            guard !t.isEmpty else { return nil }
            return .some(Self.number(t).map { JSONValue.number($0) } ?? JSONValue.string(t))
        default:
            let items = t.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
            guard !items.isEmpty else { return nil }
            let numbers = items.compactMap(Self.number)
            return .some(.array(numbers.count == items.count ? numbers.map { JSONValue.number($0) } : items.map { JSONValue.string($0) }))
        }
    }

    private var problem: String? {
        if draft.op != "always", draft.field.trimmingCharacters(in: .whitespaces).isEmpty {
            return "Name the field to check, for example amount."
        }
        if parsedValue == nil {
            return draft.op == "gt" || draft.op == "gte" ? "Enter a number." : "Enter a value."
        }
        return nil
    }

    private func tryLine(_ map: TeamMap) -> String? {
        guard problem == nil else { return nil }
        let who = approverShort(map)
        if draft.op == "always" { return "Try it: every request asks \(who) first." }
        guard draft.op == "gt" || draft.op == "gte", let n = Self.number(draft.value), n > 0 else { return nil }
        let unit = draft.unit.trimmingCharacters(in: .whitespaces).isEmpty ? map.currencyMark : draft.unit.trimmingCharacters(in: .whitespaces)
        func money(_ x: Double) -> String {
            let amount = Int(x).formatted()
            if unit.isEmpty { return amount }
            return unit.count <= 2 ? "\(unit)\(amount)" : "\(amount) \(unit)"
        }
        return "Try it: \(money((n * 1.3).rounded())) asks \(who) · \(money(max(0, (n / 3).rounded(.down)))) runs without asking."
    }

    // MARK: Save

    private func input() -> TeamRuleInput? {
        guard let value = parsedValue else { return nil }
        let scopeKind = draft.scope == "org" ? "org" : (draft.scope.hasPrefix("p:") ? "person" : "bot")
        let appParts = draft.app.split(separator: ":", maxSplits: 1).map(String.init)
        let anyApp = draft.app == "*" || appParts.count < 2
        let approverKind = draft.approver.hasPrefix("u:") ? "user" : draft.approver
        let needsField = draft.op != "always"
        let unit = draft.unit.trimmingCharacters(in: .whitespaces)
        let tool = draft.tool.trimmingCharacters(in: .whitespaces)
        return TeamRuleInput(
            scopeKind: scopeKind,
            scopeRef: scopeKind == "org" ? nil : (scopeKind == "person" ? String(draft.scope.dropFirst(2)) : draft.scope),
            appKind: anyApp ? nil : appParts[0],
            appRef: anyApp ? nil : appParts[1],
            tool: tool.isEmpty ? "*" : tool,
            field: needsField ? draft.field.trimmingCharacters(in: .whitespaces) : nil,
            op: draft.op,
            value: value,
            unitLabel: needsField && !unit.isEmpty ? String(unit.prefix(24)) : nil,
            approverKind: approverKind,
            approverUserId: approverKind == "user" ? String(draft.approver.dropFirst(2)) : nil
        )
    }

    private func save(_ map: TeamMap) {
        guard problem == nil, !saving, let input = input() else { return }
        saving = true
        error = nil
        Task {
            let failure = await store.createRule(input)
            saving = false
            if let failure {
                error = failure
            } else {
                dismiss()
            }
        }
    }
}

/// The hint line's icon in a colour, text in the default.
private struct TeamHintLabelStyle: LabelStyle {
    let tint: Color
    func makeBody(configuration: Configuration) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            configuration.icon.foregroundStyle(tint)
            configuration.title
        }
    }
}

/// Words and blanks that wrap like a sentence.
struct TeamSentenceFlow: Layout {
    var spacing: CGFloat = 6
    var lineSpacing: CGFloat = 8

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        arrange(width: proposal.width ?? .infinity, subviews: subviews).size
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        let result = arrange(width: bounds.width, subviews: subviews)
        for (index, place) in result.places.enumerated() {
            subviews[index].place(at: CGPoint(x: bounds.minX + place.x, y: bounds.minY + place.y),
                                  proposal: ProposedViewSize(result.sizes[index]))
        }
    }

    private func arrange(width: CGFloat, subviews: Subviews) -> (places: [CGPoint], sizes: [CGSize], size: CGSize) {
        var places: [CGPoint] = []
        var sizes: [CGSize] = []
        var rows: [(start: Int, height: CGFloat)] = []
        var x: CGFloat = 0
        var y: CGFloat = 0
        var rowHeight: CGFloat = 0
        var widest: CGFloat = 0
        var rowStart = 0
        for (index, subview) in subviews.enumerated() {
            var size = subview.sizeThatFits(.unspecified)
            size.width = min(size.width, width)
            if x > 0, x + size.width > width {
                rows.append((rowStart, rowHeight))
                rowStart = index
                x = 0
                y += rowHeight + lineSpacing
                rowHeight = 0
            }
            places.append(CGPoint(x: x, y: y))
            sizes.append(size)
            x += size.width + spacing
            widest = max(widest, x - spacing)
            rowHeight = max(rowHeight, size.height)
        }
        rows.append((rowStart, rowHeight))
        // Centre each item on its row's line so words and chips share a baseline-ish middle.
        for (r, row) in rows.enumerated() {
            let end = r + 1 < rows.count ? rows[r + 1].start : places.count
            for i in row.start..<end {
                places[i].y += (row.height - sizes[i].height) / 2
            }
        }
        return (places, sizes, CGSize(width: widest, height: y + rowHeight))
    }
}
