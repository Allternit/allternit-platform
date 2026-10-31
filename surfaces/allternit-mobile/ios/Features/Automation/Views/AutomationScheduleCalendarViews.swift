import SwiftUI

// Calendar view of Automation Tasks: the scheduler, the "Coming up" list and
// the tasks it can't place (AutomationSchedule.tsx AutomationCalendar and
// BotRoutinesSchedule.tsx), plus the details sheets a tap opens
// (TaskDetailsCard, RoutineDetailsPanel).

/// What the calendar and details can ask the screen to do.
struct AutomationTaskActions {
    let runNow: (AutomationTaskItem) -> Void
    let setActive: (AutomationTaskItem, Bool) -> Void
    let delete: (AutomationTaskItem) -> Void
    let new: (_ cron: String, _ day: Date) -> Void
    let move: (AutomationTaskItem, _ cron: String) -> Void
}

/// A bot's avatar, or the task type's icon in a soft circle.
struct AutomationTaskMark: View {
    let agentId: String?
    let kind: AutomationTaskItem.Kind
    let size: CGFloat
    @ObservedObject private var bots = BotModeStore.shared

    var body: some View {
        if let agentId, !agentId.isEmpty, let bot = bots.bot(agentId) {
            BotAvatarView(bot: bot, size: size)
        } else {
            Image(systemName: kind.icon)
                .font(.system(size: (size * 0.6).rounded()))
                .foregroundColor(Color("TextSecondary"))
                .frame(width: size, height: size)
                .background(Circle().fill(Color("FillNeutral")))
        }
    }
}

/// flex-wrap for the "Not on the calendar" chips.
private struct SchedulerWrapLayout: Layout {
    var spacing: CGFloat = 6

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let width = proposal.width ?? .infinity
        var x: CGFloat = 0, y: CGFloat = 0, rowHeight: CGFloat = 0, widest: CGFloat = 0
        for view in subviews {
            let size = view.sizeThatFits(ProposedViewSize(width: width, height: nil))
            if x > 0, x + size.width > width {
                x = 0
                y += rowHeight + spacing
                rowHeight = 0
            }
            x += size.width + spacing
            widest = max(widest, x - spacing)
            rowHeight = max(rowHeight, size.height)
        }
        return CGSize(width: min(widest, width), height: y + rowHeight)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var x = bounds.minX, y = bounds.minY, rowHeight: CGFloat = 0
        for view in subviews {
            let size = view.sizeThatFits(ProposedViewSize(width: bounds.width, height: nil))
            if x > bounds.minX, x + size.width > bounds.maxX {
                x = bounds.minX
                y += rowHeight + spacing
                rowHeight = 0
            }
            view.place(at: CGPoint(x: x, y: y), proposal: ProposedViewSize(size))
            x += size.width + spacing
            rowHeight = max(rowHeight, size.height)
        }
    }
}

/// `h-[560px] max-h-[70dvh] min-h-[380px]`.
private extension View {
    func schedulerCalendarHeight() -> some View {
        containerRelativeFrame(.vertical) { length, _ in max(380, min(560, length * 0.7)) }
    }
}

/// The "Coming up" / "Upcoming runs" card.
private struct UpcomingCard<Content: View>: View {
    let title: String
    let isEmpty: Bool
    @ViewBuilder var content: () -> Content

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(title)
                .font(.system(size: 12, weight: .medium))
                .foregroundColor(Color("TextTertiary"))
                .padding(.horizontal, 6)
                .padding(.bottom, 4)
            if isEmpty {
                Text("Nothing in the next two weeks.")
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextTertiary"))
                    .padding(.horizontal, 6)
                    .padding(.bottom, 4)
            } else {
                content()
            }
        }
        .padding(8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
    }
}

// MARK: - Goals, loops, heartbeats, All

struct AutomationTasksCalendar: View {
    let tasks: [AutomationTaskItem]
    let kind: AutomationTaskItem.Kind?
    let runningId: String?
    let actions: AutomationTaskActions

    private struct Pick: Identifiable {
        let task: AutomationTaskItem
        let at: Date?
        var id: String { "\(task.key)@\(at?.timeIntervalSince1970 ?? 0)" }
    }

    private struct Occurrence: Identifiable {
        let task: AutomationTaskItem
        let at: Date
        var id: String { "\(task.key)@\(at.timeIntervalSince1970)" }
    }

    @State private var pick: Pick? = nil

    private var placement: (items: [SchedulerItem], unplaced: [AutomationTaskItem]) {
        var items: [SchedulerItem] = []
        var unplaced: [AutomationTaskItem] = []
        for t in tasks {
            let key = "\(t.type.rawValue)|\(t.agentId ?? "")"
            if t.type == .goal {
                if let on = AutomationScheduleLogic.goalDay(t) {
                    items.append(SchedulerItem(
                        id: t.key, botId: key, title: t.name,
                        rule: AutomationRoutineRule(repeatKind: .daily, at: 0, days: AutomationRoutineRule.allDays),
                        enabled: t.status != "completed" && t.status != "cancelled", on: on, locked: true
                    ))
                } else {
                    unplaced.append(t)
                }
                continue
            }
            if let rule = AutomationScheduleLogic.taskRule(t.schedule) {
                items.append(SchedulerItem(id: t.key, botId: key, title: t.name, rule: rule, enabled: t.isActive))
            } else {
                unplaced.append(t)
            }
        }
        return (items, unplaced)
    }

    private var upcoming: [Occurrence] {
        let now = Date()
        let end = now.addingTimeInterval(14 * 86_400)
        let runs = tasks.flatMap { t in
            AutomationScheduleLogic.occurrences(t, from: now, to: end, limit: 12).map { Occurrence(task: t, at: $0) }
        }
        let dues: [Occurrence] = tasks.compactMap { t in
            guard let day = AutomationScheduleLogic.goalDay(t), day.addingTimeInterval(86_400) > now, t.status != "completed" else { return nil }
            return Occurrence(task: t, at: day)
        }
        return Array((runs + dues).sorted { $0.at < $1.at }.prefix(6))
    }

    private var hint: String {
        if kind == .goal {
            return "Goals sit on their target date · click a time to add a goal due that day · click one to open it"
        }
        let noun = kind?.label.lowercased() ?? "task"
        return "Click an empty time to add a \(noun) · click one to open it · drag it to move it"
    }

    var body: some View {
        let placed = placement
        let coming = upcoming
        VStack(alignment: .leading, spacing: 12) {
            AutomationScheduler(
                items: placed.items,
                avatar: { key, size in
                    let parts = key.split(separator: "|", omittingEmptySubsequences: false).map(String.init)
                    return AnyView(AutomationTaskMark(
                        agentId: parts.count > 1 ? parts[1] : nil,
                        kind: AutomationTaskItem.Kind(rawValue: parts.first ?? "") ?? .routine,
                        size: size
                    ))
                },
                colorOf: { item in
                    AutomationTaskItem.Kind(rawValue: String(item.botId.prefix { $0 != "|" }))?.scheduleColor ?? SchedulerTint.fallback
                },
                hint: hint,
                label: "\(kind?.label ?? "Automation") calendar",
                onNew: { day, minute in
                    actions.new(AutomationRoutineRule.forSlot(day: day, at: minute).cron, day)
                },
                onMove: { id, minute in
                    guard let t = tasks.first(where: { $0.key == id }),
                          let item = placed.items.first(where: { $0.id == id }), !item.locked else { return }
                    actions.move(t, item.rule.moved(to: minute).cron)
                },
                details: { id, day, close in
                    guard let t = tasks.first(where: { $0.key == id }) else { return AnyView(EmptyView()) }
                    let item = placed.items.first { $0.id == id }
                    // The run this chip stands for: its time on the day it was picked (bands and goals: the day).
                    var at: Date? = nil
                    if let item, item.on == nil, item.rule.repeatKind != .interval {
                        at = Calendar.current.date(byAdding: .minute, value: item.rule.at, to: Calendar.current.startOfDay(for: day))
                    }
                    return AnyView(details(t, at: at ?? AutomationScheduleLogic.goalDay(t), close: close))
                }
            )
            .schedulerCalendarHeight()

            UpcomingCard(title: "Coming up", isEmpty: coming.isEmpty) {
                ForEach(coming) { occurrence in
                    upcomingRow(occurrence)
                }
            }

            if !placed.unplaced.isEmpty {
                SchedulerWrapLayout {
                    Text("Not on the calendar:")
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextTertiary"))
                    ForEach(placed.unplaced, id: \.key) { t in
                        unplacedChip(t)
                    }
                }
            }
        }
        .sheet(item: $pick) { pick in
            details(pick.task, at: pick.at, close: { self.pick = nil })
        }
    }

    private func details(_ t: AutomationTaskItem, at: Date?, close: @escaping () -> Void) -> some View {
        // The sheet is live: a pause or run shows on the row the screen reloaded.
        let live = tasks.first { $0.key == t.key } ?? t
        return AutomationTaskDetailsSheet(
            task: live, at: at, isRunning: runningId == t.id, actions: actions, close: close
        )
    }

    private func upcomingRow(_ o: Occurrence) -> some View {
        let isGoal = o.task.type == .goal
        let dayText = "Due \(AutomationScheduleLogic.shortDay(o.at))"
        let whenText = isGoal ? dayText : AutomationScheduleLogic.when(o.at)
        return Button { pick = Pick(task: o.task, at: o.at) } label: {
            HStack(spacing: 8) {
                RoundedRectangle(cornerRadius: 2)
                    .fill(o.task.type.scheduleColor)
                    .frame(width: 4, height: 28)
                VStack(alignment: .leading, spacing: 1) {
                    Text(o.task.name)
                        .font(.system(size: 12.5))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Text("\(whenText) · \(o.task.type.label)")
                        .font(.system(size: 11))
                        .foregroundColor(Color("TextTertiary"))
                        .lineLimit(1)
                }
                Spacer(minLength: 0)
                AutomationTaskMark(agentId: o.task.agentId, kind: o.task.type, size: 20)
            }
            .padding(.horizontal, 6)
            .frame(minHeight: 44)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(o.task.name), \(whenText). Show details")
    }

    private func unplacedChip(_ t: AutomationTaskItem) -> some View {
        let color = t.type.scheduleColor
        let schedule = t.schedule?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let detail = t.type == .goal ? "no target date" : (schedule.isEmpty ? "manual" : schedule)
        return Button { pick = Pick(task: t, at: nil) } label: {
            Text("\(t.name) · \(detail)")
                .font(.system(size: 12))
                .foregroundColor(SchedulerTint.text(color))
                .padding(.horizontal, 8)
                .padding(.vertical, 2)
                .schedulerTint(color, percent: 12, radius: 6)
                .contentShape(Rectangle().inset(by: -12))
        }
        .buttonStyle(.plain)
    }
}

// MARK: - TaskDetailsCard

/// A task's details, as a calendar sheet (TaskDetailsCard): what the task row
/// really has and the actions the view offers. Open shows the full details in
/// the same sheet.
struct AutomationTaskDetailsSheet: View {
    let task: AutomationTaskItem
    /// The run (or due day) the card was opened from.
    let at: Date?
    let isRunning: Bool
    let actions: AutomationTaskActions
    let close: () -> Void

    @State private var isOpen = false
    @ObservedObject private var bots = BotModeStore.shared

    var body: some View {
        if isOpen {
            AutomationTaskDetailSheet(
                task: task,
                isRunning: isRunning,
                onToggle: { actions.setActive(task, $0) },
                onRunNow: { actions.runNow(task) },
                onDelete: {
                    close()
                    actions.delete(task)
                }
            )
        } else {
            ScrollView { card.padding(12) }
                .presentationDetents([.medium, .large])
                .presentationDragIndicator(.visible)
                .presentationBackground(Color("BgPanel"))
        }
    }

    private var card: some View {
        let failed = task.status == "error"
        let time = at.flatMap { date -> String? in
            guard task.type != .goal else { return nil }
            let c = Calendar.current.dateComponents([.hour, .minute], from: date)
            return AutomationRoutineRule.fmtTime((c.hour ?? 0) * 60 + (c.minute ?? 0))
        }
        let day = at.map(AutomationScheduleLogic.shortDay)
        let words = AutomationScheduleLogic.scheduleWords(task)
        return VStack(alignment: .leading, spacing: 6) {
            VStack(alignment: .leading, spacing: 4) {
                Text(task.name)
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                HStack(spacing: 6) {
                    Label(task.type.label, systemImage: task.type.icon)
                        .font(.system(size: 11, weight: .medium))
                        .foregroundColor(SchedulerTint.text(task.type.scheduleColor))
                        .padding(.horizontal, 6)
                        .padding(.vertical, 1)
                        .schedulerTint(task.type.scheduleColor, percent: 16, radius: 6)
                    if let day { Text(task.type == .goal ? "Due \(day)" : day) }
                    if !task.isActive, task.type != .goal { Text("· \(failed ? "Stopped" : "Paused")") }
                }
                .font(.system(size: 12))
                .foregroundColor(Color("TextTertiary"))
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
            .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("FillNeutral")))

            if task.type == .goal {
                row(icon: "target") { AutomationProgressBar(progress: task.progress ?? 0) }
            } else {
                row(icon: "clock") {
                    if let time {
                        Text(time).font(.system(size: 12.5, weight: .medium)).foregroundColor(Color("TextPrimary"))
                    }
                    if let words {
                        Text(words)
                            .font(.system(size: 12.5, weight: time == nil ? .medium : .regular))
                            .foregroundColor(time == nil ? Color("TextPrimary") : Color("TextSecondary"))
                    }
                    Text(AutomationScheduleLogic.scheduleLine(task))
                        .font(.system(size: 11.5))
                        .foregroundColor(Color("TextTertiary"))
                }
                row(icon: "cpu") {
                    HStack(spacing: 6) {
                        Text("Runs as").foregroundColor(Color("TextTertiary"))
                        if let agentId = task.agentId, let bot = bots.bot(agentId) {
                            BotAvatarView(bot: bot, size: 16)
                        }
                        BotNameText(task.executorName, size: 12.5)
                            .fontWeight(.medium)
                            .lineLimit(1)
                    }
                    .font(.system(size: 12.5))
                }
                row(icon: failed ? "exclamationmark.circle" : "clock.arrow.circlepath", danger: failed) {
                    if failed {
                        Text("Last run failed").font(.system(size: 12.5, weight: .medium)).foregroundColor(Theme.statusError)
                    }
                    Text(AutomationScheduleLogic.lastRunLine(task.lastRun))
                        .font(.system(size: 12.5))
                        .foregroundColor(failed ? Color("TextSecondary") : Color("TextPrimary"))
                }
            }

            HStack(spacing: 6) {
                action("Open", icon: "arrow.up.right.square", primary: true) { isOpen = true }
                if task.type != .goal {
                    action(isRunning ? "Running…" : "Run now", icon: "play.fill", busy: isRunning) { actions.runNow(task) }
                }
                action(task.isActive ? "Pause" : "Resume", icon: task.isActive ? "pause" : "play") {
                    close()
                    actions.setActive(task, !task.isActive)
                }
            }
            .padding(.top, 2)
        }
    }

    private func row<Content: View>(icon: String, danger: Bool = false, @ViewBuilder content: () -> Content) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: icon)
                .font(.system(size: 13))
                .foregroundColor(danger ? Theme.statusError : Color("TextTertiary"))
            VStack(alignment: .leading, spacing: 1) { content() }
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("FillNeutral")))
    }

    private func action(_ title: String, icon: String, primary: Bool = false, busy: Bool = false, perform: @escaping () -> Void) -> some View {
        Button(action: perform) {
            HStack(spacing: 4) {
                Image(systemName: icon).font(.system(size: 11))
                Text(title).font(.system(size: 12, weight: .medium))
            }
            .foregroundColor(primary ? Color("BgPanel") : Color("TextPrimary"))
            .padding(.horizontal, 10)
            .frame(height: 32)
            .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(primary ? Color("TextPrimary") : Color.clear))
            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(primary ? Color.clear : Color("BorderStrong"), lineWidth: 1))
            .contentShape(Rectangle().inset(by: -6))
        }
        .buttonStyle(.plain)
        .disabled(busy)
        .opacity(busy ? 0.6 : 1)
    }
}

// MARK: - Routines

/// The Routines tab's calendar (BotRoutinesSchedule): chips in each bot's colour.
struct AutomationRoutinesCalendar: View {
    let routines: [AutomationRoutine]
    @ObservedObject var store: AutomationStore
    let onNew: (_ cron: String, _ day: Date) -> Void
    @ObservedObject private var bots = BotModeStore.shared
    @ObservedObject private var colors = BotColorStore.shared

    private struct Occurrence: Identifiable {
        let routine: AutomationRoutine
        let at: Date
        var id: String { "\(routine.id)@\(at.timeIntervalSince1970)" }
    }

    private struct Selection: Identifiable { let id: String }

    @State private var selection: Selection? = nil

    private var placement: (items: [SchedulerItem], unplaced: [AutomationRoutine]) {
        var items: [SchedulerItem] = []
        var unplaced: [AutomationRoutine] = []
        for r in routines {
            if let rule = AutomationRoutineRule.fromSchedule(type: r.scheduleType, expression: r.scheduleExpression) {
                items.append(SchedulerItem(id: r.id, botId: r.agentId ?? "", title: AutomationScheduleLogic.displayName(r.name), rule: rule, enabled: r.isEnabled))
            } else {
                unplaced.append(r)
            }
        }
        return (items, unplaced)
    }

    private var upcoming: [Occurrence] {
        let now = Date()
        let end = now.addingTimeInterval(14 * 86_400)
        var all: [Occurrence] = []
        for r in routines {
            for at in RoutineOccurrences.routineOccurrences(r, from: now, to: end) {
                all.append(Occurrence(routine: r, at: at))
            }
        }
        all.sort { (l: Occurrence, r: Occurrence) -> Bool in
            l.at != r.at ? l.at < r.at : l.routine.name < r.routine.name
        }
        return Array(all.prefix(6))
    }

    private func color(for botId: String) -> Color {
        guard let hex = colors.hex(for: bots.bot(botId)) else { return SchedulerTint.fallback }
        return Color(hex: hex)
    }

    var body: some View {
        let placed = placement
        let coming = upcoming
        VStack(alignment: .leading, spacing: 12) {
            AutomationScheduler(
                items: placed.items,
                avatar: { botId, size in
                    AnyView(AutomationTaskMark(agentId: botId, kind: .routine, size: size))
                },
                colorOf: { color(for: $0.botId) },
                hint: "Click an empty time to add a routine · click one to open it · drag it to move it",
                label: "Routine calendar",
                onNew: { day, minute in
                    onNew(AutomationRoutineRule.forSlot(day: day, at: minute).cron, day)
                },
                onMove: { id, minute in
                    guard let r = routines.first(where: { $0.id == id }),
                          let item = placed.items.first(where: { $0.id == id }) else { return }
                    Task { await store.moveRoutine(r, cron: item.rule.moved(to: minute).cron) }
                },
                details: { id, _, close in
                    AnyView(AutomationRoutineDetailsSheet(routineId: id, store: store, onClose: close))
                }
            )
            .schedulerCalendarHeight()

            UpcomingCard(title: "Upcoming runs", isEmpty: coming.isEmpty) {
                ForEach(coming) { occurrence in
                    upcomingRow(occurrence)
                }
            }

            if !placed.unplaced.isEmpty {
                SchedulerWrapLayout {
                    Text("Not on the calendar:")
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextTertiary"))
                    ForEach(placed.unplaced) { r in
                        Button { selection = Selection(id: r.id) } label: {
                            Text("\(AutomationScheduleLogic.displayName(r.name)) · \(r.scheduleLabel)")
                                .font(.system(size: 12))
                                .foregroundColor(Color("TextSecondary"))
                                .padding(.horizontal, 8)
                                .padding(.vertical, 2)
                                .background(RoundedRectangle(cornerRadius: 6, style: .continuous).fill(Color("FillNeutral")))
                                .contentShape(Rectangle().inset(by: -12))
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
        }
        .sheet(item: $selection) { picked in
            AutomationRoutineDetailsSheet(routineId: picked.id, store: store, onClose: { selection = nil })
        }
    }

    private func upcomingRow(_ o: Occurrence) -> some View {
        Button { selection = Selection(id: o.routine.id) } label: {
            HStack(spacing: 8) {
                VStack(alignment: .leading, spacing: 1) {
                    Text(AutomationScheduleLogic.displayName(o.routine.name))
                        .font(.system(size: 12.5))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Text("\(AutomationScheduleLogic.when(o.at)) · \(o.routine.scheduleLabel)")
                        .font(.system(size: 11))
                        .foregroundColor(Color("TextTertiary"))
                        .lineLimit(1)
                }
                Spacer(minLength: 0)
                AutomationTaskMark(agentId: o.routine.agentId, kind: .routine, size: 20)
            }
            .padding(.horizontal, 6)
            .frame(minHeight: 44)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

/// RoutineDetailsPanel as a sheet: status, schedule, what it does, the bot, the
/// next 7 days and run history. Run now and Pause/Resume act on the server.
struct AutomationRoutineDetailsSheet: View {
    let routineId: String
    @ObservedObject var store: AutomationStore
    let onClose: () -> Void
    @ObservedObject private var bots = BotModeStore.shared
    @State private var busy = false

    private var routine: AutomationRoutine? { store.routines.first { $0.id == routineId } }

    var body: some View {
        Group {
            if let routine {
                ScrollView { panel(routine).padding(16) }
            } else {
                Color.clear
            }
        }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
        .presentationBackground(Color("BgPanel"))
    }

    private func panel(_ r: AutomationRoutine) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .top, spacing: 8) {
                VStack(alignment: .leading, spacing: 4) {
                    Text(AutomationScheduleLogic.displayName(r.name))
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                    HStack(spacing: 6) {
                        statusChip(r)
                        Text(r.scheduleLabel)
                            .font(.system(size: 12))
                            .foregroundColor(Color("TextSecondary"))
                    }
                }
                Spacer(minLength: 0)
                Button(action: onClose) {
                    Image(systemName: "xmark")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextTertiary"))
                        .frame(width: 44, height: 44)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Close details")
            }

            HStack(spacing: 6) {
                Button { act { await store.runNow(r) } } label: {
                    Label(busy ? "Working…" : "Run now", systemImage: "play.fill")
                        .font(.system(size: 12, weight: .medium))
                        .foregroundColor(Color("BgPanel"))
                        .padding(.horizontal, 10)
                        .frame(height: 32)
                        .background(RoundedRectangle(cornerRadius: 6, style: .continuous).fill(Color("TextPrimary")))
                        .contentShape(Rectangle().inset(by: -6))
                }
                .buttonStyle(.plain)
                Button { act { await store.setEnabled(r, !r.isEnabled) } } label: {
                    Label(r.isEnabled ? "Pause" : "Resume", systemImage: r.isEnabled ? "pause" : "play")
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextPrimary"))
                        .padding(.horizontal, 10)
                        .frame(height: 32)
                        .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
                        .contentShape(Rectangle().inset(by: -6))
                }
                .buttonStyle(.plain)
            }
            .disabled(busy)
            .opacity(busy ? 0.5 : 1)

            if let error = store.actionError {
                Text(error).font(.system(size: 12)).foregroundColor(Theme.statusError)
            }

            if !r.instruction.isEmpty {
                section("What it does") {
                    Text(r.instruction)
                        .font(.system(size: 12.5))
                        .foregroundColor(Color("TextPrimary"))
                        .fixedSize(horizontal: false, vertical: true)
                }
            }

            if let agentId = r.agentId, let bot = bots.bot(agentId) {
                section("Runs as") {
                    HStack(spacing: 8) {
                        BotAvatarView(bot: bot, size: 20)
                        BotNameText(bot.botDisplayName, size: 12.5)
                    }
                }
            }

            section("Next 7 days") { nextSevenDays(r) }
            section("History") { history(r) }
        }
    }

    private func act(_ work: @escaping @MainActor () async -> Void) {
        busy = true
        Task {
            await work()
            busy = false
        }
    }

    private func section<Content: View>(_ title: String, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title)
                .font(.system(size: 12, weight: .medium))
                .foregroundColor(Color("TextTertiary"))
            content()
        }
    }

    private func statusChip(_ r: AutomationRoutine) -> some View {
        Text(r.isEnabled ? "Active" : "Paused")
            .font(.system(size: 11, weight: .medium))
            .foregroundColor(r.isEnabled ? Theme.statusSuccess : Color("TextTertiary"))
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(
                RoundedRectangle(cornerRadius: 6, style: .continuous)
                    .fill(r.isEnabled ? Theme.statusSuccess.opacity(0.1) : Color("FillNeutral"))
            )
    }

    /// One cell per day, the first run time in it.
    private func nextSevenDays(_ r: AutomationRoutine) -> some View {
        let calendar = Calendar.current
        let day0 = calendar.startOfDay(for: Date())
        let end = calendar.date(byAdding: .day, value: 7, to: day0) ?? day0
        let times = RoutineOccurrences.routineOccurrences(r, from: day0, to: end)
        let nextLine: String
        if r.isEnabled, let next = libraryParseDate(r.nextRunAt) {
            nextLine = "Next: \(next.formatted(.dateTime.weekday(.abbreviated).month(.abbreviated).day().hour().minute()))"
        } else {
            nextLine = r.isEnabled ? "No next run scheduled." : "Paused: no runs until you resume it."
        }
        return VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 4) {
                ForEach(0..<7, id: \.self) { i in
                    let day = calendar.date(byAdding: .day, value: i, to: day0) ?? day0
                    let mine = times.filter { calendar.isDate($0, inSameDayAs: day) }
                    VStack(spacing: 2) {
                        Text(cellText(mine))
                            .font(.system(size: 10).monospacedDigit())
                            .lineLimit(1)
                            .minimumScaleFactor(0.6)
                            .foregroundColor(mine.isEmpty ? Color.clear : Color("TextPrimary"))
                            .frame(maxWidth: .infinity)
                            .frame(height: 28)
                            .background(
                                RoundedRectangle(cornerRadius: 6, style: .continuous)
                                    .fill(mine.isEmpty ? Color.clear : Color("AccentPrimary").opacity(0.14))
                            )
                            .overlay(
                                RoundedRectangle(cornerRadius: 6, style: .continuous)
                                    .stroke(mine.isEmpty ? Color("BorderSubtle") : Color("AccentPrimary"), lineWidth: 1)
                            )
                        Text(day.formatted(.dateTime.weekday(.abbreviated)))
                            .font(.system(size: 10.5, weight: i == 0 ? .semibold : .regular))
                            .foregroundColor(i == 0 ? Color("AccentPrimary") : Color("TextTertiary"))
                    }
                }
            }
            Text(nextLine)
                .font(.system(size: 12))
                .foregroundColor(Color("TextSecondary"))
        }
    }

    private func cellText(_ runs: [Date]) -> String {
        guard let first = runs.first else { return "·" }
        let hour = first.formatted(.dateTime.hour()).filter { !$0.isWhitespace }
        return runs.count > 1 ? "\(hour)+\(runs.count - 1)" : hour
    }

    @ViewBuilder
    private func history(_ r: AutomationRoutine) -> some View {
        let runs = Array((r.recentRuns ?? []).prefix(10))
        if runs.isEmpty {
            Text("No runs yet.")
                .font(.system(size: 12))
                .foregroundColor(Color("TextTertiary"))
        } else {
            VStack(alignment: .leading, spacing: 6) {
                ForEach(runs) { run in
                    historyRow(run)
                }
            }
        }
    }

    private func historyRow(_ run: AutomationRunTick) -> some View {
        let ok = AutomationRoutine.runSucceeded(run.status)
        let started = libraryParseDate(run.startedAt)
        let finished = libraryParseDate(run.finishedAt)
        var duration = run.durationMs
        if duration == nil, let started, let finished {
            duration = finished.timeIntervalSince(started) * 1000
        }
        let startedText = started?.formatted(.dateTime.month(.abbreviated).day().hour().minute()) ?? "—"
        let skipped = run.status == "skipped" ? " · no change" : ""
        let shownDuration = Self.durationText(duration)
        return HStack(alignment: .top, spacing: 6) {
            Image(systemName: ok ? "checkmark.circle" : "xmark.circle")
                .font(.system(size: 13))
                .foregroundColor(ok ? Theme.statusSuccess : Theme.statusError)
                .accessibilityLabel(ok ? "Succeeded" : run.status)
            VStack(alignment: .leading, spacing: 1) {
                Text(verbatim: startedText + skipped)
                    .foregroundColor(Color("TextSecondary"))
                if !ok, let reason = run.reason, !reason.isEmpty {
                    Text(reason).foregroundColor(Theme.statusError)
                }
            }
            Spacer(minLength: 0)
            if let text = shownDuration {
                Text(text)
                    .font(.system(size: 12).monospacedDigit())
                    .foregroundColor(Color("TextTertiary"))
            }
        }
        .font(.system(size: 12))
    }

    /// formatDuration: "850 ms", "48 s", "3 min".
    private static func durationText(_ ms: Double?) -> String? {
        guard let ms, ms.isFinite, ms >= 0 else { return nil }
        if ms < 1000 { return "\(Int(ms.rounded())) ms" }
        let s = Int((ms / 1000).rounded())
        return s < 120 ? "\(s) s" : "\(Int((Double(s) / 60).rounded())) min"
    }
}
