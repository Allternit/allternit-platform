import SwiftUI

/// Cron in plain words, and the next run times, for the routine rows and the
/// calendar. Handles the five-field crons the app and the web write
/// (numbers, `*`, lists, ranges, `*/n`).
enum CronWords {
    enum Frequency: String, CaseIterable, Identifiable {
        case hourly = "Hourly", daily = "Daily", weekdays = "Weekdays", weekly = "Weekly"
        var id: String { rawValue }
    }

    /// The cron the sheet saves (the PWA's routine-rule ruleToCron).
    static func cron(_ frequency: Frequency, hour: Int, minute: Int, weekday: Int) -> String {
        switch frequency {
        case .hourly: return "\(minute) * * * *"
        case .daily: return "\(minute) \(hour) * * *"
        case .weekdays: return "\(minute) \(hour) * * 1-5"
        case .weekly: return "\(minute) \(hour) * * \(weekday)"
        }
    }

    static func describe(type: String, expression: String) -> String {
        switch type {
        case "manual": return "Runs when you start it"
        case "once": return "Runs once"
        case "interval": return "Every \(expression)"
        default: break
        }
        let f = expression.split(separator: " ").map(String.init)
        guard f.count == 5 else { return expression }
        let time: String? = {
            guard let h = Int(f[1]), let m = Int(f[0]) else { return nil }
            var parts = DateComponents()
            parts.hour = h
            parts.minute = m
            guard let date = Calendar.current.date(from: parts) else { return nil }
            return date.formatted(.dateTime.hour().minute())
        }()
        if f[1] == "*", let m = Int(f[0]), f[2] == "*", f[4] == "*" {
            return m == 0 ? "Every hour" : "Every hour at :\(String(format: "%02d", m))"
        }
        guard let time else { return expression }
        if f[2] == "*", f[3] == "*" {
            switch f[4] {
            case "*": return "Every day at \(time)"
            case "1-5": return "Weekdays at \(time)"
            default:
                let names = f[4].split(separator: ",").compactMap { Int($0) }.map { weekdayName($0) }
                return names.isEmpty ? expression : "\(names.joined(separator: ", ")) at \(time)"
            }
        }
        return expression
    }

    /// Next run times within `days` (cron routines; else just `nextRunAt`).
    static func upcoming(_ routine: AutomationRoutine, days: Int) -> [Date] {
        let fields = routine.scheduleExpression.split(separator: " ").map(String.init)
        guard routine.scheduleType == "cron", fields.count == 5,
              let minutes = values(fields[0], 0...59), let hours = values(fields[1], 0...23),
              let doms = values(fields[2], 1...31), let months = values(fields[3], 1...12),
              let dows = values(fields[4], 0...7) else {
            return routine.nextRunAt.flatMap(DevicePairingView.parseDate).map { [$0] } ?? []
        }
        let calendar = Calendar.current
        let now = Date()
        let start = calendar.startOfDay(for: now)
        var out: [Date] = []
        for offset in 0..<days {
            guard let day = calendar.date(byAdding: .day, value: offset, to: start) else { continue }
            let c = calendar.dateComponents([.day, .month, .weekday], from: day)
            let dow = (c.weekday ?? 1) - 1
            guard doms.contains(c.day ?? 0), months.contains(c.month ?? 0),
                  dows.contains(dow) || (dow == 0 && dows.contains(7)) else { continue }
            for h in hours.sorted() {
                for m in minutes.sorted() {
                    if let at = calendar.date(bySettingHour: h, minute: m, second: 0, of: day), at > now {
                        out.append(at)
                        if out.count > 200 { return out }
                    }
                }
            }
        }
        return out
    }

    private static func values(_ field: String, _ range: ClosedRange<Int>) -> Set<Int>? {
        var result = Set<Int>()
        for part in field.split(separator: ",") {
            let pieces = part.split(separator: "/")
            let step = pieces.count == 2 ? Int(pieces[1]) ?? 1 : 1
            let base = String(pieces[0])
            var lo = range.lowerBound, hi = range.upperBound
            if base != "*" {
                let bounds = base.split(separator: "-").compactMap { Int($0) }
                guard let first = bounds.first else { return nil }
                lo = first
                hi = bounds.count == 2 ? bounds[1] : (pieces.count == 2 ? range.upperBound : first)
            }
            guard lo <= hi, step > 0 else { return nil }
            result.formUnion(stride(from: lo, through: hi, by: step))
        }
        return result
    }

    private static func weekdayName(_ day: Int) -> String {
        let symbols = Calendar.current.shortWeekdaySymbols
        return symbols[((day % 7) + 7) % 7]
    }
}

/// New scheduled task (the PWA's ScheduledTaskModal): what to do, which bot
/// does it, and when. Saves a local cron routine whose agent runs
/// `config.prompt` as a turn in the bot's chat.
struct ScheduledTaskSheet: View {
    @ObservedObject var store: AutomationStore
    /// What it saves as: a routine (default), a loop or an agent heartbeat.
    var kind: AutomationTaskItem.Kind = .routine
    /// The cron of the calendar slot it was opened from ("30 9 * * 1-5").
    var slotCron: String? = nil
    @ObservedObject private var bots = AgentHubStore.shared
    @Environment(\.dismiss) private var dismiss

    @State private var name = ""
    @State private var instruction = ""
    @State private var botId: String? = nil
    @State private var frequency: CronWords.Frequency = .daily
    @State private var time = Calendar.current.date(bySettingHour: 9, minute: 0, second: 0, of: Date()) ?? Date()
    @State private var weekday = 1
    @State private var isSaving = false
    @State private var error: String? = nil

    private var cron: String {
        let c = Calendar.current.dateComponents([.hour, .minute], from: time)
        return CronWords.cron(frequency, hour: c.hour ?? 9, minute: c.minute ?? 0, weekday: weekday)
    }

    private var canSave: Bool {
        !name.trimmingCharacters(in: .whitespaces).isEmpty
            && !instruction.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && (botId != nil || kind != .routine) && !isSaving
    }

    private var title: String {
        switch kind {
        case .loop: return "New loop"
        case .heartbeat: return "New heartbeat"
        case .goal, .routine: return "Scheduled task"
        }
    }

    /// Starts on the slot the calendar was tapped at (ruleForSlot's cron).
    private func applySlot() {
        let f = (slotCron ?? "").split(separator: " ").map(String.init)
        guard f.count == 5, let minute = Int(f[0]), let hour = Int(f[1]) else { return }
        time = Calendar.current.date(bySettingHour: hour, minute: minute, second: 0, of: Date()) ?? time
        if f[4] == "1-5" {
            frequency = .weekdays
        } else if let day = Int(f[4]) {
            frequency = .weekly
            weekday = day
        }
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("Name", text: $name)
                    TextField("What should the bot do?", text: $instruction, axis: .vertical)
                        .lineLimit(3...8)
                }
                if kind != .heartbeat {
                Section {
                    if bots.isLoading && bots.agents.isEmpty {
                        ProgressView()
                    } else if bots.agents.isEmpty {
                        Text("Create a bot first. A routine runs as a turn in a bot's chat.")
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                    } else {
                        Picker("Bot", selection: $botId) {
                            Text("Choose").tag(String?.none)
                            ForEach(bots.agents) { Text($0.name).tag(String?.some($0.id)) }
                        }
                    }
                }
                }
                Section {
                    Picker("Repeats", selection: $frequency) {
                        ForEach(CronWords.Frequency.allCases) { Text($0.rawValue).tag($0) }
                    }
                    if frequency == .weekly {
                        Picker("Day", selection: $weekday) {
                            ForEach(0..<7, id: \.self) { Text(Calendar.current.weekdaySymbols[$0]).tag($0) }
                        }
                    }
                    DatePicker(frequency == .hourly ? "Minute" : "Time", selection: $time, displayedComponents: .hourAndMinute)
                } footer: {
                    Text(CronWords.describe(type: "cron", expression: cron))
                }
                if let error {
                    Text(error).font(.caption).foregroundColor(Theme.statusError)
                }
            }
            .navigationTitle(title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button(isSaving ? "Saving…" : "Save") { Task { await save() } }
                        .disabled(!canSave)
                }
            }
            .task {
                applySlot()
                bots.fetchAgentsIfNeeded()
            }
        }
    }

    @MainActor
    private func save() async {
        isSaving = true
        error = nil
        defer { isSaving = false }
        let trimmedName = name.trimmingCharacters(in: .whitespaces)
        let prompt = instruction.trimmingCharacters(in: .whitespacesAndNewlines)
        do {
            switch kind {
            case .loop:
                try await AutomationClient.shared.createLoop(CreateAutomationLoopRequest(
                    name: trimmedName,
                    description: prompt,
                    agentId: botId,
                    scheduleType: "cron",
                    scheduleExpression: cron,
                    executionDomain: "local",
                    config: ["jobType": "agent", "prompt": prompt]
                ))
            case .heartbeat:
                try await CronJobStore.shared.createAgentJob(name: trimmedName, prompt: prompt, schedule: cron)
            case .goal, .routine:
                guard let botId else { return }
                try await store.create(CreateAutomationRoutineRequest(
                    name: trimmedName,
                    description: nil,
                    agentId: botId,
                    scheduleType: "cron",
                    scheduleExpression: cron,
                    timezone: TimeZone.current.identifier,
                    executionDomain: "local",
                    config: ["jobType": "agent", "prompt": prompt]
                ))
            }
            UINotificationFeedbackGenerator().notificationOccurred(.success)
            dismiss()
        } catch {
            self.error = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }
}
