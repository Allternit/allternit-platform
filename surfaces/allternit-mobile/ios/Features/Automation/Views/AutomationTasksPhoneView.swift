import SwiftUI
import UIKit

/// Automation Tasks as the web phone draws it (allternit-ai
/// src/views/tasks/AutomationTasksView.tsx with `phoneChrome`): ☰ · title · a
/// filter button whose sheet holds Type, Sort by and View; the keep-awake card;
/// List, Graph or Calendar; and a floating search + "New task" bar.
///
/// Data: goals, routines and loops from allternit-api `/api/v1/automation/*`
/// (AutomationClient); heartbeats are the scheduler's cron jobs
/// (`CronJobStore`, the web's listScheduledJobs).
struct AutomationTasksPhoneView: View {
    @Binding var isSidebarOpen: Bool
    @EnvironmentObject private var modeStore: AppModeStore

    @StateObject private var store = AutomationStore()
    @ObservedObject private var cron = CronJobStore.shared
    @ObservedObject private var bots = BotModeStore.shared
    @ObservedObject private var agentHub = AgentHubStore.shared

    @AppStorage("allternit.botRoutines.view") private var routinesViewRaw = RoutinesView.list.rawValue
    @State private var activeTab: TaskTab = .all
    @State private var sortBy: TaskSort = .date
    @State private var searchQuery = ""
    @State private var keepAwake = true
    @State private var loops: [AutomationLoop] = []
    @State private var isLoading = false
    @State private var error: String? = nil
    @State private var runningTaskId: String? = nil
    @State private var selectedTask: AutomationTaskItem? = nil
    @State private var selectedRoutine: RoutineSelection? = nil
    @State private var isFilterPresented = false
    @State private var newTask: NewTask? = nil
    /// The agent whose heartbeats are open on the Agent Heartbeats tab.
    @State private var selectedHeartbeatAgent: String? = nil

    /// A task being added: by hand, or from a calendar slot.
    private struct NewTask: Identifiable {
        let id = UUID()
        let kind: AutomationTaskItem.Kind
        var cron: String? = nil
        var targetDate: String? = nil
    }

    private struct RoutineSelection: Identifiable { let id: String }

    enum TaskTab: String, CaseIterable {
        case all, goal, routine, loop, heartbeat

        var label: String {
            switch self {
            case .all: return "All"
            case .goal: return "Goals"
            case .routine: return "Routines"
            case .loop: return "Loops"
            case .heartbeat: return "Agent Heartbeats"
            }
        }

        /// Phosphor CalendarCheck / Target / Clock / Repeat / Robot.
        var icon: String {
            switch self {
            case .all: return "calendar.badge.checkmark"
            case .goal: return "target"
            case .routine: return "clock"
            case .loop: return "repeat"
            case .heartbeat: return "cpu"
            }
        }
    }

    enum TaskSort: String, CaseIterable {
        case date, name, type
        var label: String { rawValue.prefix(1).uppercased() + rawValue.dropFirst() }
    }

    /// ROUTINES_VIEWS (RoutinesSchedule.tsx), same storage key as the web.
    enum RoutinesView: String, CaseIterable {
        case list, graph, calendar
        var label: String { rawValue.prefix(1).uppercased() + rawValue.dropFirst() }
    }

    private var routinesView: RoutinesView { RoutinesView(rawValue: routinesViewRaw) ?? .list }

    // MARK: - Unified tasks

    private var tasks: [AutomationTaskItem] {
        var out: [AutomationTaskItem] = []
        for goal in store.goals {
            out.append(AutomationTaskItem(
                type: .goal, id: goal.id, name: goal.title, description: goal.description ?? "",
                status: goal.status, isActive: goal.status == "active", frequency: "manual", schedule: nil,
                createdAt: libraryParseDate(goal.createdAt), agentId: goal.agentId,
                executorName: executor(goal.agentId), progress: goal.progress, targetDate: goal.targetDate,
                goalId: nil, nextRun: nil, lastRun: nil
            ))
        }
        for routine in store.routines {
            out.append(AutomationTaskItem(
                type: .routine, id: routine.id, name: AutomationScheduleLogic.displayName(routine.name), description: routine.description ?? "",
                status: routine.status, isActive: routine.isEnabled,
                frequency: routine.scheduleType == "cron" ? AutomationScheduleLogic.cronFrequency(routine.scheduleExpression) : "manual",
                schedule: routine.scheduleExpression, createdAt: libraryParseDate(routine.createdAt),
                agentId: routine.agentId, executorName: executor(routine.agentId), progress: nil, targetDate: nil,
                goalId: routine.goalId, nextRun: routine.nextRunAt, lastRun: routine.lastRunAt,
                instructions: routine.instruction, parentGoalName: goalName(routine.goalId)
            ))
        }
        for loop in loops {
            out.append(AutomationTaskItem(
                type: .loop, id: loop.id, name: AutomationScheduleLogic.displayName(loop.name), description: loop.description ?? "",
                status: loop.status, isActive: loop.status == "active",
                frequency: loop.scheduleType == "cron" ? "daily" : "continuous",
                schedule: loop.scheduleExpression, createdAt: libraryParseDate(loop.createdAt),
                agentId: loop.agentId, executorName: executor(loop.agentId), progress: nil, targetDate: nil,
                goalId: loop.goalId, nextRun: loop.nextRunAt, lastRun: loop.lastRunAt,
                instructions: loop.instruction, parentGoalName: goalName(loop.goalId)
            ))
        }
        for job in cron.jobs {
            let expression: String?
            switch job.schedule {
            case .cron(let value, _): expression = value
            case .interval(let seconds, _): expression = "\(seconds)s"
            case .unknown: expression = nil
            }
            out.append(AutomationTaskItem(
                type: .heartbeat, id: job.id, name: job.name, description: job.description ?? "",
                status: job.status, isActive: job.status == "active", frequency: Self.heartbeatFrequency(expression),
                schedule: expression, createdAt: libraryParseDate(job.createdAt),
                agentId: job.agentConfig?.agentId, executorName: executor(job.agentConfig?.agentId),
                progress: nil, targetDate: nil, goalId: nil, nextRun: job.nextRunAt, lastRun: job.lastRunAt
            ))
        }
        return out
    }

    private var filteredTasks: [AutomationTaskItem] {
        let query = searchQuery.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        // Ties keep their order (goals first), as the web's stable sort does.
        let rows = tasks
            .filter { activeTab == .all || $0.type.rawValue == activeTab.rawValue }
            .filter { query.isEmpty || $0.name.lowercased().contains(query) || $0.description.lowercased().contains(query) }
        return rows.enumerated()
            .sorted { left, right in
                let a = left.element, b = right.element
                switch sortBy {
                case .name:
                    let order = a.name.localizedCompare(b.name)
                    return order == .orderedSame ? left.offset < right.offset : order == .orderedAscending
                case .type:
                    return a.type.rawValue == b.type.rawValue ? left.offset < right.offset : a.type.rawValue < b.type.rawValue
                case .date:
                    let l = a.createdAt ?? .distantPast, r = b.createdAt ?? .distantPast
                    return l == r ? left.offset < right.offset : l > r
                }
            }
            .map(\.element)
    }

    /// The Routines tab's rows, filtered by the search.
    private var visibleRoutines: [AutomationRoutine] {
        let query = searchQuery.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return store.routines.filter { query.isEmpty || $0.name.lowercased().contains(query) }
    }

    private func goalName(_ id: String?) -> String? {
        id.flatMap { id in store.goals.first { $0.id == id }?.title }
    }

    private func executor(_ agentId: String?) -> String {
        guard let agentId else { return "Default Model" }
        return bots.bot(agentId)?.botDisplayName ?? "Default Model"
    }

    private static func cronFrequency(_ expression: String) -> String {
        switch expression {
        case "0 * * * *": return "hourly"
        case "0 9 * * *": return "daily"
        case "0 9 * * 1-5": return "weekdays"
        case "0 9 * * 1": return "weekly"
        default: return "custom"
        }
    }

    private static func heartbeatFrequency(_ expression: String?) -> String {
        guard let expression else { return "manual" }
        let value = cronFrequency(expression)
        return value == "custom" ? "manual" : value
    }

    // MARK: - Body

    var body: some View {
        VStack(spacing: 0) {
            LibrarySectionHeader(title: "Automation Tasks", onMenu: { openLibraryDrawer($isSidebarOpen) }) {
                Button { isFilterPresented = true } label: {
                    PhoneFilterButtonLabel(active: filtersOn)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Filter and sort")
            }
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    if let error { errorBanner(error) }
                    if let actionError = store.actionError { errorBanner(actionError) }
                    typeAndViewBar.padding(.bottom, 12)
                    keepAwakeBanner
                    mainContent.padding(.top, 20)
                }
                .padding(.horizontal, 16)
                .padding(.top, 8)
                // Clears the floating bar.
                .padding(.bottom, 96)
            }
            .refreshable { await loadAll() }
        }
        .background(Color("BgPrimary").ignoresSafeArea())
        .overlay(alignment: .bottom) {
            LibraryFloatingBar(
                query: $searchQuery,
                placeholder: "Search automation tasks",
                newLabel: "New task",
                wide: .new,
                newMenu: {
                    Button { newTask = NewTask(kind: activeKind ?? .routine) } label: { Label("Set up manually", systemImage: "plus") }
                    Button(action: startWithAssistant) { Label("Create with Assistant", systemImage: "cpu") }
                }
            )
        }
        .sheet(isPresented: $isFilterPresented) {
            AutomationFilterSheet(activeTab: $activeTab, sortBy: $sortBy, routinesViewRaw: $routinesViewRaw)
        }
        .sheet(item: $newTask, onDismiss: { Task { await loadAll() } }) { new in
            AutomationWizardSheet(
                store: store,
                draft: AutomationTaskDraft(kind: new.kind, slotCron: new.cron, targetDate: new.targetDate)
            )
        }
        .sheet(item: $selectedTask) { task in
            AutomationTaskDetailSheet(
                task: task,
                isRunning: runningTaskId == task.id,
                onToggle: { active in Task { await setActive(task, active) } },
                onRunNow: { Task { await runNow(task) } },
                onDelete: { Task { await delete(task) } },
                edit: editContext(for: task)
            )
        }
        .sheet(item: $selectedRoutine) { picked in
            AutomationRoutineDetailsSheet(routineId: picked.id, store: store, onClose: { selectedRoutine = nil })
        }
        .task {
            // The drawer's quick scheduled task lands here on heartbeats.
            switch modeStore.automationKind {
            case .cron: activeTab = .heartbeat
            case .routines: activeTab = .routine
            case .loops: activeTab = .loop
            case .overview: break
            }
            modeStore.automationKind = .overview
            await loadAll()
        }
    }

    // MARK: - Chrome

    /// `filtersOn`: the filter button's dot.
    private var filtersOn: Bool { activeTab != .all || sortBy != .date || routinesView != .list }

    /// Desktop's tab strip and List / Graph / Calendar switch, on the screen
    /// rather than behind the filter button.
    private var typeAndViewBar: some View {
        VStack(alignment: .leading, spacing: 8) {
            AutomationTypeTabs(selection: $activeTab)
            AutomationViewToggle(selection: $routinesViewRaw)
        }
    }

    private var keepAwakeBanner: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .center, spacing: 8) {
                Image(systemName: "calendar.badge.checkmark")
                    .font(.system(size: 16))
                    .foregroundColor(Color("AccentPrimary"))
                Text("Automation tasks only run while your computer is awake and online.")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                    .fixedSize(horizontal: false, vertical: true)
            }
            HStack(spacing: 8) {
                Text("Keep awake")
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextTertiary"))
                AutomationToggleSwitch(isOn: $keepAwake, label: "Keep awake")
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("BgPanel")))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Color("BorderStrong"), lineWidth: 1))
    }

    private func errorBanner(_ message: String) -> some View {
        HStack(spacing: 8) {
            Image(systemName: "exclamationmark.triangle")
            Text(message).fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
            Button("Dismiss") {
                error = nil
                store.actionError = nil
            }
            .font(.system(size: 12))
        }
        .font(.system(size: 13))
        .foregroundColor(Theme.statusError)
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Theme.statusError.opacity(0.08)))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Theme.statusError.opacity(0.3), lineWidth: 1))
        .padding(.bottom, 16)
    }

    // MARK: - Main content

    private var actions: AutomationTaskActions {
        AutomationTaskActions(
            runNow: { task in Task { await runNow(task) } },
            setActive: { task, active in Task { await setActive(task, active) } },
            delete: { task in Task { await delete(task) } },
            new: { cron, day in newTask = NewTask(kind: activeKind ?? .routine, cron: cron, targetDate: Self.ymd(day)) },
            move: { task, cron in Task { await move(task, cron: cron) } },
            edit: { task in editContext(for: task) }
        )
    }

    /// What the task's Edit form starts from, read from its full record.
    private func editContext(for task: AutomationTaskItem) -> AutomationTaskEditContext? {
        let draft: AutomationTaskDraft?
        switch task.type {
        case .goal: draft = store.goals.first { $0.id == task.id }.map { AutomationTaskDraft(goal: $0) }
        case .routine: draft = store.routines.first { $0.id == task.id }.map { AutomationTaskDraft(routine: $0) }
        case .loop: draft = loops.first { $0.id == task.id }.map { AutomationTaskDraft(loop: $0) }
        case .heartbeat: draft = cron.job(withId: task.id).map { AutomationTaskDraft(job: $0) }
        }
        return draft.map { draft in
            AutomationTaskEditContext(draft: draft, store: store, onSaved: { Task { await loadAll() } })
        }
    }

    /// Heartbeats no listed agent runs: the default model's, a bot's own chat
    /// jobs, or a deleted agent's. The agent list can't reach these.
    private var unassignedHeartbeats: [AutomationTaskItem] {
        filteredTasks.filter { task in
            guard let id = task.agentId, !id.isEmpty else { return true }
            return agentHub.agent(withId: id) == nil
        }
    }

    /// The tab's kind; nil on All, which mixes them.
    private var activeKind: AutomationTaskItem.Kind? { AutomationTaskItem.Kind(rawValue: activeTab.rawValue) }

    private static func ymd(_ day: Date) -> String {
        let c = Calendar.current.dateComponents([.year, .month, .day], from: day)
        return String(format: "%04d-%02d-%02d", c.year ?? 0, c.month ?? 0, c.day ?? 0)
    }

    /// Routines that can run: a routine with no bot isn't on the graph or calendar.
    private var scheduledRoutines: [AutomationRoutine] {
        visibleRoutines
            .filter { !($0.agentId ?? "").isEmpty }
            .sorted { a, b in
                let (botA, botB) = (a.agentId ?? "", b.agentId ?? "")
                return botA != botB ? botA < botB : a.createdAt < b.createdAt
            }
    }

    /// Which view each tab shows: mainContent in AutomationTasksView.tsx with
    /// `routineViews` on, so the view switch applies to every tab.
    @ViewBuilder
    private var mainContent: some View {
        switch (routinesView, activeTab) {
        case (.graph, .routine):
            AutomationRoutinesGraph(
                routines: scheduledRoutines,
                isLoaded: !isLoading,
                selectedId: selectedRoutine?.id,
                onSelect: { selectedRoutine = RoutineSelection(id: $0.id) },
                onRunNow: { routine in Task { await runRoutineNow(routine) } },
                onNew: { newTask = NewTask(kind: .routine) }
            )
        case (.calendar, .routine):
            AutomationRoutinesCalendar(
                routines: scheduledRoutines,
                store: store,
                onNew: { cron, day in newTask = NewTask(kind: .routine, cron: cron, targetDate: Self.ymd(day)) }
            )
        case (.graph, _):
            AutomationTasksGraph(
                tasks: filteredTasks,
                allTasks: tasks,
                kind: activeKind,
                isLoaded: !isLoading || !tasks.isEmpty,
                runningId: runningTaskId,
                onOpen: { selectedTask = $0 },
                onRunNow: { task in Task { await runNow(task) } }
            )
        case (.calendar, _):
            AutomationTasksCalendar(tasks: filteredTasks, kind: activeKind, runningId: runningTaskId, actions: actions)
        case (.list, .heartbeat):
            if let id = selectedHeartbeatAgent, let agent = agentHub.agent(withId: id) {
                HeartbeatSchedulerView(agent: agent, onClose: { selectedHeartbeatAgent = nil })
            } else {
                VStack(alignment: .leading, spacing: 20) {
                    HeartbeatAgentPicker(query: searchQuery, onPick: { selectedHeartbeatAgent = $0.id })
                    let unassigned = unassignedHeartbeats
                    if !unassigned.isEmpty {
                        Text("Not assigned to an agent")
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundColor(Color("TextSecondary"))
                        LazyVStack(spacing: 20) {
                            ForEach(unassigned, id: \.key) { task in
                                AutomationTaskCard(
                                    task: task,
                                    isRunning: runningTaskId == task.id,
                                    onOpen: { selectedTask = task },
                                    onRunNow: { Task { await runNow(task) } }
                                )
                            }
                        }
                    }
                }
            }
        case (.list, .routine):
            routineList
        case (.list, _):
            taskCards
        }
    }

    /// Routines as run rows; routines with no bot keep their card.
    private var routineList: some View {
        let rows = visibleRoutines.filter { !($0.agentId ?? "").isEmpty }
        let needBot = filteredTasks.filter { ($0.agentId ?? "").isEmpty }
        return VStack(spacing: 24) {
            VStack(spacing: 0) {
                if rows.isEmpty {
                    emptyState(
                        title: searchQuery.isEmpty ? "No routines yet" : "No routines match your search",
                        message: "Tap 'New task' to add a routine."
                    )
                    .padding(.vertical, 40)
                } else {
                    ForEach(Array(rows.enumerated()), id: \.element.id) { index, routine in
                        if index > 0 { Divider().background(Color("BorderSubtle")) }
                        RoutineRunRowView(routine: routine, store: store)
                            .padding(.horizontal, 10)
                            .padding(.vertical, 10)
                    }
                }
            }
            .padding(8)
            .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Color("BgPanel")))
            .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).stroke(Color("BorderStrong"), lineWidth: 1))
            if !needBot.isEmpty {
                LazyVStack(spacing: 20) {
                    ForEach(needBot, id: \.key) { task in
                        AutomationTaskCard(
                            task: task,
                            isRunning: runningTaskId == task.id,
                            onOpen: { selectedTask = task },
                            onRunNow: { Task { await runNow(task) } }
                        )
                    }
                }
            }
        }
    }

    @ViewBuilder
    private var taskCards: some View {
        let shown = filteredTasks
        if !shown.isEmpty {
            LazyVStack(spacing: 20) {
                ForEach(shown, id: \.key) { task in
                    AutomationTaskCard(
                        task: task,
                        isRunning: runningTaskId == task.id,
                        onOpen: { selectedTask = task },
                        onRunNow: { Task { await runNow(task) } }
                    )
                }
            }
        } else if isLoading {
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text("Loading automation tasks…")
            }
            .font(.system(size: 13))
            .foregroundColor(Color("TextTertiary"))
            .frame(maxWidth: .infinity)
            .padding(.vertical, 80)
        } else {
            emptyState(
                title: searchQuery.isEmpty ? "No automation tasks yet" : "No tasks match your search",
                message: searchQuery.isEmpty
                    ? "Tap 'New task' to configure your first Goal, Routine, or Loop."
                    : "Try another search or type."
            )
            .padding(.vertical, 80)
        }
    }

    private func emptyState(title: String, message: String) -> some View {
        VStack(spacing: 10) {
            Image(systemName: "clock")
                .font(.system(size: 48))
                .foregroundColor(Color("TextTertiary"))
                .opacity(0.25)
                .padding(.bottom, 6)
            Text(title)
                .font(.system(size: 16, weight: .bold))
                .foregroundColor(Color("TextSecondary"))
            Text(message)
                .font(.system(size: 13))
                .foregroundColor(Color("TextTertiary"))
                .lineSpacing(5)
                .frame(maxWidth: 280)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity)
        .padding(.horizontal, 24)
    }

    // MARK: - Actions

    private func loadAll() async {
        isLoading = true
        defer { isLoading = false }
        await store.load()
        await cron.refresh()
        var failed = store.loadError != nil
        do {
            loops = try await AutomationClient.shared.listLoops()
        } catch {
            failed = true
        }
        error = failed ? "Failed to load automation tasks" : nil
    }

    /// The run happens on the server; pick up its result shortly.
    private func refreshAfterRun() {
        Task {
            for delay in [4, 11] {
                try? await Task.sleep(for: .seconds(delay))
                await loadAll()
            }
        }
    }

    private func runRoutineNow(_ routine: AutomationRoutine) async {
        await store.runNow(routine)
        refreshAfterRun()
    }

    /// A dragged task's new cron, through the same update route as the web's save.
    private func move(_ task: AutomationTaskItem, cron expression: String) async {
        do {
            switch task.type {
            case .goal: return
            case .routine:
                if let routine = store.routines.first(where: { $0.id == task.id }) {
                    await store.moveRoutine(routine, cron: expression)
                }
            case .loop:
                try await AutomationClient.shared.updateLoopSchedule(id: task.id, cron: expression)
                await loadAll()
            case .heartbeat:
                try await cron.updateSchedule(id: task.id, schedule: expression)
            }
        } catch {
            self.error = "Failed to update automation"
        }
    }

    private func startWithAssistant() {
        // The web opens Chat with "/schedule " typed; here a fresh chat opens.
        modeStore.selectBarItem(.chats)
        NotificationCenter.default.post(name: .shellNewChat, object: nil)
    }

    private func runNow(_ task: AutomationTaskItem) async {
        runningTaskId = task.id
        defer { runningTaskId = nil }
        do {
            switch task.type {
            case .goal: return
            case .routine:
                if let routine = store.routines.first(where: { $0.id == task.id }) { await store.runNow(routine) }
            case .loop: try await AutomationClient.shared.runLoop(id: task.id)
            case .heartbeat: try await cron.runNow(id: task.id)
            }
            refreshAfterRun()
        } catch {
            self.error = "Failed to run \(task.name)"
        }
    }

    private func setActive(_ task: AutomationTaskItem, _ active: Bool) async {
        do {
            switch task.type {
            case .goal:
                if let goal = store.goals.first(where: { $0.id == task.id }) {
                    await store.setGoalStatus(goal, active ? "active" : "paused")
                }
            case .routine:
                if let routine = store.routines.first(where: { $0.id == task.id }) { await store.setEnabled(routine, active) }
            case .loop:
                try await AutomationClient.shared.setLoopEnabled(id: task.id, enabled: active)
            case .heartbeat:
                if active { try await cron.resume(id: task.id) } else { try await cron.pause(id: task.id) }
            }
            await loadAll()
        } catch {
            self.error = "Failed to update status for \(task.name)"
        }
    }

    private func delete(_ task: AutomationTaskItem) async {
        do {
            switch task.type {
            case .goal:
                if let goal = store.goals.first(where: { $0.id == task.id }) { await store.deleteGoal(goal) }
            case .routine:
                if let routine = store.routines.first(where: { $0.id == task.id }) { await store.delete(routine) }
            case .loop:
                try await AutomationClient.shared.deleteLoop(id: task.id)
                loops.removeAll { $0.id == task.id }
            case .heartbeat:
                try await cron.deleteJob(id: task.id)
            }
            selectedTask = nil
        } catch {
            self.error = "Failed to delete \(task.name)"
        }
    }
}

// MARK: - Model

/// One row of the unified list (the web's UnifiedTask).
struct AutomationTaskItem: Identifiable, Hashable {
    enum Kind: String {
        case goal, routine, loop, heartbeat

        var label: String {
            switch self {
            case .goal: return "Goal"
            case .routine: return "Routine"
            case .loop: return "Loop"
            case .heartbeat: return "Heartbeat"
            }
        }

        /// Phosphor Target / Clock / Repeat / Heartbeat (AutomationSchedule TYPE_ICON).
        var icon: String {
            switch self {
            case .goal: return "target"
            case .routine: return "clock"
            case .loop: return "repeat"
            case .heartbeat: return "waveform.path.ecg"
            }
        }

        /// The badge's words (text-*-600); its fill and edge use `scheduleColor` (*-500).
        var tint: Color {
            switch self {
            case .goal: return Color(hex: "#2563EB")
            case .routine: return Color(hex: "#059669")
            case .loop: return Color(hex: "#9333EA")
            case .heartbeat: return Color(hex: "#D97706")
            }
        }
    }

    let type: Kind
    let id: String
    let name: String
    let description: String
    let status: String
    let isActive: Bool
    let frequency: String
    let schedule: String?
    let createdAt: Date?
    let agentId: String?
    let executorName: String
    let progress: Double?
    let targetDate: String?
    /// The goal this task serves.
    let goalId: String?
    let nextRun: String?
    let lastRun: String?
    /// What the bot is told to do, for the details.
    var instructions: String? = nil
    /// The goal this task serves, by name.
    var parentGoalName: String? = nil

    /// The raw cron or interval the calendar and graph read.
    var scheduleExpression: String? { schedule }

    var key: String { "\(type.rawValue)-\(id)" }

    var targetLine: String {
        if type == .goal, let targetDate, let date = libraryParseDate(targetDate) ?? Self.day(targetDate) {
            return "Target: \(date.formatted(date: .numeric, time: .omitted))"
        }
        if let schedule, !schedule.isEmpty { return schedule }
        return "Manual"
    }

    private static func day(_ raw: String) -> Date? {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyy-MM-dd"
        return formatter.date(from: raw)
    }
}

// MARK: - Card

/// TaskCard: type badge + frequency, Active/Paused + Run now, name,
/// description, and a footer with the executor (or progress) and schedule.
struct AutomationTaskCard: View {
    let task: AutomationTaskItem
    let isRunning: Bool
    let onOpen: () -> Void
    let onRunNow: () -> Void

    var body: some View {
        Button(action: onOpen) {
            VStack(alignment: .leading, spacing: 0) {
                HStack(alignment: .top) {
                    HStack(spacing: 8) {
                        Text(task.type.label.uppercased())
                            .font(.system(size: 10, weight: .bold))
                            .tracking(0.5)
                            .foregroundColor(task.type.tint)
                            .padding(.horizontal, 10)
                            .padding(.vertical, 2)
                            .background(Capsule().fill(task.type.scheduleColor.opacity(0.1)))
                            .overlay(Capsule().stroke(task.type.scheduleColor.opacity(0.2), lineWidth: 1))
                        if task.type != .goal {
                            Text(task.frequency)
                                .font(.system(size: 11))
                                .foregroundColor(Color("TextTertiary"))
                        }
                    }
                    Spacer()
                    HStack(spacing: 6) {
                        Text(task.isActive ? "Active" : "Paused")
                            .font(.system(size: 11, weight: .semibold))
                            .foregroundColor(task.isActive ? Color(hex: "#059669") : Color("TextTertiary"))
                            .padding(.horizontal, 8)
                            .padding(.vertical, 2)
                            .background(RoundedRectangle(cornerRadius: 6).fill(task.isActive ? Color(hex: "#10B981").opacity(0.1) : Color(hex: "#71717A").opacity(0.1)))
                            .overlay(RoundedRectangle(cornerRadius: 6).stroke(task.isActive ? Color(hex: "#10B981").opacity(0.2) : Color.clear, lineWidth: 1))
                        if task.type != .goal {
                            Button(action: onRunNow) {
                                Group {
                                    if isRunning { ProgressView().controlSize(.mini) } else { Image(systemName: "play").font(.system(size: 12)) }
                                }
                                .foregroundColor(Color("TextSecondary"))
                                .frame(width: 28, height: 28)
                                .background(RoundedRectangle(cornerRadius: 8).fill(Color("BgPanel")))
                                .overlay(RoundedRectangle(cornerRadius: 8).stroke(Color("BorderStrong"), lineWidth: 1))
                                .contentShape(Rectangle().inset(by: -8))
                            }
                            .buttonStyle(.plain)
                            .disabled(isRunning)
                            .accessibilityLabel("Run Now")
                        }
                    }
                }
                .padding(.bottom, 12)

                Text(task.name)
                    .font(.system(size: 15, weight: .bold))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                    .padding(.bottom, 6)
                Text(task.description.isEmpty ? "No description provided." : task.description)
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                    .lineSpacing(5)
                    .lineLimit(2)
                    .fixedSize(horizontal: false, vertical: true)

                Spacer(minLength: 0)
                Rectangle().fill(Color("BorderSubtle")).frame(height: 1).padding(.top, 16)
                HStack {
                    if task.type == .goal {
                        HStack(spacing: 4) {
                            Image(systemName: "target").font(.system(size: 12))
                            Text("Progress: \(Int(task.progress ?? 0))%")
                        }
                    } else {
                        HStack(spacing: 4) {
                            Image(systemName: "sparkle").font(.system(size: 12))
                            BotNameText(task.executorName, size: 11, ink: "TextTertiary").lineLimit(1)
                        }
                    }
                    Spacer()
                    Text(task.targetLine).lineLimit(1)
                }
                .font(.system(size: 11))
                .foregroundColor(Color("TextTertiary"))
                .padding(.top, 12)
            }
            .padding(24)
            .frame(maxWidth: .infinity, minHeight: 160, alignment: .topLeading)
            .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("BgPanel")))
            .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Color("BorderStrong"), lineWidth: 1))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

// MARK: - Detail

/// TaskDetailOverlay: what the task is and does, its switch, Edit, Run now and
/// Delete.
struct AutomationTaskDetailSheet: View {
    let task: AutomationTaskItem
    let isRunning: Bool
    let onToggle: (Bool) -> Void
    let onRunNow: () -> Void
    let onDelete: () -> Void
    /// Nil when the task's full record hasn't loaded, so there's nothing to edit yet.
    let edit: AutomationTaskEditContext?

    @Environment(\.dismiss) private var dismiss
    @State private var active: Bool
    @State private var confirmDelete = false
    @State private var editDraft: AutomationTaskDraft? = nil

    init(task: AutomationTaskItem, isRunning: Bool, onToggle: @escaping (Bool) -> Void,
         onRunNow: @escaping () -> Void, onDelete: @escaping () -> Void,
         edit: AutomationTaskEditContext? = nil) {
        self.edit = edit
        self.task = task
        self.isRunning = isRunning
        self.onToggle = onToggle
        self.onRunNow = onRunNow
        self.onDelete = onDelete
        _active = State(initialValue: task.isActive)
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                header
                Rectangle().fill(Color("BorderSubtle")).frame(height: 1)
                detailsList.padding(24)
            }
        }
        .background(Color("BgPanel"))
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
        .presentationBackground(Color("BgPanel"))
        .confirmationDialog("Delete \(task.name)?", isPresented: $confirmDelete, titleVisibility: .visible) {
            Button("Delete", role: .destructive, action: onDelete)
        } message: {
            Text("This can't be undone.")
        }
        .sheet(item: $editDraft) { draft in
            if let edit {
                // As on the web, a save closes the details too: its copy is now old.
                AutomationWizardSheet(store: edit.store, draft: draft, onSaved: {
                    edit.onSaved()
                    dismiss()
                })
            }
        }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(alignment: .top, spacing: 8) {
                VStack(alignment: .leading, spacing: 6) {
                    Text(task.name)
                        .font(.system(size: 18, weight: .bold))
                        .foregroundColor(Color("TextPrimary"))
                        .fixedSize(horizontal: false, vertical: true)
                    HStack(spacing: 8) {
                        Text(task.type.label)
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundColor(Color("TextPrimary"))
                            .padding(.horizontal, 10)
                            .padding(.vertical, 2)
                            .background(Capsule().fill(Color(hex: "#71717A").opacity(0.1)))
                        if task.type != .goal, task.isActive, let next = libraryParseDate(task.nextRun) {
                            Text("Next run: \(AutomationScheduleLogic.when(next))")
                                .font(.system(size: 12))
                                .foregroundColor(Color("TextTertiary"))
                                .lineLimit(1)
                        }
                    }
                }
                Spacer(minLength: 0)
                Button { dismiss() } label: {
                    Image(systemName: "xmark")
                        .font(.system(size: 16))
                        .foregroundColor(Color("TextSecondary"))
                        .frame(width: 44, height: 44)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .padding(.top, -10)
                .padding(.trailing, -10)
                .accessibilityLabel("Close")
            }
            HStack(spacing: 8) {
                if let edit {
                    actionButton("Edit", icon: "pencil") { editDraft = edit.draft }
                }
                actionButton("Delete", icon: "trash") { confirmDelete = true }
                if task.type != .goal {
                    actionButton(isRunning ? "Running…" : "Run Now", icon: "play", primary: true, busy: isRunning, perform: onRunNow)
                }
            }
        }
        .padding(24)
    }

    private var detailsList: some View {
        VStack(alignment: .leading, spacing: 20) {
            HStack {
                Text("Status (\(active ? "Active" : "Paused"))")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
                Spacer()
                AutomationToggleSwitch(isOn: $active, label: "Status")
            }
            .padding(14)
            .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("BgPanel")))
            .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Color("BorderStrong"), lineWidth: 1))
            .onChange(of: active) { _, value in onToggle(value) }

            VStack(alignment: .leading, spacing: 16) {
                item("Description", task.description.isEmpty ? "No description provided." : task.description)
                if task.type == .goal {
                    VStack(alignment: .leading, spacing: 4) {
                        itemLabel("Progress")
                        AutomationProgressBar(progress: task.progress ?? 0)
                    }
                    item("Target Date", task.targetLine.replacingOccurrences(of: "Target: ", with: ""))
                } else {
                    if let instructions = task.instructions, !instructions.isEmpty {
                        item("Instructions / Prompt", instructions)
                    }
                    item("Executor", task.executorName, icon: task.agentId == nil ? "sparkle" : "cpu")
                    if let goal = task.parentGoalName { item("Parent Goal", goal) }
                    item("Frequency", task.frequency)
                    if let schedule = task.schedule, !schedule.isEmpty { item("Expression", schedule) }
                    if let last = libraryParseDate(task.lastRun) {
                        item("Last Run", AutomationScheduleLogic.when(last))
                    }
                }
            }

            if task.type == .loop {
                LoopRunHistorySection(loopId: task.id)
            } else if task.type == .routine {
                RoutineRunHistorySection(runs: edit?.store.routines.first { $0.id == task.id }?.recentRuns ?? [])
            }
        }
    }

    private func itemLabel(_ title: String) -> some View {
        Text(title.uppercased())
            .font(.system(size: 10, weight: .bold))
            .tracking(0.5)
            .foregroundColor(Color("TextTertiary"))
    }

    /// DetailItem: a small upper-case label over its value.
    private func item(_ title: String, _ value: String, icon: String? = nil) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            itemLabel(title)
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                if let icon { Image(systemName: icon).font(.system(size: 12)) }
                Text(value)
                    .font(.system(size: 13))
                    .lineSpacing(5)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .foregroundColor(Color("TextSecondary"))
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    /// ActionButton: one of a row of equal buttons.
    private func actionButton(_ title: String, icon: String, primary: Bool = false, busy: Bool = false, perform: @escaping () -> Void) -> some View {
        Button(action: perform) {
            HStack(spacing: 6) {
                if busy { ProgressView().controlSize(.mini) } else { Image(systemName: icon).font(.system(size: 13)) }
                Text(title).font(.system(size: 12, weight: .semibold))
            }
            .foregroundColor(busy ? Color("TextTertiary") : (primary ? Color("BgPanel") : Color("TextSecondary")))
            .frame(maxWidth: .infinity, minHeight: 36)
            .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(busy ? Color(hex: "#71717A").opacity(0.1) : (primary ? Color("TextPrimary") : Color("BgPanel"))))
            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(primary || busy ? Color.clear : Color("BorderStrong"), lineWidth: 1))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(busy)
    }
}

/// `ToggleSwitch` (AutomationTasksView.tsx): green when on, in a 44 pt tap area.
struct AutomationToggleSwitch: View {
    @Binding var isOn: Bool
    let label: String

    var body: some View {
        Button { isOn.toggle() } label: {
            Capsule()
                .fill(isOn ? Color(hex: "#10B981").opacity(0.2) : Color(hex: "#71717A").opacity(0.2))
                .frame(width: 40, height: 22)
                .overlay(alignment: isOn ? .trailing : .leading) {
                    Circle()
                        .fill(isOn ? Color(hex: "#059669") : Color("TextTertiary"))
                        .frame(width: 16, height: 16)
                        .padding(.horizontal, 3)
                }
                .animation(.easeOut(duration: 0.15), value: isOn)
                .frame(height: 44)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
        .accessibilityValue(isOn ? "On" : "Off")
    }
}

/// One run in the details' history: status chip, date, error (TaskDetailOverlay).
private struct AutomationRunHistoryCard: View {
    let status: String
    let trailing: String?
    let date: String
    let detail: String?
    let detailIsError: Bool
    var onStop: (() -> Void)? = nil

    private var tint: Color {
        switch status {
        case "completed", "succeeded", "success": return Color(hex: "#10B981")
        case "failed": return Color(hex: "#EF4444")
        default: return Color(hex: "#F59E0B")
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 8) {
                Text(status.uppercased())
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundColor(tint)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 2)
                    .background(RoundedRectangle(cornerRadius: 4, style: .continuous).fill(tint.opacity(0.1)))
                Spacer(minLength: 0)
                if let trailing {
                    Text(trailing).foregroundColor(Color("TextTertiary")).lineLimit(1)
                }
                if let onStop {
                    Button("Stop", action: onStop)
                        .underline()
                        .foregroundColor(Color("TextPrimary"))
                        .frame(minHeight: 44)
                        .buttonStyle(.plain)
                }
            }
            Text(date).foregroundColor(Color("TextSecondary"))
            if let detail, !detail.isEmpty {
                Text(detail)
                    .foregroundColor(detailIsError ? Theme.statusError : Color("TextTertiary"))
                    .lineLimit(3)
            }
        }
        .font(.system(size: 12))
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("BgPanel")))
        .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Color("BorderStrong"), lineWidth: 1))
    }
}

/// The "Run history" block over a rule.
private struct AutomationRunHistoryBlock<Content: View>: View {
    @ViewBuilder var content: () -> Content

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Rectangle().fill(Color("BorderSubtle")).frame(height: 1)
            Label("Run history", systemImage: "clock")
                .font(.system(size: 13, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
            content()
        }
    }
}

/// A routine's run history, from the runs on its row.
private struct RoutineRunHistorySection: View {
    let runs: [AutomationRunTick]

    var body: some View {
        AutomationRunHistoryBlock {
            if runs.isEmpty {
                Text("No runs recorded yet.")
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextSecondary"))
            } else {
                VStack(spacing: 8) {
                    ForEach(runs.prefix(20)) { run in
                        AutomationRunHistoryCard(
                            status: run.status,
                            trailing: nil,
                            date: libraryParseDate(run.startedAt)?.formatted(.dateTime.month(.abbreviated).day().hour().minute()) ?? "—",
                            detail: run.reason,
                            detailIsError: true
                        )
                    }
                }
            }
        }
    }
}

/// A loop's run history, read from `GET …/loops/:id/runs`.
private struct LoopRunHistorySection: View {
    let loopId: String
    @State private var runs: [AutomationRunRecord] = []
    @State private var isLoading = true
    @State private var error: String? = nil

    var body: some View {
        AutomationRunHistoryBlock {
            if let error {
                VStack(alignment: .leading, spacing: 8) {
                    Text(FriendlyErrorMessage.from(error))
                        .font(.system(size: 12))
                        .foregroundColor(Theme.statusError)
                    Button("Try again") { Task { await load() } }
                        .font(.system(size: 13, weight: .medium))
                        .frame(minHeight: 44)
                }
            }
            if isLoading && runs.isEmpty {
                ProgressView().frame(maxWidth: .infinity).padding(.vertical, 16)
            } else if runs.isEmpty {
                if error == nil {
                    Text("No runs recorded yet.")
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextSecondary"))
                }
            } else {
                VStack(spacing: 8) {
                    ForEach(runs.prefix(20)) { run in
                        AutomationRunHistoryCard(
                            status: run.status,
                            trailing: run.triggeredBy,
                            date: libraryParseDate(run.startedAt ?? run.scheduledAt)?.formatted(.dateTime.month(.abbreviated).day().hour().minute()) ?? "—",
                            detail: run.error ?? run.output,
                            detailIsError: run.error != nil,
                            onStop: run.status == "running" ? { stop(run) } : nil
                        )
                    }
                }
            }
        }
        .task { await load() }
    }

    private func stop(_ run: AutomationRunRecord) {
        Task {
            do {
                try await AutomationClient.shared.cancelLoopRun(loopId: loopId, runId: run.id)
                await load()
            } catch {
                self.error = error.localizedDescription
            }
        }
    }

    private func load() async {
        isLoading = true
        defer { isLoading = false }
        do {
            runs = try await AutomationClient.shared.listLoopRuns(id: loopId)
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
    }
}

// MARK: - Type tabs and view switch

/// Arc Tabs with Desktop's labels and icons: a muted strip whose selected tab
/// is a raised card. It scrolls sideways on a narrow screen.
private struct AutomationTypeTabs: View {
    @Binding var selection: AutomationTasksPhoneView.TaskTab

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 4) {
                    ForEach(AutomationTasksPhoneView.TaskTab.allCases, id: \.self) { tab in
                        let on = selection == tab
                        Button { selection = tab } label: {
                            HStack(spacing: 6) {
                                Image(systemName: tab.icon).font(.system(size: 13))
                                Text(tab.label).font(.system(size: 13, weight: .medium)).fixedSize()
                            }
                            .foregroundColor(on ? Color("TextPrimary") : Color("TextTertiary"))
                            .padding(.horizontal, 12)
                            .frame(height: 36)
                            .background(
                                RoundedRectangle(cornerRadius: 7, style: .continuous)
                                    .fill(on ? Color("BgPanel") : Color.clear)
                                    .shadow(color: .black.opacity(on ? 0.1 : 0), radius: 1, y: 1)
                            )
                            .overlay(
                                RoundedRectangle(cornerRadius: 7, style: .continuous)
                                    .stroke(on ? Color("BorderSubtle") : Color.clear, lineWidth: 1)
                            )
                            .contentShape(Rectangle().inset(by: -4))
                        }
                        .buttonStyle(.plain)
                        .id(tab)
                        .accessibilityAddTraits(on ? .isSelected : [])
                    }
                }
                .padding(4)
            }
            .onChange(of: selection) { _, tab in
                withAnimation(.easeOut(duration: 0.2)) { proxy.scrollTo(tab, anchor: .center) }
            }
        }
        .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("FillNeutral")))
        .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
        .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Task type")
    }
}

/// `Toggle` in RoutinesSchedule.tsx: List, Graph, Calendar.
private struct AutomationViewToggle: View {
    @Binding var selection: String

    var body: some View {
        HStack(spacing: 0) {
            ForEach(AutomationTasksPhoneView.RoutinesView.allCases, id: \.self) { view in
                let on = selection == view.rawValue
                Button { selection = view.rawValue } label: {
                    Text(view.label)
                        .font(.system(size: 12, weight: .medium))
                        .foregroundColor(on ? Color("TextPrimary") : Color("TextTertiary"))
                        .padding(.horizontal, 10)
                        .padding(.vertical, 4)
                        .background(
                            RoundedRectangle(cornerRadius: 6, style: .continuous)
                                .fill(on ? Color("BgPanel") : Color.clear)
                                .shadow(color: .black.opacity(on ? 0.06 : 0), radius: 2, y: 1)
                        )
                        .contentShape(Rectangle().inset(by: -10))
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(on ? .isSelected : [])
            }
        }
        .padding(2)
        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("FillNeutral")))
        .accessibilityElement(children: .contain)
        .accessibilityLabel("View")
    }
}

// MARK: - Filter sheet

/// The filter button's sheet (QuietMenu on a phone): Type, Sort by, View, each
/// row checked when on; a pick closes it.
private struct AutomationFilterSheet: View {
    @Binding var activeTab: AutomationTasksPhoneView.TaskTab
    @Binding var sortBy: AutomationTasksPhoneView.TaskSort
    @Binding var routinesViewRaw: String
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                section("Type")
                ForEach(AutomationTasksPhoneView.TaskTab.allCases, id: \.self) { tab in
                    row(tab.label, checked: activeTab == tab) { activeTab = tab }
                }
                Divider().background(Color("BorderSubtle")).padding(.vertical, 6)
                section("Sort by")
                ForEach(AutomationTasksPhoneView.TaskSort.allCases, id: \.self) { sort in
                    row(sort.label, checked: sortBy == sort) { sortBy = sort }
                }
                Divider().background(Color("BorderSubtle")).padding(.vertical, 6)
                section("View")
                ForEach(AutomationTasksPhoneView.RoutinesView.allCases, id: \.self) { view in
                    row(view.label, checked: routinesViewRaw == view.rawValue) { routinesViewRaw = view.rawValue }
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
        }
        .presentationDetents([.height(640), .large])
        .presentationDragIndicator(.visible)
        .presentationCornerRadius(24)
        .presentationBackground(Color("BgPanel"))
    }

    private func section(_ title: String) -> some View {
        Text(title)
            .font(.system(size: 13))
            .foregroundColor(Color("TextTertiary"))
            .padding(.vertical, 6)
    }

    private func row(_ title: String, checked: Bool, action: @escaping () -> Void) -> some View {
        Button {
            action()
            dismiss()
        } label: {
            HStack {
                Text(title)
                    .font(.system(size: 16))
                    .foregroundColor(Color("TextPrimary"))
                Spacer()
                if checked {
                    Image(systemName: "checkmark")
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextTertiary"))
                }
            }
            .frame(minHeight: 44)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(checked ? .isSelected : [])
    }
}
