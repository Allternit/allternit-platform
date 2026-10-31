import SwiftUI
import UIKit

/// Frequency on the wizard (`Frequency`, AutomationTasksView.tsx), plus Custom
/// for a cron no preset describes, so opening one to edit doesn't change it.
enum AutomationFrequency: String, Identifiable {
    case manual, hourly, daily, weekdays, weekly, continuous, custom

    var id: String { rawValue }
    var label: String { rawValue.prefix(1).uppercased() + rawValue.dropFirst() }

    /// handleFrequencyChange: what picking it writes to the expression.
    var defaultExpression: String {
        switch self {
        case .manual: return ""
        case .hourly: return "0 * * * *"
        case .daily, .custom: return "0 9 * * *"
        case .weekdays: return "0 9 * * 1-5"
        case .weekly: return "0 9 * * 1"
        case .continuous: return "5m"
        }
    }

    /// The preset a cron is, or Custom when no preset says the same thing.
    static func of(cron: String) -> AutomationFrequency {
        let f = cron.split(whereSeparator: \.isWhitespace).map(String.init)
        guard f.count == 5, f[2] == "*", f[3] == "*", Int(f[0]) != nil else { return .custom }
        if f[1] == "*" { return f[4] == "*" ? .hourly : .custom }
        guard Int(f[1]) != nil else { return .custom }
        switch f[4] {
        case "*": return .daily
        case "1-5": return .weekdays
        case "0", "1", "2", "3", "4", "5", "6", "7": return .weekly
        default: return .custom
        }
    }
}

/// What the wizard starts from: an empty task, a calendar slot, or an existing
/// goal, routine, loop or heartbeat.
struct AutomationTaskDraft: Identifiable {
    let id = UUID()
    var kind: AutomationTaskItem.Kind
    /// Set when editing.
    var editingId: String? = nil
    /// A routine's `[bot:<name>] ` owner prefix, kept out of the field and put back on save.
    var namePrefix = ""
    var name = ""
    var description = ""
    var prompt = ""
    var frequency: AutomationFrequency = .daily
    var expression = "0 9 * * *"
    /// The stored schedule type, for a type no preset covers ("once").
    var scheduleType = "cron"
    var goalId: String? = nil
    var agentId: String? = nil
    /// "provider/model" for a loop or heartbeat on the Default Model.
    var model: String? = nil
    var priority = "medium"
    var targetDate: String? = nil
    /// The record's whole `config`: the update route replaces that column.
    var config: [String: JSONValue] = [:]
    var timezone: String? = nil
    /// False for shell and http cron jobs, which have no prompt or agent.
    var hasPrompt = true
    /// A job bound to a bot's own chat (`config.bot`): it isn't reassigned here.
    var boundBot: String? = nil
    /// The calendar slot it was opened from.
    var slotCron: String? = nil

    /// A new task, on a calendar slot or day when it was opened from one.
    init(kind: AutomationTaskItem.Kind, slotCron: String? = nil, targetDate: String? = nil) {
        self.kind = kind
        self.slotCron = slotCron
        self.targetDate = kind == .goal ? targetDate : nil
        switch kind {
        case .goal: frequency = .manual
        case .loop: frequency = .continuous; expression = "5m"
        case .routine, .heartbeat: break
        }
        if kind != .goal, let slotCron, !slotCron.isEmpty {
            expression = slotCron
            frequency = .of(cron: slotCron)
        }
    }

    init(goal: AutomationGoal) {
        kind = .goal
        editingId = goal.id
        name = goal.title
        description = goal.description ?? ""
        frequency = .manual
        priority = goal.priority
        targetDate = goal.targetDate
        agentId = goal.agentId
    }

    init(routine: AutomationRoutine) {
        kind = .routine
        editingId = routine.id
        name = AutomationScheduleLogic.displayName(routine.name)
        namePrefix = Self.prefix(of: routine.name, display: name)
        description = routine.description ?? ""
        prompt = routine.instruction
        goalId = routine.goalId
        agentId = routine.agentId
        config = routine.config?.value ?? [:]
        timezone = routine.timezone
        applySchedule(type: routine.scheduleType, expression: routine.scheduleExpression)
    }

    init(loop: AutomationLoop) {
        kind = .loop
        editingId = loop.id
        name = AutomationScheduleLogic.displayName(loop.name)
        namePrefix = Self.prefix(of: loop.name, display: name)
        description = loop.description ?? ""
        prompt = loop.instruction
        goalId = loop.goalId
        agentId = loop.agentId
        config = loop.config?.value ?? [:]
        if case .string(let saved)? = config["model"], !saved.isEmpty { model = saved }
        applySchedule(type: loop.scheduleType, expression: loop.scheduleExpression)
    }

    init(job: CronJob) {
        kind = .heartbeat
        editingId = job.id
        name = AutomationScheduleLogic.displayName(job.name)
        namePrefix = Self.prefix(of: job.name, display: name)
        description = job.description ?? ""
        prompt = job.agentConfig?.prompt ?? ""
        hasPrompt = job.type == "agent"
        agentId = job.agentConfig?.agentId
        model = job.agentConfig?.model
        boundBot = job.agentConfig?.bot
        switch job.schedule {
        case .cron(let value, _):
            expression = value
            frequency = .of(cron: value)
        case .interval(let seconds, _):
            expression = "\(seconds)s"
            frequency = .custom
        case .unknown:
            expression = ""
            frequency = .custom
        }
    }

    private mutating func applySchedule(type: String, expression: String) {
        scheduleType = type
        self.expression = expression
        switch type {
        case "manual": frequency = .manual
        case "interval": frequency = kind == .loop ? .continuous : .custom
        case "cron": frequency = .of(cron: expression)
        default: frequency = .custom
        }
    }

    private static func prefix(of raw: String, display: String) -> String {
        guard raw.hasPrefix("[bot:"), raw.hasSuffix(display) else { return "" }
        return String(raw.dropLast(display.count))
    }
}

/// What a task's details need to open its Edit form.
struct AutomationTaskEditContext {
    let draft: AutomationTaskDraft
    let store: AutomationStore
    let onSaved: @MainActor () -> Void
}

/// AutomationWizardForm (AutomationTasksView.tsx): one form to create or edit a
/// goal, routine, loop or heartbeat. A goal can also package a routine and a
/// monitoring loop, when it's created or later.
struct AutomationWizardSheet: View {
    @ObservedObject var store: AutomationStore
    let draft: AutomationTaskDraft
    var onSaved: @MainActor () -> Void = {}

    @ObservedObject private var hub = AgentHubStore.shared
    @ObservedObject private var modelStore = ModelStore.shared
    @Environment(\.dismiss) private var dismiss

    @State private var kind: AutomationTaskItem.Kind
    @State private var name: String
    @State private var details: String
    @State private var prompt: String
    @State private var frequency: AutomationFrequency
    @State private var expression: String
    @State private var goalId: String?
    @State private var agentId: String?
    @State private var usesDefaultModel: Bool
    @State private var model: String?
    @State private var priority: String
    @State private var hasTargetDate: Bool
    @State private var targetDate: Date

    @State private var addRoutine = false
    @State private var routineName = ""
    @State private var routinePrompt = ""
    @State private var routineFrequency: AutomationFrequency = .daily
    @State private var routineExpression = "0 9 * * *"
    @State private var addLoop = false
    @State private var loopName = ""
    @State private var loopPrompt = ""
    @State private var loopExpression = "5m"

    @State private var isSaving = false
    @State private var error: String? = nil
    /// A goal already saved by a try whose packaged tasks failed: a retry
    /// adds to it rather than making a second goal.
    @State private var savedGoalId: String? = nil
    @State private var savedPackagedRoutine = false

    init(store: AutomationStore, draft: AutomationTaskDraft, onSaved: @escaping @MainActor () -> Void = {}) {
        self.store = store
        self.draft = draft
        self.onSaved = onSaved
        _kind = State(initialValue: draft.kind)
        _name = State(initialValue: draft.name)
        _details = State(initialValue: draft.description)
        _prompt = State(initialValue: draft.prompt)
        _frequency = State(initialValue: draft.frequency)
        _expression = State(initialValue: draft.expression)
        _goalId = State(initialValue: draft.goalId)
        _agentId = State(initialValue: draft.agentId)
        _usesDefaultModel = State(initialValue: draft.editingId != nil && draft.agentId == nil)
        _model = State(initialValue: draft.model)
        _priority = State(initialValue: draft.priority)
        let date = draft.targetDate.flatMap(Self.parseDay)
        _hasTargetDate = State(initialValue: date != nil)
        _targetDate = State(initialValue: date ?? Date())
    }

    private var isEditing: Bool { draft.editingId != nil }

    private static func parseDay(_ raw: String) -> Date? {
        // The plain day first: read as a UTC instant it can land on the day before.
        dayFormatter.date(from: raw) ?? libraryParseDate(raw)
    }

    private static var dayFormatter: DateFormatter {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyy-MM-dd"
        return formatter
    }

    // MARK: - Bots

    /// `useRoutineBots`: bots to offer, main bot first.
    private var routineBots: [AgentRecord] {
        let bots = hub.agents.filter(\.isBot)
        return bots.filter(Self.isMainBot) + bots.filter { !Self.isMainBot($0) }
    }

    /// `defaultRoutineBotId`: the main bot, else the first bot.
    private var defaultBotId: String? { routineBots.first?.id }

    /// bot-roster-preview `isChiefOfStaff`.
    private static func isMainBot(_ bot: AgentRecord) -> Bool {
        if bot.isPrimaryTemplateBot { return true }
        if case .object(let profile)? = bot.config?["botProfile"], case .bool(true)? = profile["chiefOfStaff"] { return true }
        return false
    }

    /// Fills the default once bots load, so a form never opens with no bot
    /// when there is one (RoutineBotPicker's effect). A loop or heartbeat being
    /// edited keeps the Default Model it was saved with.
    private func applyDefaultBot() {
        guard agentId == nil, !usesDefaultModel, let bot = defaultBotId else { return }
        agentId = bot
    }

    // MARK: - Schedule

    private var frequencyOptions: [AutomationFrequency] {
        var options: [AutomationFrequency]
        switch kind {
        case .goal: options = []
        case .routine: options = [.manual, .hourly, .daily, .weekdays, .weekly]
        case .loop: options = [.continuous, .hourly, .daily, .weekdays, .weekly]
        // A heartbeat has no manual: the scheduler needs a schedule to hold it.
        case .heartbeat: options = [.hourly, .daily, .weekdays, .weekly]
        }
        if frequency == .custom { options.append(.custom) }
        return options
    }

    private var trimmedExpression: String { expression.trimmingCharacters(in: .whitespacesAndNewlines) }

    private var scheduleType: String {
        switch frequency {
        case .manual: return "manual"
        case .continuous: return "interval"
        case .custom: return draft.scheduleType
        case .hourly, .daily, .weekdays, .weekly: return "cron"
        }
    }

    private var hasFiveFields: Bool {
        trimmedExpression.split(whereSeparator: \.isWhitespace).count == 5
    }

    private var scheduleIsValid: Bool {
        if frequency == .manual { return true }
        if trimmedExpression.isEmpty { return isEditing && draft.expression.isEmpty }
        if hasFiveFields { return !CronDays.parseCronDays(trimmedExpression).isEmpty }
        return true
    }

    /// handleFrequencyChange.
    private func setFrequency(_ value: AutomationFrequency) {
        frequency = value
        if value != .custom { expression = value.defaultExpression }
    }

    private func setKind(_ value: AutomationTaskItem.Kind) {
        kind = value
        if let slot = draft.slotCron, !slot.isEmpty, value != .goal {
            expression = slot
            frequency = .of(cron: slot)
        } else {
            switch value {
            case .goal: setFrequency(.manual)
            case .routine, .heartbeat: setFrequency(.daily)
            case .loop: setFrequency(.continuous)
            }
        }
        if value == .routine { usesDefaultModel = false }
        applyDefaultBot()
    }

    // MARK: - Validity

    private func filled(_ text: String) -> Bool {
        !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    private var canSave: Bool {
        guard !isSaving, filled(name), filled(details) else { return false }
        switch kind {
        case .goal:
            if addRoutine, !(filled(routineName) && filled(routinePrompt) && filled(routineExpression) && defaultRoutineBot != nil) { return false }
            if addLoop, !(filled(loopName) && filled(loopPrompt) && filled(loopExpression)) { return false }
            return true
        case .routine:
            return filled(prompt) && agentId != nil && scheduleIsValid
        case .loop, .heartbeat:
            if draft.hasPrompt, !filled(prompt) { return false }
            if draft.hasPrompt, draft.boundBot == nil, !usesDefaultModel, agentId == nil { return false }
            return scheduleIsValid
        }
    }

    /// The bot a packaged routine runs as: `data.agentId || defaultRoutineBot`.
    private var defaultRoutineBot: String? { agentId ?? defaultBotId }

    // MARK: - Body

    var body: some View {
        NavigationStack {
            Form {
                if !isEditing { typeSection }
                basicsSection
                if kind != .goal {
                    if kind == .routine || kind == .loop { goalSection }
                    scheduleSection
                    executorSection
                } else {
                    goalFieldsSection
                    packageSection
                }
                if let error {
                    Section {
                        Text(error).font(.footnote).foregroundColor(Theme.statusError)
                    }
                }
            }
            .scrollDismissesKeyboard(.interactively)
            .scrollContentBackground(.hidden)
            .background(Color("BgPrimary"))
            .navigationTitle(isEditing ? "Edit Automation" : "Create Automation")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button(saveLabel) { Task { await save() } }
                        .disabled(!canSave)
                }
            }
            .task {
                hub.fetchAgentsIfNeeded()
                modelStore.fetchModelsIfNeeded()
                applyDefaultBot()
                if store.goals.isEmpty { await store.load() }
            }
            .onChange(of: hub.agents) { _, _ in applyDefaultBot() }
        }
    }

    private var saveLabel: String {
        if isSaving { return isEditing ? "Updating…" : "Saving…" }
        return isEditing ? "Update" : "Save"
    }

    private var typeSection: some View {
        Section("Task Type") {
            Picker("Task Type", selection: Binding(get: { kind }, set: { setKind($0) })) {
                Text("Goal").tag(AutomationTaskItem.Kind.goal)
                Text("Routine").tag(AutomationTaskItem.Kind.routine)
                Text("Loop").tag(AutomationTaskItem.Kind.loop)
                Text("Heartbeat").tag(AutomationTaskItem.Kind.heartbeat)
            }
            .pickerStyle(.segmented)
            .labelsHidden()
        }
    }

    private var basicsSection: some View {
        Group {
            Section("Name *") {
                TextField(kind == .goal ? "Objective name" : "Task name", text: $name)
            }
            Section("Description *") {
                TextField("Provide a brief summary of the task...", text: $details, axis: .vertical)
                    .lineLimit(1...3)
            }
            if kind != .goal, draft.hasPrompt {
                Section("Instructions / Prompt *") {
                    TextField("What instructions should the executor run? (e.g. Triage support tickets...)", text: $prompt, axis: .vertical)
                        .lineLimit(4...10)
                }
            }
        }
    }

    private var goalSection: some View {
        Section("Associated Goal (Workflow)") {
            if let goalsError = store.goalsError, store.goals.isEmpty {
                Text(FriendlyErrorMessage.from(goalsError)).font(.footnote).foregroundColor(Theme.statusError)
                Button("Try again") { Task { await store.load() } }
            } else {
                Picker("Goal", selection: $goalId) {
                    Text("No goal selected").tag(String?.none)
                    ForEach(store.goals) { Text($0.title).tag(String?.some($0.id)) }
                }
            }
        }
    }

    private var scheduleSection: some View {
        Section {
            Picker("Frequency", selection: Binding(get: { frequency }, set: { setFrequency($0) })) {
                ForEach(frequencyOptions) { Text($0.label).tag($0) }
            }
            if frequency != .manual {
                LabeledContent(frequency == .continuous ? "Delay Interval" : "Schedule Expression") {
                    TextField(frequency == .continuous ? "5m" : "0 9 * * *", text: $expression)
                        .multilineTextAlignment(.trailing)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .font(.body.monospaced())
                }
            }
            if frequency != .manual, frequency != .continuous, hasFiveFields {
                VStack(alignment: .leading, spacing: 8) {
                    Text("Days of week")
                    DayOfWeekSelector(
                        selectedDays: CronDays.parseCronDays(trimmedExpression),
                        onChange: { days in
                            expression = CronDays.applyCronDays(trimmedExpression, days: days)
                            frequency = .of(cron: expression)
                        }
                    )
                }
                .padding(.vertical, 4)
            }
        } footer: {
            if !scheduleIsValid {
                Text(hasFiveFields ? "Pick at least one day." : "Add a schedule.")
            } else if frequency != .manual, !trimmedExpression.isEmpty {
                Text(CronWords.describe(type: scheduleType, expression: trimmedExpression))
            } else if frequency == .manual {
                Text(CronWords.describe(type: "manual", expression: ""))
            }
        }
    }

    @ViewBuilder
    private var executorSection: some View {
        if kind == .routine {
            Section("Runs as *") { botPicker }
        } else if let bot = draft.boundBot {
            Section("Executor Selection") {
                LabeledContent("Runs as", value: bot)
            }
        } else if draft.hasPrompt {
            Section("Executor Selection") {
                Picker("Executor", selection: Binding(
                    get: { usesDefaultModel },
                    set: { value in
                        usesDefaultModel = value
                        if value {
                            agentId = nil
                        } else {
                            model = nil
                            applyDefaultBot()
                        }
                    }
                )) {
                    Text("Default Model").tag(true)
                    Text("Assign to Agent").tag(false)
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                if usesDefaultModel { modelPicker } else { agentPicker }
            }
        }
    }

    @ViewBuilder
    private var botPicker: some View {
        if hub.isLoading && hub.agents.isEmpty {
            ProgressView()
        } else if let loadError = hub.loadError, hub.agents.isEmpty {
            agentsError(loadError)
        } else if routineBots.isEmpty {
            Text("You don't have a bot yet. Create one first; routines run as a bot's turn.")
                .font(.footnote)
                .foregroundColor(Color("TextSecondary"))
        } else {
            Picker("Bot", selection: $agentId) {
                if agentId == nil { Text("Choose a bot…").tag(String?.none) }
                ForEach(routineBots) { bot in
                    Text(bot.botDisplayName + (Self.isMainBot(bot) ? " (main bot)" : ""))
                        .tag(String?.some(bot.id))
                }
            }
        }
    }

    @ViewBuilder
    private var agentPicker: some View {
        if hub.isLoading && hub.agents.isEmpty {
            ProgressView()
        } else if let loadError = hub.loadError, hub.agents.isEmpty {
            agentsError(loadError)
        } else if hub.agents.isEmpty {
            Text("Create an agent first, or use the Default Model.")
                .font(.footnote)
                .foregroundColor(Color("TextSecondary"))
        } else {
            Picker("Agent", selection: $agentId) {
                Text("Select agent...").tag(String?.none)
                ForEach(hub.agents) { Text($0.botDisplayName).tag(String?.some($0.id)) }
            }
        }
    }

    /// The job's `config.model`: "provider/model", or nothing for the default.
    @ViewBuilder
    private var modelPicker: some View {
        if modelStore.isLoading && modelStore.models.isEmpty {
            ProgressView()
        } else if let loadError = modelStore.loadError, modelStore.models.isEmpty {
            VStack(alignment: .leading, spacing: 8) {
                Text(FriendlyErrorMessage.from(loadError))
                    .font(.footnote)
                    .foregroundColor(Theme.statusError)
                Button("Try again") { modelStore.fetchModelsIfNeeded(force: true) }
            }
        } else {
            Picker("Model", selection: $model) {
                Text("Default model").tag(String?.none)
                if let model, !modelStore.models.contains(where: { Self.modelRef($0) == model }) {
                    Text(model).tag(String?.some(model))
                }
                ForEach(modelStore.models) { Text($0.name).tag(String?.some(Self.modelRef($0))) }
            }
        }
    }

    private static func modelRef(_ model: RuntimeModel) -> String {
        model.id.contains("/") ? model.id : "\(model.provider)/\(model.id)"
    }

    private func agentsError(_ message: String) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(FriendlyErrorMessage.from(message))
                .font(.footnote)
                .foregroundColor(Theme.statusError)
            Button("Try again") { hub.fetchAgentsIfNeeded(force: true) }
        }
    }

    private var goalFieldsSection: some View {
        Section {
            Picker("Priority", selection: $priority) {
                Text("Low").tag("low")
                Text("Medium").tag("medium")
                Text("High").tag("high")
                Text("Urgent").tag("urgent")
            }
            Toggle("Target Date", isOn: $hasTargetDate)
                .tint(Color("AccentPrimary"))
            if hasTargetDate {
                DatePicker("Date", selection: $targetDate, displayedComponents: .date)
            }
        }
    }

    @ViewBuilder
    private var packageSection: some View {
        Section("Package Tasks (Optional)") {
            Toggle("Schedule a Routine for this Goal", isOn: Binding(
                get: { addRoutine },
                set: { value in
                    addRoutine = value
                    routineName = value ? "\(name) Sync" : ""
                }
            ))
            .tint(Color("AccentPrimary"))
            if addRoutine {
                TextField("Daily sync, weekly review...", text: $routineName)
                TextField("What should run on this schedule?", text: $routinePrompt, axis: .vertical)
                    .lineLimit(2...5)
                Picker("Frequency", selection: Binding(
                    get: { routineFrequency },
                    set: { value in
                        routineFrequency = value
                        routineExpression = value.defaultExpression
                    }
                )) {
                    ForEach([AutomationFrequency.hourly, .daily, .weekdays, .weekly]) { Text($0.label).tag($0) }
                }
                LabeledContent("Expression") {
                    TextField("0 9 * * *", text: $routineExpression)
                        .multilineTextAlignment(.trailing)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .font(.body.monospaced())
                }
                if !hub.isLoading, routineBots.isEmpty {
                    Text("You don't have a bot yet. Create one first; routines run as a bot's turn.")
                        .font(.footnote)
                        .foregroundColor(Color("TextSecondary"))
                }
            }
        }
        Section {
            Toggle("Run a Monitoring Loop for this Goal", isOn: Binding(
                get: { addLoop },
                set: { value in
                    addLoop = value
                    loopName = value ? "\(name) Monitor" : ""
                }
            ))
            .tint(Color("AccentPrimary"))
            if addLoop {
                TextField("Topic check, price alert...", text: $loopName)
                TextField("What should be monitored continuously?", text: $loopPrompt, axis: .vertical)
                    .lineLimit(2...5)
                LabeledContent("Delay Interval") {
                    TextField("5m", text: $loopExpression)
                        .multilineTextAlignment(.trailing)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .font(.body.monospaced())
                }
            }
        }
    }

    // MARK: - Save

    private func clean(_ text: String) -> String {
        text.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    @MainActor
    private func save() async {
        isSaving = true
        error = nil
        defer { isSaving = false }
        do {
            if let id = draft.editingId {
                try await update(id: id)
            } else {
                try await create()
            }
            await store.load()
            UINotificationFeedbackGenerator().notificationOccurred(.success)
            onSaved()
            dismiss()
        } catch {
            let reason = FriendlyErrorMessage.from(error.localizedDescription)
            self.error = savedGoalId == nil ? reason : "The goal is saved, but a packaged task isn't. \(reason)"
        }
    }

    private var targetDateValue: String? {
        hasTargetDate ? Self.dayFormatter.string(from: targetDate) : nil
    }

    @MainActor
    private func create() async throws {
        let client = AutomationClient.shared
        let title = clean(name)
        let summary = clean(details)
        let instruction = clean(prompt)
        switch kind {
        case .goal:
            let goalId: String
            if let savedGoalId {
                goalId = savedGoalId
            } else {
                goalId = try await client.createGoal(CreateAutomationGoalRequest(
                    title: title, description: summary, priority: priority,
                    agentId: agentId, targetDate: targetDateValue
                )).id
                savedGoalId = goalId
            }
            try await createPackagedTasks(goalId: goalId, goalTitle: title)
        case .routine:
            guard let agentId else { return }
            _ = try await client.createRoutine(CreateAutomationRoutineRequest(
                name: title,
                description: summary,
                agentId: agentId,
                scheduleType: scheduleType,
                scheduleExpression: frequency == .manual ? "" : trimmedExpression,
                timezone: TimeZone.current.identifier,
                executionDomain: "local",
                config: ["jobType": "agent", "prompt": instruction],
                goalId: goalId
            ))
        case .loop:
            try await client.createLoop(CreateAutomationLoopRequest(
                name: title,
                description: summary,
                agentId: usesDefaultModel ? nil : agentId,
                scheduleType: scheduleType,
                scheduleExpression: trimmedExpression,
                executionDomain: "local",
                config: {
                    var config = ["jobType": "agent", "prompt": instruction]
                    if usesDefaultModel, let model { config["model"] = model }
                    return config
                }(),
                goalId: goalId
            ))
        case .heartbeat:
            try await CronJobStore.shared.createAgentJob(
                name: title,
                prompt: instruction,
                schedule: trimmedExpression,
                description: summary,
                agentId: usesDefaultModel ? nil : agentId,
                model: usesDefaultModel ? model : nil,
                maxRetries: 3,
                timeoutSeconds: 300
            )
        }
    }

    /// A goal's packaged routine and monitoring loop. A retry skips the routine
    /// when only the loop failed.
    @MainActor
    private func createPackagedTasks(goalId: String, goalTitle: String) async throws {
        let client = AutomationClient.shared
        if addRoutine, !savedPackagedRoutine, let bot = defaultRoutineBot {
            _ = try await client.createRoutine(CreateAutomationRoutineRequest(
                name: clean(routineName),
                description: "Scheduled routine supporting goal: \(goalTitle)",
                agentId: bot,
                scheduleType: "cron",
                scheduleExpression: clean(routineExpression),
                timezone: TimeZone.current.identifier,
                executionDomain: "local",
                config: ["jobType": "agent", "prompt": clean(routinePrompt)],
                goalId: goalId
            ))
            savedPackagedRoutine = true
        }
        if addLoop {
            try await client.createLoop(CreateAutomationLoopRequest(
                name: clean(loopName),
                description: "Monitoring loop supporting goal: \(goalTitle)",
                agentId: agentId,
                scheduleType: "interval",
                scheduleExpression: clean(loopExpression),
                executionDomain: "local",
                config: ["jobType": "agent", "prompt": clean(loopPrompt)],
                goalId: goalId
            ))
        }
    }

    /// Sends only what changed: a new schedule restarts the task's run
    /// bookkeeping, so saving a new name mustn't.
    @MainActor
    private func update(id: String) async throws {
        let client = AutomationClient.shared
        let newName = clean(name) == draft.name ? nil : draft.namePrefix + clean(name)
        let newDetails = clean(details) == draft.description ? nil : clean(details)
        let scheduleChanged = scheduleType != draft.scheduleType || trimmedExpression != draft.expression
        let newExpression = frequency == .manual ? "" : trimmedExpression
        let promptChanged = clean(prompt) != draft.prompt

        var mergedConfig = draft.config
        mergedConfig["prompt"] = .string(clean(prompt))
        if mergedConfig["jobType"] == nil { mergedConfig["jobType"] = .string("agent") }

        switch kind {
        case .goal:
            var request = UpdateAutomationGoalRequest(status: nil, progress: nil)
            if clean(name) != draft.name { request.title = clean(name) }
            request.description = newDetails
            if priority != draft.priority { request.priority = priority }
            // COALESCE on the server: only an empty string clears a date.
            let oldDate = draft.targetDate.flatMap(Self.parseDay).map(Self.dayFormatter.string(from:))
            if targetDateValue != oldDate { request.targetDate = targetDateValue ?? "" }
            try await client.updateGoal(id: id, request)
            if addRoutine || addLoop {
                savedGoalId = id
                try await createPackagedTasks(goalId: id, goalTitle: clean(name))
            }
        case .routine:
            var request = UpdateAutomationRoutineRequest(name: newName, description: newDetails)
            if agentId != draft.agentId { request.agentId = agentId }
            if goalId != draft.goalId { request.goalId = goalId ?? "" }
            if scheduleChanged {
                request.scheduleType = scheduleType
                request.scheduleExpression = newExpression
                request.timezone = draft.timezone ?? TimeZone.current.identifier
            }
            if promptChanged { request.config = mergedConfig }
            try await client.updateRoutine(id: id, request)
        case .loop:
            var request = UpdateAutomationLoopRequest(name: newName, description: newDetails)
            let newAgent = usesDefaultModel ? nil : agentId
            if newAgent != draft.agentId { request.agentId = newAgent ?? "" }
            if goalId != draft.goalId { request.goalId = goalId ?? "" }
            if scheduleChanged {
                request.scheduleType = scheduleType
                request.scheduleExpression = newExpression
            }
            let newModel = usesDefaultModel ? model : nil
            if newModel != draft.model { mergedConfig["model"] = newModel.map(JSONValue.string) }
            if promptChanged || newModel != draft.model { request.config = mergedConfig }
            try await client.updateLoop(id: id, request)
        case .heartbeat:
            var request = UpdateCronJobRequest(name: newName, description: newDetails)
            if trimmedExpression != draft.expression { request.schedule = trimmedExpression }
            var config: [String: JSONValue] = [:]
            if draft.hasPrompt, promptChanged { config["prompt"] = .string(clean(prompt)) }
            let newAgent = usesDefaultModel ? nil : agentId
            if draft.hasPrompt, draft.boundBot == nil, newAgent != draft.agentId {
                config["agentId"] = newAgent.map(JSONValue.string) ?? .null
            }
            let newModel = usesDefaultModel ? model : nil
            if draft.hasPrompt, draft.boundBot == nil, newModel != draft.model {
                config["model"] = newModel.map(JSONValue.string) ?? .null
            }
            if !config.isEmpty { request.config = config }
            try await CronJobStore.shared.updateJob(id: id, request)
        }
    }
}
