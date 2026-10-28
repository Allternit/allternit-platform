import SwiftUI

// -----------------------------------------------------------------------------
// A routine's schedule as people see it ("Weekdays · 9:05 AM") and the cron the
// computer's scheduler runs — the port of allternit-ai
// `src/components/bots/schedule/routine-rule.ts` (ruleFromCron / ruleToCron /
// ruleLabel, the mapping team-slots.ts uses) and `lib/bots/routine-occurrences.ts`
// parseCron. Times are minutes after midnight, local time.
// -----------------------------------------------------------------------------

struct RoutineRule: Equatable, Sendable {
    enum Repeat: String, CaseIterable, Sendable { case daily, weekdays, weekly, monthly, interval }
    var repeatKind: Repeat
    /// First (or only) run, minutes after midnight.
    var at: Int
    /// Interval routines: last run of the day.
    var until: Int? = nil
    /// Interval routines: minutes between runs.
    var every: Int? = nil
    /// Weekdays it runs on (0 = Sunday).
    var days: [Int]
    /// Monthly routines: days of the month.
    var dom: [Int]? = nil

    static let allDays = [0, 1, 2, 3, 4, 5, 6]
    static let weekdays = [1, 2, 3, 4, 5]
    static let intervalChoices = [30, 60, 120, 180, 240]
    static let dayShort = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]

    // MARK: cron → rule

    private static func parseField(_ field: Substring, _ lo: Int, _ hi: Int) -> Set<Int>? {
        var out = Set<Int>()
        for part in field.split(separator: ",") {
            let pieces = part.split(separator: "/", omittingEmptySubsequences: false)
            let step = pieces.count > 1 ? Int(pieces[1]) : 1
            guard let step, step >= 1 else { return nil }
            let range = pieces[0]
            var start: Int?, end: Int?
            if range == "*" {
                start = lo; end = hi
            } else if range.contains("-") {
                let ab = range.split(separator: "-")
                guard ab.count == 2 else { return nil }
                start = Int(ab[0]); end = Int(ab[1])
            } else {
                start = Int(range); end = start
            }
            guard let s = start, let e = end, s >= lo, e <= hi, s <= e else { return nil }
            out.formUnion(stride(from: s, through: e, by: step))
        }
        return out.isEmpty ? nil : out
    }

    /// Rule for a 5-field cron, or nil when the editor can't show it (e.g. specific months).
    init?(cron: String) {
        let f = cron.split(whereSeparator: \.isWhitespace)
        guard f.count == 5,
              let minutes = Self.parseField(f[0], 0, 59), let hours = Self.parseField(f[1], 0, 23),
              let doms = Self.parseField(f[2], 1, 31), let months = Self.parseField(f[3], 1, 12),
              var dows = Self.parseField(f[4], 0, 7), months.count == 12 else { return nil }
        if dows.contains(7) { dows.insert(0) }
        let times = hours.sorted().flatMap { h in minutes.sorted().map { h * 60 + $0 } }
        guard let first = times.first else { return nil }
        let dowRestricted = f[4] != "*", domRestricted = f[2] != "*"
        let days = dowRestricted ? dows.filter { $0 < 7 }.sorted() : Self.allDays
        if domRestricted {
            guard !dowRestricted, times.count == 1 else { return nil }
            self.init(repeatKind: .monthly, at: first, days: Self.allDays, dom: doms.sorted())
            return
        }
        if times.count > 1 {
            let every = times[1] - times[0]
            let even = times.indices.dropFirst().allSatisfy { times[$0] - times[$0 - 1] == every }
            guard even, Self.intervalChoices.contains(every) else { return nil }
            self.init(repeatKind: .interval, at: first, until: times.last, every: every, days: days)
            return
        }
        let kind: Repeat = days.count == 7 ? .daily : days == Self.weekdays ? .weekdays : .weekly
        self.init(repeatKind: kind, at: first, days: days)
    }

    init(repeatKind: Repeat, at: Int, until: Int? = nil, every: Int? = nil, days: [Int], dom: [Int]? = nil) {
        self.repeatKind = repeatKind
        self.at = at
        self.until = until
        self.every = every
        self.days = days
        self.dom = dom
    }

    // MARK: rule → cron

    var cron: String {
        let hh = at / 60, mm = at % 60
        let d = Array(Set(days)).sorted()
        let dow = repeatKind == .daily || repeatKind == .monthly || d.count == 7 ? "*"
            : d == Self.weekdays ? "1-5" : d.map(String.init).joined(separator: ",")
        switch repeatKind {
        case .monthly:
            return "\(mm) \(hh) \((dom?.isEmpty == false ? dom! : [1]).map(String.init).joined(separator: ",")) * *"
        case .interval:
            let e = every ?? 60
            let lastHour = (until ?? at) / 60
            if e % 60 == 0 { return "\(mm) \(hh)-\(lastHour)/\(e / 60) * * \(dow)" }
            return "*/\(e) \(hh)-\(lastHour) * * \(dow)"
        default:
            return "\(mm) \(hh) * * \(dow)"
        }
    }

    // MARK: words

    static func time(_ m: Int) -> String {
        let h = m / 60
        return "\(h % 12 == 0 ? 12 : h % 12):\(String(format: "%02d", m % 60)) \(h < 12 ? "AM" : "PM")"
    }

    private static func short(_ m: Int) -> String {
        let h = m / 60, min = m % 60
        return "\(h % 12 == 0 ? 12 : h % 12)\(min > 0 ? ":\(String(format: "%02d", min))" : "")\(h < 12 ? "a" : "p")"
    }

    private static func ordinal(_ n: Int) -> String {
        let tens = n % 100
        if (11...13).contains(tens) { return "\(n)th" }
        return "\(n)" + (["th", "st", "nd", "rd"].indices.contains(n % 10) ? ["th", "st", "nd", "rd"][n % 10] : "th")
    }

    /// "Weekdays · 9:05 AM", "Every 2 h · 8a–6p weekdays".
    var label: String {
        let t = Self.time(at)
        switch repeatKind {
        case .daily: return "Every day · \(t)"
        case .weekdays: return "Weekdays · \(t)"
        case .weekly: return "\(days.map { Self.dayShort[$0] }.joined(separator: ", ")) · \(t)"
        case .monthly: return "Monthly on the \((dom ?? [1]).map(Self.ordinal).joined(separator: " & ")) · \(t)"
        case .interval:
            let e = every ?? 60
            let span = days.count == 7 ? "" : days == Self.weekdays ? " weekdays" : " " + days.map { Self.dayShort[$0] }.joined(separator: ", ")
            return "Every \(e % 60 == 0 ? "\(e / 60) h" : "\(e) min") · \(Self.short(at))–\(Self.short(until ?? at))\(span)"
        }
    }

    var isValid: Bool {
        switch repeatKind {
        case .weekly: return !days.isEmpty
        case .interval: return !days.isEmpty && (until ?? -1) >= at && Self.intervalChoices.contains(every ?? 0)
        case .monthly: return !(dom ?? []).isEmpty && (dom ?? []).allSatisfy { (1...28).contains($0) }
        default: return true
        }
    }
}

/// What the editor saved.
struct RoutineEdit {
    var rule: RoutineRule
    var title: String
    /// The bot that runs it (a `botChoices` value), when the editor offered a choice.
    var botKey: String?
}

/// Change when a routine runs: every day, weekdays, or chosen days at a time
/// (the web RoutineEditor's daily / weekdays / some days). A routine already on
/// a monthly or every-few-hours schedule keeps that shape and can still move
/// its time. A new routine also takes what it does and which bot runs it.
/// Saves a rule; the caller turns it into cron (`rule.cron`).
struct RoutineScheduleEditor: View {
    let title: String
    let initial: RoutineRule
    var titleEditable = false
    var botChoices: [WizardOption] = []
    var saving = false
    var error: String? = nil
    var onSave: (RoutineEdit) -> Void
    var onDelete: (() -> Void)? = nil
    var onCancel: () -> Void

    @State private var rule: RoutineRule
    @State private var time: Date
    @State private var text: String
    @State private var botKey: String

    private static let weekOrder = [1, 2, 3, 4, 5, 6, 0]

    init(title: String, initial: RoutineRule, titleEditable: Bool = false, botChoices: [WizardOption] = [],
         initialBot: String? = nil, saving: Bool = false, error: String? = nil,
         onSave: @escaping (RoutineEdit) -> Void, onDelete: (() -> Void)? = nil, onCancel: @escaping () -> Void) {
        self.title = title
        self.initial = initial
        self.titleEditable = titleEditable
        self.botChoices = botChoices
        self.saving = saving
        self.error = error
        self.onSave = onSave
        self.onDelete = onDelete
        self.onCancel = onCancel
        _rule = State(initialValue: initial)
        _text = State(initialValue: title)
        _botKey = State(initialValue: initialBot ?? botChoices.first?.value ?? "")
        let start = Calendar.current.startOfDay(for: Date())
        _time = State(initialValue: start.addingTimeInterval(TimeInterval(initial.at * 60)))
    }

    private var canSave: Bool {
        rule.isValid && (!titleEditable || !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
    }

    private var choices: [(RoutineRule.Repeat, String)] {
        var out: [(RoutineRule.Repeat, String)] = [(.daily, "Every day"), (.weekdays, "Weekdays"), (.weekly, "Some days")]
        if initial.repeatKind == .monthly { out.append((.monthly, "Monthly")) }
        if initial.repeatKind == .interval { out.append((.interval, "Every few hours")) }
        return out
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    if titleEditable {
                        TextField("e.g. Send me a morning brief", text: $text, axis: .vertical)
                            .lineLimit(1...4)
                    } else {
                        Text(title).font(.subheadline.weight(.semibold)).foregroundColor(Color("TextPrimary"))
                    }
                    if botChoices.count > 1 {
                        Picker("Runs as", selection: $botKey) {
                            ForEach(botChoices, id: \.value) { Text($0.label).tag($0.value) }
                        }
                    }
                } header: {
                    if titleEditable { Text("What it does") }
                }
                Section("Repeats") {
                    Picker("Repeats", selection: Binding(get: { rule.repeatKind }, set: setRepeat)) {
                        ForEach(Array(choices.enumerated()), id: \.offset) { _, c in Text(c.1).tag(c.0) }
                    }
                    .pickerStyle(.inline)
                    .labelsHidden()
                    if rule.repeatKind == .weekly {
                        HStack(spacing: 6) {
                            ForEach(Self.weekOrder, id: \.self) { d in
                                let on = rule.days.contains(d)
                                Button {
                                    if on { rule.days.removeAll { $0 == d } } else { rule.days.append(d); rule.days.sort() }
                                } label: {
                                    Text(String(RoutineRule.dayShort[d].prefix(2)))
                                        .font(.caption.weight(.semibold))
                                        .foregroundColor(on ? .white : Color("TextPrimary"))
                                        .frame(maxWidth: .infinity, minHeight: 34)
                                        .background(Capsule().fill(on ? Color("AccentPrimary") : Color("BgSecondary")))
                                }
                                .buttonStyle(.plain)
                                .accessibilityLabel(RoutineRule.dayShort[d])
                                .accessibilityAddTraits(on ? .isSelected : [])
                            }
                        }
                    }
                }
                Section(rule.repeatKind == .interval ? "Starts at" : "At") {
                    DatePicker("Time", selection: $time, displayedComponents: .hourAndMinute)
                        .onChange(of: time) { _, t in
                            let c = Calendar.current.dateComponents([.hour, .minute], from: t)
                            let at = (c.hour ?? 0) * 60 + (c.minute ?? 0)
                            if rule.repeatKind == .interval {
                                // Keep an interval window's length (moveRule).
                                let span = (rule.until ?? rule.at) - rule.at
                                rule.at = max(0, min(24 * 60 - 1 - span, at))
                                rule.until = rule.at + span
                            } else {
                                rule.at = at
                            }
                        }
                }
                Section {
                    Text(rule.label).font(.footnote).foregroundColor(Color("TextSecondary"))
                    if let error { Text(error).font(.footnote).foregroundColor(Theme.statusError) }
                }
                if let onDelete {
                    Section { Button("Remove routine", role: .destructive, action: onDelete) }
                }
            }
            .navigationTitle("Schedule")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel", action: onCancel) }
                ToolbarItem(placement: .confirmationAction) {
                    if saving { ProgressView() } else {
                        Button("Save") {
                            onSave(RoutineEdit(rule: rule, title: text.trimmingCharacters(in: .whitespacesAndNewlines),
                                               botKey: botChoices.isEmpty ? nil : botKey))
                        }
                        .disabled(!canSave)
                    }
                }
            }
        }
        .presentationDetents([.medium, .large])
    }

    private func setRepeat(_ r: RoutineRule.Repeat) {
        switch r {
        case .daily: rule = RoutineRule(repeatKind: .daily, at: rule.at, days: RoutineRule.allDays)
        case .weekdays: rule = RoutineRule(repeatKind: .weekdays, at: rule.at, days: RoutineRule.weekdays)
        case .weekly:
            let keep = rule.days.count == 7 ? [Calendar.current.component(.weekday, from: Date()) - 1] : rule.days
            rule = RoutineRule(repeatKind: .weekly, at: rule.at, days: keep)
        case .monthly, .interval:
            rule = initial
            rule.at = initial.at
        }
    }
}
