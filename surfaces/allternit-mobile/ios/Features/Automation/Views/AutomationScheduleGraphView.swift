import SwiftUI

// Graph view of Automation Tasks at phone width. The web draws a React Flow
// canvas above `sm` and "the same cards, stacked" on a phone
// (AutomationSchedule.tsx AutomationGraph, RoutinesGraph.tsx); this is that
// phone branch, in the same flow order.

/// EmptyState (Arc) used by the graph views.
struct AutomationGraphEmptyState: View {
    let title: String
    let message: String
    var actionTitle: String? = nil
    var action: (() -> Void)? = nil

    var body: some View {
        VStack(spacing: 10) {
            Image(systemName: "calendar")
                .font(.system(size: 24, weight: .light))
                .foregroundColor(Color("TextTertiary"))
            Text(title)
                .font(.system(size: 16, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
            Text(message)
                .font(.system(size: 14))
                .foregroundColor(Color("TextSecondary"))
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
            if let actionTitle, let action {
                Button(action: action) {
                    Label(actionTitle, systemImage: "plus")
                        .font(.system(size: 13, weight: .medium))
                        .foregroundColor(Color("BgPrimary"))
                        .padding(.horizontal, 14)
                        .frame(minHeight: 44)
                        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("TextPrimary")))
                }
                .buttonStyle(.plain)
            }
        }
        .frame(maxWidth: .infinity)
        .padding(.horizontal, 24)
        .padding(.vertical, 40)
    }
}

/// The small outlined "Run now" on a flow card; its tap area reaches 44 pt.
private struct GraphRunNowButton: View {
    let disabled: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 4) {
                Image(systemName: "play.fill").font(.system(size: 9))
                Text("Run now").font(.system(size: 11))
            }
            .foregroundColor(Color("TextPrimary"))
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
            .contentShape(Rectangle().inset(by: -14))
        }
        .buttonStyle(.plain)
        .disabled(disabled)
        .opacity(disabled ? 0.5 : 1)
    }
}

/// The card shell: elevated fill, hairline, small shadow.
private struct GraphCard<Content: View>: View {
    var border: Color = Color("BorderSubtle")
    @ViewBuilder var content: () -> Content

    var body: some View {
        content()
            .padding(10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .fill(Color("BgPanel"))
                    .shadow(color: .black.opacity(0.06), radius: 2, y: 1)
            )
            .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(border, lineWidth: 1))
    }
}

// MARK: - Goals, loops, heartbeats, All

struct AutomationTasksGraph: View {
    let tasks: [AutomationTaskItem]
    let allTasks: [AutomationTaskItem]
    let kind: AutomationTaskItem.Kind?
    let isLoaded: Bool
    let runningId: String?
    let onOpen: (AutomationTaskItem) -> Void
    let onRunNow: (AutomationTaskItem) -> Void

    private var noun: String {
        switch kind {
        case nil: return "automation tasks"
        case .goal: return "goals"
        case .routine: return "routines"
        case .loop: return "loops"
        case .heartbeat: return "heartbeats"
        }
    }

    var body: some View {
        if tasks.isEmpty {
            if isLoaded {
                AutomationGraphEmptyState(
                    title: "No \(noun) yet",
                    message: "Click New task above to add one. It shows here with its schedule, the bot that runs it and the goal it serves."
                )
            } else {
                Text("Loading \(kind == nil ? "tasks" : noun)…")
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextTertiary"))
            }
        } else {
            VStack(spacing: 8) {
                ForEach(Self.flowOrder(tasks: tasks, allTasks: allTasks), id: \.key) { task in
                    Group {
                        if task.type == .goal {
                            goalCard(task)
                        } else {
                            triggerCard(task)
                        }
                    }
                    .contentShape(Rectangle())
                    .onTapGesture { onOpen(task) }
                    .accessibilityElement(children: .contain)
                    .accessibilityLabel("\(task.name): \(AutomationScheduleLogic.scheduleLine(task)). Show details")
                    .accessibilityAction { onOpen(task) }
                }
            }
        }
    }

    /// layoutAutomationGraph's order: goals with the tasks that serve them
    /// (grouped by who runs them), then the tasks that serve no goal.
    static func flowOrder(tasks: [AutomationTaskItem], allTasks: [AutomationTaskItem]) -> [AutomationTaskItem] {
        let goals = allTasks.filter { $0.type == .goal }
        // On a Goals tab, pull in each goal's tasks from the full list.
        let goalIds = Set(tasks.filter { $0.type == .goal }.map(\.id))
        let shown = Set(tasks.map(\.key))
        let pulled = allTasks.filter { t in
            guard t.type != .goal, let goalId = t.goalId else { return false }
            return goalIds.contains(goalId) && !shown.contains(t.key)
        }

        var order: [String] = []
        var goalOf: [String: AutomationTaskItem] = [:]
        var members: [String: [AutomationTaskItem]] = [:]
        func touch(_ key: String) {
            if !order.contains(key) { order.append(key) }
        }
        for t in tasks + pulled {
            if t.type == .goal {
                touch(t.id)
                goalOf[t.id] = t
                continue
            }
            let goal = t.goalId.flatMap { id in goals.first { $0.id == id } }
            let key = goal?.id ?? ""
            touch(key)
            if let goal { goalOf[key] = goal }
            members[key, default: []].append(t)
        }
        if let loose = order.firstIndex(of: "") { order.append(order.remove(at: loose)) }

        var out: [AutomationTaskItem] = []
        for key in order {
            if let goal = goalOf[key] { out.append(goal) }
            var runners: [String] = []
            var byRunner: [String: [AutomationTaskItem]] = [:]
            for t in members[key] ?? [] {
                let runner = t.agentId ?? "model:\(t.executorName)"
                if !runners.contains(runner) { runners.append(runner) }
                byRunner[runner, default: []].append(t)
            }
            for runner in runners { out.append(contentsOf: byRunner[runner] ?? []) }
        }
        return out
    }

    private func triggerCard(_ t: AutomationTaskItem) -> some View {
        let failed = t.status == "error"
        return GraphCard(border: failed ? Theme.statusError : Color("BorderSubtle")) {
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 4) {
                    Image(systemName: t.type.icon).font(.system(size: 10))
                    Text("\(t.type.label) · \(AutomationScheduleLogic.scheduleLine(t))").lineLimit(1)
                }
                .font(.system(size: 10.5))
                .foregroundColor(Color("TextTertiary"))
                Text(t.name)
                    .font(.system(size: 12.5, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                Text(AutomationScheduleLogic.lastRunLine(t.lastRun))
                    .font(.system(size: 11))
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(1)
                HStack {
                    Spacer(minLength: 0)
                    GraphRunNowButton(disabled: runningId == t.id) { onRunNow(t) }
                }
                .padding(.top, 4)
            }
        }
        .opacity(!t.isActive && !failed ? 0.6 : 1)
    }

    private func goalCard(_ g: AutomationTaskItem) -> some View {
        GraphCard {
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 4) {
                    Image(systemName: "target").font(.system(size: 10))
                    Text("Goal · \(AutomationScheduleLogic.scheduleLine(g))").lineLimit(1)
                }
                .font(.system(size: 10.5))
                .foregroundColor(Color("TextTertiary"))
                Text(g.name)
                    .font(.system(size: 12.5, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                AutomationProgressBar(progress: g.progress ?? 0)
                    .padding(.top, 4)
            }
        }
    }
}

/// A goal's progress: a 4 pt bar and the percent.
struct AutomationProgressBar: View {
    let progress: Double

    private var percent: Int { Int(max(0, min(100, progress.rounded()))) }

    var body: some View {
        HStack(spacing: 8) {
            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    Capsule().fill(Color("FillNeutral"))
                    Capsule().fill(Color("AccentPrimary")).frame(width: geo.size.width * CGFloat(percent) / 100)
                }
            }
            .frame(height: 4)
            Text("\(percent)%")
                .font(.system(size: 11).monospacedDigit())
                .foregroundColor(Color("TextTertiary"))
        }
    }
}

// MARK: - Routines

extension AutomationRoutine {
    /// routineResult (RoutinesGraph.tsx) from the row's recent runs.
    enum FlowResult { case ok, failed, none }

    /// The newest finished run.
    var lastFinishedRun: AutomationRunTick? {
        recentRuns?.first { $0.status != "running" && $0.status != "pending" }
    }

    var flowResult: FlowResult {
        guard let run = lastFinishedRun else { return .none }
        return Self.runSucceeded(run.status) ? .ok : .failed
    }

    /// The cloud cron daemon says `success`.
    static func runSucceeded(_ status: String) -> Bool {
        ["succeeded", "completed", "success", "skipped"].contains(status)
    }

    var consecutiveFailures: Int {
        (recentRuns ?? []).filter { $0.status != "running" && $0.status != "pending" }
            .prefix { !Self.runSucceeded($0.status) }.count
    }

    /// `scheduleLabel`: "Every day at 7:00 AM", "Every Monday at 9:00 AM".
    var scheduleLabel: String {
        let f = scheduleExpression.split(whereSeparator: \.isWhitespace).map(String.init)
        if scheduleType == "cron", f.count == 5, f[2] == "*", f[3] == "*",
           let m = Int(f[0]), let h = Int(f[1]), let dow = Int(f[4]), (0...7).contains(dow) {
            let name = Calendar.current.weekdaySymbols[dow % 7]
            return "Every \(name) at \(AutomationRoutineRule.fmtTime(h * 60 + m))"
        }
        return CronWords.describe(type: scheduleType, expression: scheduleExpression)
    }

    /// "Next Fri 7:00 AM" / "Paused" / "No next run".
    var triggerLine: String {
        guard isEnabled else { return "Paused" }
        if let next = libraryParseDate(nextRunAt) {
            return "Next \(next.formatted(.dateTime.weekday(.abbreviated).hour().minute()))"
        }
        return "No next run"
    }

    var resultLine: String {
        switch flowResult {
        case .none: return "No run yet"
        case .ok, .failed:
            let word = flowResult == .ok ? "Done" : "Failed"
            guard let at = libraryParseDate(lastFinishedRun?.startedAt ?? lastRunAt) else { return word }
            return "\(word) · \(at.formatted(.dateTime.weekday(.abbreviated).hour().minute()))"
        }
    }
}

struct AutomationRoutinesGraph: View {
    let routines: [AutomationRoutine]
    let isLoaded: Bool
    let selectedId: String?
    let onSelect: (AutomationRoutine) -> Void
    let onRunNow: (AutomationRoutine) -> Void
    let onNew: () -> Void

    var body: some View {
        if routines.isEmpty {
            if isLoaded {
                AutomationGraphEmptyState(
                    title: "No routines yet",
                    message: "A bot's routines run on their own and post in its main thread.",
                    actionTitle: "New routine",
                    action: onNew
                )
            } else {
                Text("Loading routines…")
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextTertiary"))
            }
        } else {
            VStack(spacing: 8) {
                ForEach(routines) { routine in
                    VStack(alignment: .leading, spacing: 4) {
                        triggerCard(routine)
                        connector(resultColor(routine.flowResult))
                        resultCard(routine)
                    }
                    .contentShape(Rectangle())
                    .onTapGesture { onSelect(routine) }
                    .accessibilityElement(children: .contain)
                    .accessibilityLabel("\(displayName(routine)): \(routine.resultLine). Show details")
                    .accessibilityAction { onSelect(routine) }
                }
            }
        }
    }

    private func displayName(_ r: AutomationRoutine) -> String {
        AutomationScheduleLogic.displayName(r.name)
    }

    private func resultColor(_ result: AutomationRoutine.FlowResult) -> Color {
        switch result {
        case .ok: return Theme.statusSuccess
        case .failed: return Theme.statusError
        case .none: return Color("BorderStrong")
        }
    }

    /// The line from the trigger to its result.
    private func connector(_ color: Color) -> some View {
        Path { path in
            path.move(to: CGPoint(x: 0.5, y: 0))
            path.addLine(to: CGPoint(x: 0.5, y: 12))
        }
        .stroke(color, lineWidth: 1)
        .frame(width: 1, height: 12)
        .padding(.leading, 20)
        .accessibilityHidden(true)
    }

    private func triggerCard(_ r: AutomationRoutine) -> some View {
        let manual = r.scheduleType == "manual"
        return GraphCard(border: r.id == selectedId ? Color("AccentPrimary") : Color("BorderSubtle")) {
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 4) {
                    Image(systemName: manual ? "bolt" : "clock").font(.system(size: 10))
                    Text(r.triggerLine).lineLimit(1)
                }
                .font(.system(size: 10.5))
                .foregroundColor(Color("TextTertiary"))
                Text(displayName(r))
                    .font(.system(size: 12.5, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                Text(r.scheduleLabel)
                    .font(.system(size: 11))
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(1)
                HStack {
                    Spacer(minLength: 0)
                    GraphRunNowButton(disabled: false) { onRunNow(r) }
                }
                .padding(.top, 4)
            }
        }
        .opacity(r.isEnabled ? 1 : 0.6)
    }

    private func resultCard(_ r: AutomationRoutine) -> some View {
        let result = r.flowResult
        let selected = r.id == selectedId
        let error = result == .failed ? r.lastFinishedRun?.reason : nil
        return GraphCard(border: selected ? Color("AccentPrimary") : (result == .failed ? Theme.statusError : Color("BorderSubtle"))) {
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 4) {
                    Image(systemName: result == .ok ? "checkmark.circle" : (result == .failed ? "xmark.circle" : "clock"))
                        .font(.system(size: 11))
                        .foregroundColor(resultColor(result))
                    Text(r.resultLine)
                        .lineLimit(1)
                        .foregroundColor(result == .none ? Color("TextTertiary") : resultColor(result))
                }
                .font(.system(size: 11.5))
                Text(displayName(r))
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                if let error, !error.isEmpty {
                    Text(error)
                        .font(.system(size: 11))
                        .foregroundColor(Theme.statusError)
                        .lineLimit(2)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 4)
                        .background(RoundedRectangle(cornerRadius: 6, style: .continuous).fill(Theme.statusError.opacity(0.1)))
                        .padding(.top, 2)
                } else if r.consecutiveFailures > 1 {
                    Text("\(r.consecutiveFailures) failures in a row")
                        .font(.system(size: 11))
                        .foregroundColor(Theme.statusError)
                        .padding(.top, 2)
                }
            }
        }
    }
}
