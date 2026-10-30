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
    @State private var isTypeChoicePresented = false
    @State private var newTask: NewTask? = nil

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
                goalId: routine.goalId, nextRun: routine.nextRunAt, lastRun: routine.lastRunAt
            ))
        }
        for loop in loops {
            out.append(AutomationTaskItem(
                type: .loop, id: loop.id, name: AutomationScheduleLogic.displayName(loop.name), description: loop.description ?? "",
                status: loop.status, isActive: loop.status == "active",
                frequency: loop.scheduleType == "cron" ? "daily" : "continuous",
                schedule: loop.scheduleExpression, createdAt: libraryParseDate(loop.createdAt),
                agentId: loop.agentId, executorName: executor(loop.agentId), progress: nil, targetDate: nil,
                goalId: loop.goalId, nextRun: nil, lastRun: nil
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
        return tasks
            .filter { activeTab == .all || $0.type.rawValue == activeTab.rawValue }
            .filter { query.isEmpty || $0.name.lowercased().contains(query) || $0.description.lowercased().contains(query) }
            .sorted { a, b in
                switch sortBy {
                case .name: return a.name.localizedCompare(b.name) == .orderedAscending
                case .type: return a.type.rawValue < b.type.rawValue
                case .date: return (a.createdAt ?? .distantPast) > (b.createdAt ?? .distantPast)
                }
            }
    }

    /// The Routines tab's rows, filtered by the search.
    private var visibleRoutines: [AutomationRoutine] {
        let query = searchQuery.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return store.routines.filter { query.isEmpty || $0.name.lowercased().contains(query) }
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
                wide: .new
            ) {
                Button { isTypeChoicePresented = true } label: { Label("Set up manually", systemImage: "plus") }
                Button(action: startWithAssistant) { Label("Create with Assistant", systemImage: "cpu") }
            }
        }
        .sheet(isPresented: $isFilterPresented) {
            AutomationFilterSheet(activeTab: $activeTab, sortBy: $sortBy, routinesViewRaw: $routinesViewRaw)
        }
        .confirmationDialog("New task", isPresented: $isTypeChoicePresented, titleVisibility: .visible) {
            Button("Goal") { newTask = NewTask(kind: .goal) }
            Button("Routine") { newTask = NewTask(kind: .routine) }
            Button("Loop") { newTask = NewTask(kind: .loop) }
            Button("Agent Heartbeat") { newTask = NewTask(kind: .heartbeat) }
            Button("Cancel", role: .cancel) {}
        }
        .sheet(item: $newTask, onDismiss: { Task { await loadAll() } }) { new in
            if new.kind == .goal {
                NewGoalSheet(store: store, targetDate: new.targetDate)
            } else {
                ScheduledTaskSheet(store: store, kind: new.kind, slotCron: new.cron)
            }
        }
        .sheet(item: $selectedTask) { task in
            AutomationTaskDetailSheet(
                task: task,
                isRunning: runningTaskId == task.id,
                onToggle: { active in Task { await setActive(task, active) } },
                onRunNow: { Task { await runNow(task) } },
                onDelete: { Task { await delete(task) } }
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

    private var keepAwakeBanner: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Image(systemName: "calendar.badge.checkmark")
                    .font(.system(size: 15))
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
                Toggle("Keep awake", isOn: $keepAwake)
                    .labelsHidden()
                    .tint(Color(hex: "#059669"))
                    .scaleEffect(0.8, anchor: .leading)
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
            move: { task, cron in Task { await move(task, cron: cron) } }
        )
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
            VStack(alignment: .leading, spacing: 20) {
                VStack(alignment: .leading, spacing: 8) {
                    Label("Agent Heartbeat Tasks", systemImage: "clock")
                        .font(.system(size: 16, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                    Text("Configure periodic tasks that agents execute automatically. Heartbeat tasks are stored in each agent's workspace and synced with the scheduler.")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                        .fixedSize(horizontal: false, vertical: true)
                }
                .padding(.horizontal, 20)
                .padding(.vertical, 16)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("BgPanel")))
                .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Color("BorderStrong"), lineWidth: 1))
                taskCards
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
                .font(.system(size: 36, weight: .light))
                .foregroundColor(Color("TextTertiary"))
            Text(title)
                .font(.system(size: 16, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
            Text(message)
                .font(.system(size: 14))
                .foregroundColor(Color("TextSecondary"))
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

        /// Phosphor Target / Clock / Repeat / Robot.
        var icon: String {
            switch self {
            case .goal: return "target"
            case .routine: return "clock"
            case .loop: return "repeat"
            case .heartbeat: return "cpu"
            }
        }

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
                            .background(Capsule().fill(task.type.tint.opacity(0.1)))
                            .overlay(Capsule().stroke(task.type.tint.opacity(0.2), lineWidth: 1))
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
                            .background(RoundedRectangle(cornerRadius: 6).fill(task.isActive ? Color(hex: "#10B981").opacity(0.1) : Color("FillNeutral")))
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
                    .lineLimit(2)
                    .fixedSize(horizontal: false, vertical: true)

                Divider().background(Color("BorderSubtle")).padding(.top, 16)
                HStack {
                    if task.type == .goal {
                        Label("Progress: \(Int(task.progress ?? 0))%", systemImage: "target")
                    } else {
                        HStack(spacing: 4) {
                            Image(systemName: "sparkle")
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

/// TaskDetailOverlay: what the task is and does, its switch, Run now and
/// Delete.
struct AutomationTaskDetailSheet: View {
    let task: AutomationTaskItem
    let isRunning: Bool
    let onToggle: (Bool) -> Void
    let onRunNow: () -> Void
    let onDelete: () -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var active: Bool
    @State private var confirmDelete = false

    init(task: AutomationTaskItem, isRunning: Bool, onToggle: @escaping (Bool) -> Void,
         onRunNow: @escaping () -> Void, onDelete: @escaping () -> Void) {
        self.task = task
        self.isRunning = isRunning
        self.onToggle = onToggle
        self.onRunNow = onRunNow
        self.onDelete = onDelete
        _active = State(initialValue: task.isActive)
    }

    var body: some View {
        NavigationStack {
            List {
                Section {
                    VStack(alignment: .leading, spacing: 8) {
                        Label(task.type.label, systemImage: task.type.icon)
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundColor(task.type.tint)
                        Text(task.name)
                            .font(.system(size: 20, weight: .semibold))
                            .foregroundColor(Color("TextPrimary"))
                        Text(task.description.isEmpty ? "No description provided." : task.description)
                            .font(.system(size: 14))
                            .foregroundColor(Color("TextSecondary"))
                    }
                    .padding(.vertical, 4)
                }
                Section {
                    Toggle("Active", isOn: $active)
                        .onChange(of: active) { _, value in onToggle(value) }
                    if task.type == .goal {
                        row("Progress", "\(Int(task.progress ?? 0))%")
                        row("Target", task.targetLine)
                    } else {
                        row("Schedule", task.schedule ?? "Manual")
                        row("Runs as", task.executorName)
                    }
                }
                Section {
                    if task.type != .goal {
                        Button(action: onRunNow) {
                            HStack {
                                Label("Run Now", systemImage: "play")
                                if isRunning { Spacer(); ProgressView() }
                            }
                        }
                        .disabled(isRunning)
                    }
                    Button(role: .destructive) { confirmDelete = true } label: { Label("Delete", systemImage: "trash") }
                }
            }
            .scrollContentBackground(.hidden)
            .background(Color("BgPrimary"))
            .navigationTitle(task.type.label)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
            }
            .confirmationDialog("Delete \(task.name)?", isPresented: $confirmDelete, titleVisibility: .visible) {
                Button("Delete", role: .destructive, action: onDelete)
            }
        }
        .presentationDetents([.medium, .large])
    }

    private func row(_ title: String, _ value: String) -> some View {
        HStack {
            Text(title).foregroundColor(Color("TextPrimary"))
            Spacer()
            Text(value).foregroundColor(Color("TextSecondary")).lineLimit(1)
        }
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
