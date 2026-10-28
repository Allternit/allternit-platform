import SwiftUI
import Charts

/// Bot routines, their schedule, and goals (the PWA's Automation Tasks →
/// Routines / Calendar / Graph + Goals, over allternit-api
/// `/api/v1/automation/*`). Each routine is a run row (RoutineRunRow.tsx):
/// schedule in words, next run or the live step, the last 14 runs as a
/// strip, Run now / Stop and an on/off switch; tap expands it.
struct AutomationHubView: View {
    enum Tab: String, CaseIterable, Identifiable {
        case routines = "Routines", calendar = "Calendar", graph = "Graph", goals = "Goals"
        var id: String { rawValue }
    }

    @StateObject private var store = AutomationStore()
    @State private var tab: Tab = .routines
    @State private var isScheduleSheetPresented = false
    @State private var isGoalSheetPresented = false

    var body: some View {
        VStack(spacing: 0) {
            Picker("View", selection: $tab) {
                ForEach(Tab.allCases) { Text($0.rawValue).tag($0) }
            }
            .pickerStyle(.segmented)
            .padding(.horizontal, 16)
            .padding(.vertical, 8)

            Group {
                switch tab {
                case .routines: routinesList
                case .calendar: AutomationCalendarView(routines: store.routines)
                case .graph: AutomationGraphView(routines: store.routines)
                case .goals: GoalsListView(store: store)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .background(Color("BgPrimary"))
        .navigationTitle("Automation")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button(action: {
                    if tab == .goals { isGoalSheetPresented = true } else { isScheduleSheetPresented = true }
                }) {
                    Image(systemName: "plus")
                }
                .accessibilityLabel(tab == .goals ? "New goal" : "New scheduled task")
            }
        }
        .sheet(isPresented: $isScheduleSheetPresented) {
            ScheduledTaskSheet(store: store)
        }
        .sheet(isPresented: $isGoalSheetPresented) {
            NewGoalSheet(store: store)
        }
        .task { await store.load() }
        .refreshable { await store.load() }
    }

    @ViewBuilder
    private var routinesList: some View {
        if store.isLoading && store.routines.isEmpty {
            ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if let error = store.loadError, store.routines.isEmpty {
            let offline = error.localizedCaseInsensitiveContains("offline") || error.localizedCaseInsensitiveContains("connect")
            FriendlyStateView(
                style: offline ? .offline : .error,
                icon: offline ? "wifi.slash" : "exclamationmark.triangle",
                title: offline ? "Your computer is offline" : "Couldn't load routines",
                message: FriendlyErrorMessage.from(error),
                actionTitle: "Try again",
                action: { Task { await store.load() } }
            )
        } else if store.routines.isEmpty {
            FriendlyStateView(
                style: .empty,
                icon: "clock.arrow.2.circlepath",
                title: "No routines yet",
                message: "Schedule a task and a bot runs it for you on time.",
                actionTitle: "New scheduled task",
                action: { isScheduleSheetPresented = true }
            )
        } else {
            List {
                if let error = store.actionError {
                    Text(error).font(.caption).foregroundColor(Theme.statusError)
                }
                ForEach(store.routines) { routine in
                    RoutineRunRowView(routine: routine, store: store)
                }
            }
            .listStyle(.insetGrouped)
            .scrollContentBackground(.hidden)
        }
    }
}

// MARK: - Store

@MainActor
final class AutomationStore: ObservableObject {
    @Published private(set) var routines: [AutomationRoutine] = []
    @Published private(set) var goals: [AutomationGoal] = []
    @Published private(set) var isLoading = false
    @Published private(set) var loadError: String? = nil
    @Published private(set) var goalsError: String? = nil
    @Published var actionError: String? = nil
    /// Live runs keyed by routine id, while polled.
    @Published private(set) var live: [String: AutomationLiveRun] = [:]

    private let client = AutomationClient.shared

    func load() async {
        isLoading = true
        loadError = nil
        defer { isLoading = false }
        do {
            routines = try await client.listRoutines()
        } catch {
            loadError = error.localizedDescription
        }
        do {
            goals = try await client.listGoals()
            goalsError = nil
        } catch {
            goalsError = error.localizedDescription
        }
    }

    func runNow(_ routine: AutomationRoutine) async {
        actionError = nil
        do {
            try await client.runNow(id: routine.id)
            await pollLive(routine.id)
        } catch {
            actionError = "Couldn't start \(routine.name). \(FriendlyErrorMessage.from(error.localizedDescription))"
        }
    }

    func stop(_ routine: AutomationRoutine) async {
        guard let runId = live[routine.id]?.runId else { return }
        do {
            try await client.cancelRun(routineId: routine.id, runId: runId)
            await pollLive(routine.id)
        } catch {
            actionError = "Couldn't stop \(routine.name). \(FriendlyErrorMessage.from(error.localizedDescription))"
        }
    }

    func setEnabled(_ routine: AutomationRoutine, _ enabled: Bool) async {
        do {
            try await client.setEnabled(id: routine.id, enabled: enabled)
            await load()
        } catch {
            actionError = "Couldn't update \(routine.name). \(FriendlyErrorMessage.from(error.localizedDescription))"
        }
    }

    func delete(_ routine: AutomationRoutine) async {
        do {
            try await client.deleteRoutine(id: routine.id)
            routines.removeAll { $0.id == routine.id }
        } catch {
            actionError = "Couldn't delete \(routine.name). \(FriendlyErrorMessage.from(error.localizedDescription))"
        }
    }

    /// One live poll; returns whether the run is still going.
    @discardableResult
    func pollLive(_ routineId: String) async -> Bool {
        guard let run = try? await client.live(id: routineId) else { return false }
        live[routineId] = run
        if !run.running { await load() }
        return run.running
    }

    func create(_ request: CreateAutomationRoutineRequest) async throws {
        let created = try await client.createRoutine(request)
        routines.insert(created, at: 0)
        await load()
    }

    func createGoal(_ request: CreateAutomationGoalRequest) async throws {
        let goal = try await client.createGoal(request)
        goals.insert(goal, at: 0)
    }

    func setGoalStatus(_ goal: AutomationGoal, _ status: String) async {
        do {
            try await client.updateGoal(id: goal.id, UpdateAutomationGoalRequest(status: status, progress: status == "completed" ? 100 : nil))
            goals = try await client.listGoals()
        } catch {
            goalsError = error.localizedDescription
        }
    }

    func deleteGoal(_ goal: AutomationGoal) async {
        do {
            try await client.deleteGoal(id: goal.id)
            goals.removeAll { $0.id == goal.id }
        } catch {
            goalsError = error.localizedDescription
        }
    }
}

// MARK: - Run row

struct RoutineRunRowView: View {
    let routine: AutomationRoutine
    @ObservedObject var store: AutomationStore
    @State private var isExpanded = false
    @State private var confirmDelete = false

    private var liveRun: AutomationLiveRun? {
        guard let run = store.live[routine.id], run.running else { return nil }
        return run
    }

    private var isRunning: Bool {
        liveRun != nil || routine.recentRuns?.first?.status == "running"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Button(action: { withAnimation(.easeOut(duration: 0.2)) { isExpanded.toggle() } }) {
                HStack(alignment: .top, spacing: 10) {
                    VStack(alignment: .leading, spacing: 3) {
                        Text(routine.name)
                            .font(.subheadline.weight(.semibold))
                            .foregroundColor(Color("TextPrimary"))
                        Text(CronWords.describe(type: routine.scheduleType, expression: routine.scheduleExpression))
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                        Text(statusLine)
                            .font(.caption)
                            .foregroundColor(isRunning ? Color("AccentPrimary") : Color("TextSecondary"))
                            .lineLimit(1)
                    }
                    Spacer()
                    Image(systemName: "chevron.right")
                        .font(.system(size: 11, weight: .bold))
                        .foregroundColor(Color("TextSecondary"))
                        .rotationEffect(.degrees(isExpanded ? 90 : 0))
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            RunStrip(runs: routine.recentRuns ?? [])

            HStack(spacing: 10) {
                if isRunning {
                    Button(action: { Task { await store.stop(routine) } }) {
                        Label("Stop", systemImage: "stop.fill").font(.caption.weight(.semibold))
                    }
                    .buttonStyle(.bordered)
                    .disabled(liveRun?.canStop == false || liveRun?.runId == nil)
                } else {
                    Button(action: { Task { await store.runNow(routine) } }) {
                        Label("Run now", systemImage: "play.fill").font(.caption.weight(.semibold))
                    }
                    .buttonStyle(.bordered)
                }
                Spacer()
                Toggle("On", isOn: Binding(
                    get: { routine.isEnabled },
                    set: { value in Task { await store.setEnabled(routine, value) } }
                ))
                .labelsHidden()
                .tint(Color("AccentPrimary"))
                .accessibilityLabel("\(routine.name) on")
            }

            if isExpanded {
                expanded
            }
        }
        .padding(.vertical, 4)
        .task(id: isRunning) {
            // Poll the live run every 3 s while it's going.
            guard isRunning else { return }
            while !Task.isCancelled {
                let stillRunning = await store.pollLive(routine.id)
                if !stillRunning { break }
                try? await Task.sleep(nanoseconds: 3_000_000_000)
            }
        }
        .confirmationDialog("Delete \(routine.name)?", isPresented: $confirmDelete, titleVisibility: .visible) {
            Button("Delete", role: .destructive) { Task { await store.delete(routine) } }
            Button("Cancel", role: .cancel) {}
        }
    }

    private var statusLine: String {
        if let liveRun {
            let current = liveRun.steps?.first(where: { $0.status == "in_progress" })?.content
            return "Running" + (current.map { " · \($0)" } ?? "")
        }
        if isRunning { return "Running" }
        if !routine.isEnabled { return "Off" }
        if let next = routine.nextRunAt.flatMap(DevicePairingView.parseDate) {
            return "Next " + next.formatted(.relative(presentation: .named))
        }
        if let last = routine.recentRuns?.first {
            return "Last run " + RunStrip.label(last.status).lowercased()
        }
        return "Not run yet"
    }

    @ViewBuilder
    private var expanded: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let liveRun {
                ForEach(Array((liveRun.steps ?? []).enumerated()), id: \.offset) { _, step in
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Image(systemName: step.status == "completed" ? "checkmark.circle.fill"
                              : step.status == "in_progress" ? "circle.dotted" : "circle")
                            .font(.caption)
                            .foregroundColor(step.status == "completed" ? Theme.statusSuccess : Color("TextSecondary"))
                        Text(step.content ?? "")
                            .font(.caption)
                            .foregroundColor(Color("TextPrimary"))
                    }
                }
                if let text = liveRun.text, !text.isEmpty {
                    Text(text)
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(6)
                }
            }
            if let description = routine.description, !description.isEmpty {
                Text(description)
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }
            if let runs = routine.recentRuns, !runs.isEmpty {
                Text("History")
                    .font(.caption.weight(.semibold))
                    .foregroundColor(Color("TextPrimary"))
                ForEach(runs.prefix(5)) { run in
                    HStack {
                        Circle().fill(RunStrip.color(run.status)).frame(width: 6, height: 6)
                        Text(RunStrip.label(run.status))
                        if let reason = run.reason { Text("· \(reason)") }
                        Spacer()
                        if let started = run.startedAt.flatMap(DevicePairingView.parseDate) {
                            Text(started.formatted(.dateTime.month(.abbreviated).day().hour().minute()))
                        }
                    }
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
                }
            }
            Button(role: .destructive, action: { confirmDelete = true }) {
                Label("Delete routine", systemImage: "trash").font(.caption)
            }
            .buttonStyle(.plain)
            .foregroundColor(Theme.statusError)
        }
        .padding(.top, 4)
    }
}

/// The last 14 runs, oldest left.
struct RunStrip: View {
    let runs: [AutomationRunTick]

    var body: some View {
        HStack(spacing: 3) {
            ForEach(Array(runs.prefix(14).reversed())) { run in
                RoundedRectangle(cornerRadius: 2)
                    .fill(Self.color(run.status))
                    .frame(width: 10, height: 14)
                    .accessibilityLabel(Self.label(run.status))
            }
            if runs.isEmpty {
                Text("No runs yet").font(.caption2).foregroundColor(Color("TextSecondary"))
            }
        }
    }

    static func color(_ status: String) -> Color {
        switch status {
        case "completed", "succeeded", "success": return Theme.statusSuccess
        case "failed": return Theme.statusError
        case "running", "pending": return Color("AccentPrimary")
        case "cancelled": return Theme.statusWarning
        default: return Color("TextSecondary").opacity(0.4)
        }
    }

    static func label(_ status: String) -> String {
        switch status {
        case "completed", "succeeded", "success": return "Done"
        case "failed": return "Failed"
        case "running": return "Running"
        case "pending": return "Waiting"
        case "cancelled": return "Stopped"
        case "skipped": return "Skipped"
        default: return status.capitalized
        }
    }
}

// MARK: - Calendar + graph

/// The next week of runs, by day (from each routine's cron).
struct AutomationCalendarView: View {
    let routines: [AutomationRoutine]

    private struct Slot: Identifiable {
        let id: String
        let at: Date
        let name: String
    }

    private struct Day: Identifiable {
        let day: Date
        let slots: [Slot]
        var id: Date { day }
    }

    private var days: [Day] {
        let calendar = Calendar.current
        let start = calendar.startOfDay(for: Date())
        var buckets: [Date: [Slot]] = [:]
        for routine in routines where routine.isEnabled {
            for date in CronWords.upcoming(routine, days: 7) {
                let slot = Slot(id: "\(routine.id)-\(date.timeIntervalSince1970)", at: date, name: routine.name)
                buckets[calendar.startOfDay(for: date), default: []].append(slot)
            }
        }
        return (0..<7).compactMap { offset in
            guard let day = calendar.date(byAdding: .day, value: offset, to: start) else { return nil }
            return Day(day: day, slots: (buckets[day] ?? []).sorted { $0.at < $1.at })
        }
    }

    var body: some View {
        List {
            ForEach(days) { entry in
                Section {
                    if entry.slots.isEmpty {
                        Text("Nothing scheduled").font(.caption).foregroundColor(Color("TextSecondary"))
                    }
                    ForEach(entry.slots) { slot in
                        HStack {
                            Text(slot.at.formatted(.dateTime.hour().minute()))
                                .font(.caption.monospacedDigit())
                                .foregroundColor(Color("TextSecondary"))
                                .frame(width: 64, alignment: .leading)
                            Text(slot.name).font(.subheadline).foregroundColor(Color("TextPrimary"))
                        }
                    }
                } header: {
                    Text(entry.day.formatted(.dateTime.weekday(.wide).month(.abbreviated).day()))
                }
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
    }
}

/// Runs per day for the last 14 days, done vs failed.
struct AutomationGraphView: View {
    let routines: [AutomationRoutine]

    private struct Point: Identifiable {
        let day: Date
        let outcome: String
        let count: Int
        var id: String { "\(day.timeIntervalSince1970)-\(outcome)" }
    }

    private var points: [Point] {
        let calendar = Calendar.current
        var counts: [Date: [String: Int]] = [:]
        let cutoff = calendar.date(byAdding: .day, value: -13, to: calendar.startOfDay(for: Date())) ?? Date()
        for routine in routines {
            for run in routine.recentRuns ?? [] {
                guard let started = run.startedAt.flatMap(DevicePairingView.parseDate), started >= cutoff else { continue }
                let outcome = run.status == "failed" ? "Failed" : (RunStrip.label(run.status) == "Done" ? "Done" : "Other")
                counts[calendar.startOfDay(for: started), default: [:]][outcome, default: 0] += 1
            }
        }
        return counts.flatMap { day, byOutcome in byOutcome.map { Point(day: day, outcome: $0.key, count: $0.value) } }
    }

    var body: some View {
        if points.isEmpty {
            FriendlyStateView(style: .empty, icon: "chart.bar", title: "No runs in the last two weeks",
                              message: "Runs show here as your routines go.")
        } else {
            VStack(alignment: .leading, spacing: 8) {
                Text("Runs per day · last 14 days")
                    .font(.subheadline.weight(.semibold))
                    .foregroundColor(Color("TextPrimary"))
                Chart(points) { point in
                    BarMark(
                        x: .value("Day", point.day, unit: .day),
                        y: .value("Runs", point.count)
                    )
                    .foregroundStyle(by: .value("Outcome", point.outcome))
                }
                .chartForegroundStyleScale([
                    "Done": Theme.statusSuccess,
                    "Failed": Theme.statusError,
                    "Other": Color("TextSecondary"),
                ])
                .frame(height: 220)
            }
            .padding(16)
            .frame(maxHeight: .infinity, alignment: .top)
        }
    }
}

// MARK: - Goals

struct GoalsListView: View {
    @ObservedObject var store: AutomationStore

    var body: some View {
        if let error = store.goalsError, store.goals.isEmpty {
            FriendlyStateView(style: .error, icon: "exclamationmark.triangle", title: "Couldn't load goals",
                              message: FriendlyErrorMessage.from(error), actionTitle: "Try again",
                              action: { Task { await store.load() } })
        } else if store.goals.isEmpty {
            FriendlyStateView(style: .empty, icon: "flag", title: "No goals yet",
                              message: "A goal groups the routines working toward it.")
        } else {
            List {
                ForEach(store.goals) { goal in
                    VStack(alignment: .leading, spacing: 6) {
                        HStack {
                            Text(goal.title)
                                .font(.subheadline.weight(.semibold))
                                .foregroundColor(Color("TextPrimary"))
                            Spacer()
                            Text(goal.priority.capitalized)
                                .font(.caption2.weight(.semibold))
                                .padding(.horizontal, 8).padding(.vertical, 2)
                                .background(Color("BgSecondary"))
                                .clipShape(Capsule())
                                .foregroundColor(Color("TextSecondary"))
                        }
                        if let description = goal.description, !description.isEmpty {
                            Text(description).font(.caption).foregroundColor(Color("TextSecondary")).lineLimit(2)
                        }
                        PlanUsageBar(label: goal.status.capitalized, detail: "", fraction: goal.progress / 100)
                        HStack(spacing: 10) {
                            if goal.status != "completed" {
                                Button("Mark done") { Task { await store.setGoalStatus(goal, "completed") } }
                            }
                            Button(goal.status == "paused" ? "Resume" : "Pause") {
                                Task { await store.setGoalStatus(goal, goal.status == "paused" ? "active" : "paused") }
                            }
                            Spacer()
                            Button(role: .destructive, action: { Task { await store.deleteGoal(goal) } }) {
                                Image(systemName: "trash")
                            }
                            .accessibilityLabel("Delete goal")
                        }
                        .font(.caption)
                        .buttonStyle(.bordered)
                    }
                    .padding(.vertical, 4)
                }
            }
            .listStyle(.insetGrouped)
            .scrollContentBackground(.hidden)
        }
    }
}

struct NewGoalSheet: View {
    @ObservedObject var store: AutomationStore
    @Environment(\.dismiss) private var dismiss
    @State private var title = ""
    @State private var details = ""
    @State private var priority = "medium"
    @State private var isSaving = false
    @State private var error: String? = nil

    var body: some View {
        NavigationStack {
            Form {
                TextField("Goal", text: $title)
                TextField("Details (optional)", text: $details, axis: .vertical).lineLimit(2...5)
                Picker("Priority", selection: $priority) {
                    ForEach(["low", "medium", "high", "urgent"], id: \.self) { Text($0.capitalized).tag($0) }
                }
                if let error { Text(error).font(.caption).foregroundColor(Theme.statusError) }
            }
            .navigationTitle("New goal")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button(isSaving ? "Saving…" : "Save") { Task { await save() } }
                        .disabled(title.trimmingCharacters(in: .whitespaces).isEmpty || isSaving)
                }
            }
        }
        .presentationDetents([.medium, .large])
    }

    @MainActor
    private func save() async {
        isSaving = true
        defer { isSaving = false }
        do {
            let trimmed = details.trimmingCharacters(in: .whitespacesAndNewlines)
            try await store.createGoal(CreateAutomationGoalRequest(
                title: title.trimmingCharacters(in: .whitespaces),
                description: trimmed.isEmpty ? nil : trimmed,
                priority: priority, agentId: nil, targetDate: nil))
            dismiss()
        } catch {
            self.error = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }
}
