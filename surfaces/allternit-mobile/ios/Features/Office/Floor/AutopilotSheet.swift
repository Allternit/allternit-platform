import SwiftUI

/// The person's Autopilot (switch + rules), shared by the chief dock's switch
/// and the Autopilot sheet. Server: `GET|PUT /floor/autopilot`.
@MainActor
final class FloorAutopilotModel: ObservableObject {
    enum LoadState: Equatable { case idle, loading, loaded, offline, failed(String) }

    @Published private(set) var autopilot: FloorAutopilot?
    @Published private(set) var state: LoadState = .idle
    @Published private(set) var saving = false
    /// The last refused or failed save, in plain words.
    @Published var saveError: String?

    var enabled: Bool { autopilot?.enabled ?? false }

    func load() async {
        if autopilot == nil { state = .loading }
        do {
            autopilot = try await BotModeClient.shared.floorAutopilot()
            state = .loaded
        } catch is CancellationError {
        } catch {
            // A failed refresh keeps the last good value.
            guard autopilot == nil else { return }
            state = isOfflineError(error) ? .offline : .failed(plainErrorText(error, fallback: "Couldn't load Autopilot."))
        }
    }

    /// Saves; returns true when the server accepted it.
    @discardableResult
    func save(_ next: FloorAutopilot) async -> Bool {
        saving = true
        saveError = nil
        defer { saving = false }
        do {
            autopilot = try await BotModeClient.shared.saveFloorAutopilot(next)
            state = .loaded
            return true
        } catch let rejection as FloorAutopilotRejection {
            saveError = rejection.errorDescription
        } catch {
            saveError = isOfflineError(error) ? "You're offline. Autopilot wasn't changed." : plainErrorText(error, fallback: "Couldn't save Autopilot. Try again.")
        }
        return false
    }

    /// The dock's switch: same rules, the other state.
    func setEnabled(_ on: Bool) async {
        guard var next = autopilot, next.enabled != on else { return }
        next.enabled = on
        if await save(next) { Haptics.selection() } else { Haptics.error() }
    }
}

/// Autopilot rules (opened from the chief dock's info control): the switch,
/// one row per rule (scope, need, level), and a link to the bot's own
/// settings. Questions are Draft only for now (the server has no path to
/// answer them in the main bot's session yet), so their level is shown, not
/// offered.
struct AutopilotSheet: View {
    @ObservedObject var model: FloorAutopilotModel
    /// The main bot's name for copy; "your main bot" when there is none.
    let mainBotName: String
    /// Bots and pods a rule can be scoped to.
    let bots: [AgentRecord]
    let pods: [FloorStateV2.Pod]
    /// Opens the main bot's settings (the sheet closes first). Nil when there is no main bot.
    var onOpenBotSettings: (() -> Void)?

    @Environment(\.dismiss) private var dismiss
    @State private var draft: FloorAutopilot?
    @State private var editing: FloorAutopilotRule?

    var body: some View {
        NavigationStack {
            content
                .navigationTitle("Autopilot")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("Close") { dismiss() }.disabled(model.saving)
                    }
                    ToolbarItem(placement: .confirmationAction) {
                        if model.saving {
                            ProgressView()
                        } else {
                            Button("Save") { save() }
                                .disabled(draft == nil || draft == model.autopilot)
                        }
                    }
                }
                .sheet(item: $editing) { rule in
                    AutopilotRuleEditor(rule: rule, mainBotName: mainBotName, bots: bots, pods: pods,
                                        isNew: !(draft?.rules.contains { $0.id == rule.id } ?? false)) { updated in
                        upsert(updated)
                    } onDelete: {
                        draft?.rules.removeAll { $0.id == rule.id }
                    }
                }
        }
        .task {
            if model.autopilot == nil { await model.load() }
            if draft == nil { draft = model.autopilot }
        }
        .onChange(of: model.autopilot) { _, latest in
            if draft == nil { draft = latest }
        }
    }

    @ViewBuilder
    private var content: some View {
        switch (draft, model.state) {
        case (nil, .offline):
            FriendlyStateView(style: .offline, icon: "wifi.slash", title: "You're offline",
                              message: "Autopilot loads when you're back online.",
                              actionTitle: "Try again", action: { Task { await model.load() } })
        case (nil, .failed(let message)):
            FriendlyStateView(style: .error, icon: "exclamationmark.triangle", title: "Couldn't load Autopilot",
                              message: message, actionTitle: "Try again", action: { Task { await model.load() } })
        case (nil, _):
            ProgressView("Loading Autopilot…")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        case (.some, _):
            form
        }
    }

    private var form: some View {
        Form {
            if let error = model.saveError {
                Section {
                    Label(error, systemImage: "exclamationmark.circle")
                        .font(.footnote)
                        .foregroundColor(Theme.statusError)
                }
            }
            Section {
                Toggle("Autopilot", isOn: Binding(
                    get: { draft?.enabled ?? false },
                    set: { draft?.enabled = $0 }
                ))
                .tint(Color("AccentPrimary"))
            } footer: {
                Text("When it's on, \(mainBotName) handles the needs your rules allow. Everything without a rule waits for you (Ask first). Payments, signing and deleting always wait for you.")
            }

            Section {
                let rules = draft?.rules ?? []
                if rules.isEmpty {
                    Text("No rules yet. Every need asks you first.")
                        .font(.subheadline)
                        .foregroundColor(Color("TextSecondary"))
                }
                ForEach(rules) { rule in
                    Button { editing = rule } label: { ruleRow(rule) }
                        .buttonStyle(.plain)
                        .accessibilityHint("Edits this rule")
                }
                Button {
                    editing = .new()
                } label: {
                    Label("Add a rule", systemImage: "plus")
                }
                .disabled(rules.count >= 50)
            } header: {
                Text("Rules")
            }

            Section {
                if let onOpenBotSettings {
                    Button {
                        dismiss()
                        onOpenBotSettings()
                    } label: {
                        Label("\(mainBotName)'s settings", systemImage: "slider.horizontal.3")
                    }
                }
            } footer: {
                Text("Sending email and messages outside Allternit follows each bot's own settings, not Autopilot.")
            }
        }
        .scrollContentBackground(.hidden)
        .background(Color("BgPanel"))
    }

    private func ruleRow(_ rule: FloorAutopilotRule) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text("\(FloorAutopilotRule.needName(rule.need)) · \(scopeName(rule.scope))")
                    .font(.body)
                    .foregroundColor(Color("TextPrimary"))
                Text(levelLine(rule))
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
            }
            Spacer(minLength: 0)
            Image(systemName: "chevron.right")
                .font(.footnote.weight(.semibold))
                .foregroundColor(Color("TextSecondary"))
                .accessibilityHidden(true)
        }
        .frame(minHeight: 44)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
    }

    private func levelLine(_ rule: FloorAutopilotRule) -> String {
        // Questions are fixed to Draft only, whatever an older client stored.
        let level = rule.need == "question" ? "draft" : rule.level
        var line = FloorAutopilotRule.levelName(level)
        if level == "limits", let max = rule.limits?.maxPerDay { line += " · up to \(max) a day" }
        return line
    }

    private func scopeName(_ scope: FloorAutopilotRule.Scope) -> String {
        switch scope.type {
        case "bot": return bots.first { $0.id == scope.id }?.botDisplayName ?? "One bot"
        case "pod": return pods.first { $0.id == scope.id }?.name ?? "One pod"
        default: return "All bots"
        }
    }

    private func upsert(_ rule: FloorAutopilotRule) {
        guard draft != nil else { return }
        if let index = draft!.rules.firstIndex(where: { $0.id == rule.id }) {
            draft!.rules[index] = rule
        } else {
            draft!.rules.append(rule)
        }
    }

    private func save() {
        guard let draft else { return }
        Task {
            if await model.save(draft) {
                Haptics.success()
                dismiss()
            } else {
                Haptics.error()
            }
        }
    }
}

/// One rule: scope (all / a bot / a pod), need, level, and limits for "Act within limits".
private struct AutopilotRuleEditor: View {
    @State var rule: FloorAutopilotRule
    let mainBotName: String
    let bots: [AgentRecord]
    let pods: [FloorStateV2.Pod]
    let isNew: Bool
    let onSave: (FloorAutopilotRule) -> Void
    let onDelete: () -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var actionsText = ""

    var body: some View {
        NavigationStack {
            Form {
                Section("Which bots") {
                    Picker("Applies to", selection: $rule.scope.type) {
                        Text("All bots").tag("all")
                        if !bots.isEmpty { Text("One bot").tag("bot") }
                        if !pods.isEmpty { Text("One pod").tag("pod") }
                    }
                    if rule.scope.type == "bot" {
                        Picker("Bot", selection: scopeId) {
                            Text("Pick a bot").tag("")
                            ForEach(bots) { Text($0.botDisplayName).tag($0.id) }
                        }
                    } else if rule.scope.type == "pod" {
                        Picker("Pod", selection: scopeId) {
                            Text("Pick a pod").tag("")
                            ForEach(pods, id: \.id) { Text($0.name ?? "Pod").tag($0.id) }
                        }
                    }
                }

                Section("Which need") {
                    Picker("Need", selection: $rule.need) {
                        ForEach(FloorAutopilotRule.needs, id: \.self) { Text(FloorAutopilotRule.needName($0)).tag($0) }
                    }
                }

                Section {
                    if rule.need == "question" {
                        LabeledContent("Level", value: FloorAutopilotRule.levelName("draft"))
                    } else {
                        Picker("Level", selection: $rule.level) {
                            ForEach(FloorAutopilotRule.levels, id: \.self) { Text(FloorAutopilotRule.levelName($0)).tag($0) }
                        }
                    }
                } header: {
                    Text("What \(mainBotName) may do")
                } footer: {
                    Text(rule.need == "question"
                         ? "Questions are Draft only for now: \(mainBotName) writes a proposed answer and you send it."
                         : FloorAutopilotRule.levelHelp(rule.level))
                }

                if rule.need != "question", rule.level == "limits" {
                    Section {
                        Stepper(value: maxPerDay, in: 1...200) {
                            LabeledContent("Answers per day", value: "\(maxPerDay.wrappedValue)")
                        }
                        TextField("Allowed actions, comma separated", text: $actionsText, axis: .vertical)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                    } header: {
                        Text("Limits")
                    } footer: {
                        Text("For example: read, grep, websearch. Leave empty to allow any safe action. Past a limit, it asks you first.")
                    }
                }

                if !isNew {
                    Section {
                        Button("Delete rule", role: .destructive) {
                            onDelete()
                            dismiss()
                        }
                    }
                }
            }
            .scrollContentBackground(.hidden)
            .background(Color("BgPanel"))
            .navigationTitle(isNew ? "New rule" : "Rule")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") {
                        onSave(finished)
                        dismiss()
                    }
                    .disabled(rule.scope.type != "all" && (rule.scope.id ?? "").isEmpty)
                }
            }
        }
        .onAppear { actionsText = (rule.limits?.allowedActions ?? []).joined(separator: ", ") }
        .onChange(of: rule.scope.type) { _, type in
            if type == "all" { rule.scope.id = nil } else if rule.scope.id != nil, !scopeIds(type).contains(rule.scope.id!) { rule.scope.id = nil }
        }
    }

    private func scopeIds(_ type: String) -> Set<String> {
        type == "bot" ? Set(bots.map(\.id)) : Set(pods.map(\.id))
    }

    private var scopeId: Binding<String> {
        Binding(get: { rule.scope.id ?? "" }, set: { rule.scope.id = $0.isEmpty ? nil : $0 })
    }

    private var maxPerDay: Binding<Int> {
        Binding(
            get: { rule.limits?.maxPerDay ?? 20 },
            set: { value in
                var limits = rule.limits ?? FloorAutopilotRule.Limits(maxPerDay: nil, allowedActions: nil)
                limits.maxPerDay = value
                rule.limits = limits
            }
        )
    }

    /// The rule as saved: questions fixed to Draft only; limits only for "Act within limits".
    private var finished: FloorAutopilotRule {
        var out = rule
        if out.need == "question" { out.level = "draft" }
        if out.level == "limits" {
            let actions = actionsText.split(separator: ",").map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }.filter { !$0.isEmpty }
            out.limits = FloorAutopilotRule.Limits(maxPerDay: out.limits?.maxPerDay ?? 20,
                                                   allowedActions: actions.isEmpty ? nil : actions)
        } else {
            out.limits = nil
        }
        return out
    }
}
