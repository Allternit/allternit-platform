import SwiftUI
import UIKit

// Schedule maths for the Automation Tasks calendar and graph, ported from
// allternit-ai src/lib/bots/routine-occurrences.ts (cron + interval run times,
// mirrors the API's local scheduler) and
// src/components/bots/schedule/routine-rule.ts (a schedule as the calendar
// sees it, and the cron it saves back as). Times are minutes after midnight in
// the viewer's local time.

// MARK: - routine-occurrences.ts

struct AutomationCronSpec {
    let minutes: [Int]
    let hours: [Int]
    let doms: Set<Int>
    let months: Set<Int>
    let dows: Set<Int>
    let domRestricted: Bool
    let dowRestricted: Bool
}

enum RoutineOccurrences {
    /// Hard cap so a `* * * * *` routine can't freeze a month view.
    static let maxPerRoutine = 500

    private static func parseField(_ field: String, _ min: Int, _ max: Int) -> Set<Int>? {
        var out = Set<Int>()
        for part in field.split(separator: ",", omittingEmptySubsequences: false) {
            let pieces = part.split(separator: "/", omittingEmptySubsequences: false).map(String.init)
            let rangePart = pieces[0]
            var step = 1
            if pieces.count > 1 {
                guard let parsed = Int(pieces[1]), parsed >= 1 else { return nil }
                step = parsed
            }
            let start: Int
            let end: Int
            if rangePart == "*" {
                start = min
                end = max
            } else if rangePart.contains("-") {
                let bounds = rangePart.split(separator: "-", omittingEmptySubsequences: false).map { Int($0) }
                guard bounds.count >= 2, let a = bounds[0], let b = bounds[1] else { return nil }
                start = a
                end = b
            } else {
                guard let value = Int(rangePart) else { return nil }
                start = value
                end = value
            }
            guard start >= min, end <= max, start <= end else { return nil }
            out.formUnion(stride(from: start, through: end, by: step))
        }
        return out.isEmpty ? nil : out
    }

    static func parseCron(_ expression: String) -> AutomationCronSpec? {
        let f = expression.split(whereSeparator: \.isWhitespace).map(String.init)
        guard f.count == 5,
              let minutes = parseField(f[0], 0, 59),
              let hours = parseField(f[1], 0, 23),
              let doms = parseField(f[2], 1, 31),
              let months = parseField(f[3], 1, 12),
              var dows = parseField(f[4], 0, 7) else { return nil }
        if dows.contains(7) { dows.insert(0) }
        return AutomationCronSpec(
            minutes: minutes.sorted(), hours: hours.sorted(), doms: doms, months: months, dows: dows,
            domRestricted: f[2] != "*", dowRestricted: f[4] != "*"
        )
    }

    private static func dayMatches(_ c: AutomationCronSpec, _ day: Date) -> Bool {
        let calendar = Calendar.current
        guard c.months.contains(calendar.component(.month, from: day)) else { return false }
        let dom = c.doms.contains(calendar.component(.day, from: day))
        let dow = c.dows.contains(calendar.component(.weekday, from: day) - 1)
        if c.domRestricted && c.dowRestricted { return dom || dow }
        if c.domRestricted { return dom }
        if c.dowRestricted { return dow }
        return true
    }

    /// Cron run times in [from, to), local time.
    static func cronTimes(_ expression: String, from: Date, to: Date, limit: Int = maxPerRoutine) -> [Date] {
        guard let c = parseCron(expression), to > from else { return [] }
        let calendar = Calendar.current
        var out: [Date] = []
        var day = calendar.startOfDay(for: from)
        while day < to && out.count < limit {
            if dayMatches(c, day) {
                for h in c.hours {
                    for m in c.minutes {
                        guard let t = calendar.date(bySettingHour: h, minute: m, second: 0, of: day) else { continue }
                        if t >= from && t < to { out.append(t) }
                        if out.count >= limit { return out }
                    }
                }
            }
            guard let next = calendar.date(byAdding: .day, value: 1, to: day) else { break }
            day = next
        }
        return out
    }

    /// "90s", "5 min", "2h", "1d" in seconds.
    static func intervalSeconds(_ expression: String) -> TimeInterval? {
        let trimmed = expression.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        let digits = trimmed.prefix(while: \.isNumber)
        guard let n = Int(digits), n > 0 else { return nil }
        let unit = trimmed.dropFirst(digits.count).trimmingCharacters(in: .whitespaces)
        let multiplier: Int
        switch unit {
        case "", "s", "sec", "secs", "second", "seconds": multiplier = unit.isEmpty ? 0 : 1
        case "m", "min", "mins", "minute", "minutes": multiplier = 60
        case "h", "hr", "hrs", "hour", "hours": multiplier = 3600
        case "d", "day", "days": multiplier = 86_400
        default: multiplier = 0
        }
        return multiplier > 0 ? TimeInterval(n * multiplier) : nil
    }

    /// A routine's run times in [from, to).
    static func routineOccurrences(_ r: AutomationRoutine, from: Date, to: Date, limit: Int = maxPerRoutine) -> [Date] {
        guard r.isEnabled else { return [] }
        let next = libraryParseDate(r.nextRunAt)
        if r.scheduleType == "cron" {
            // Future runs only; a run due but not yet fired is the scheduler's cursor.
            let now = Date()
            let floor = next.map { $0 < now ? $0 : now } ?? now
            return cronTimes(r.scheduleExpression, from: max(from, floor), to: to, limit: limit)
        }
        guard let next else { return [] }
        if r.scheduleType == "interval", let step = intervalSeconds(r.scheduleExpression) {
            var out: [Date] = []
            var t = next
            if t < from { t = t.addingTimeInterval((from.timeIntervalSince(t) / step).rounded(.up) * step) }
            while t < to && out.count < limit {
                out.append(t)
                t = t.addingTimeInterval(step)
            }
            return out
        }
        return next >= from && next < to ? [next] : []
    }
}

// MARK: - routine-rule.ts

struct AutomationRoutineRule: Equatable, Sendable {
    enum Repeat: Sendable { case daily, weekdays, weekly, monthly, interval }

    var repeatKind: Repeat
    /// First (or only) run, minutes after midnight.
    var at: Int
    /// Interval routines: last run of the day, minutes after midnight.
    var until: Int? = nil
    /// Interval routines: minutes between runs.
    var every: Int? = nil
    /// Weekdays it runs on (0 = Sunday). All seven for daily and monthly.
    var days: [Int]
    /// Monthly routines: days of the month.
    var dom: [Int]? = nil

    static let allDays = [0, 1, 2, 3, 4, 5, 6]
    /// The gaps the editor offers for "every few hours", in minutes.
    static let intervalChoices = [30, 60, 120, 180, 240]
    static let weekdays = [1, 2, 3, 4, 5]
    static let dayShort = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]

    /// Rule for a 5-field cron, or nil when it is something the editor can't show (e.g. specific months).
    static func fromCron(_ expression: String) -> AutomationRoutineRule? {
        guard let c = RoutineOccurrences.parseCron(expression), c.months.count == 12 else { return nil }
        let times = c.hours.flatMap { h in c.minutes.map { m in h * 60 + m } }
        guard let first = times.first else { return nil }
        let days = c.dowRestricted ? c.dows.filter { $0 < 7 }.sorted() : allDays
        if c.domRestricted {
            if c.dowRestricted || times.count > 1 { return nil }
            return AutomationRoutineRule(repeatKind: .monthly, at: first, days: allDays, dom: c.doms.sorted())
        }
        if times.count > 1 {
            let every = times[1] - times[0]
            guard evenlySpaced(times, every), intervalChoices.contains(every) else { return nil }
            return AutomationRoutineRule(repeatKind: .interval, at: first, until: times.last, every: every, days: days)
        }
        let kind: Repeat = days.count == 7 ? .daily : (days == weekdays ? .weekdays : .weekly)
        return AutomationRoutineRule(repeatKind: kind, at: first, days: days)
    }

    /// Rule for a server schedule; `interval` ("2h") becomes every N h all day.
    static func fromSchedule(type: String, expression: String) -> AutomationRoutineRule? {
        if type == "cron" { return fromCron(expression) }
        if type == "interval" {
            let every = Int(((RoutineOccurrences.intervalSeconds(expression) ?? 0) / 60).rounded())
            guard intervalChoices.contains(every) else { return nil }
            return AutomationRoutineRule(repeatKind: .interval, at: 0, until: 24 * 60 - every, every: every, days: allDays)
        }
        return nil
    }

    fileprivate static func evenlySpaced(_ times: [Int], _ every: Int) -> Bool {
        times.indices.allSatisfy { $0 == 0 || times[$0] - times[$0 - 1] == every }
    }

    /// The cron the API runs for a rule. Interval windows run on whole hours or every N minutes.
    var cron: String {
        let hh = at / 60
        let mm = at % 60
        let unique = Array(Set(days)).sorted()
        let dow: String
        if repeatKind == .daily || repeatKind == .monthly || unique.count == 7 {
            dow = "*"
        } else if unique == Self.weekdays {
            dow = "1-5"
        } else {
            dow = unique.map(String.init).joined(separator: ",")
        }
        switch repeatKind {
        case .monthly:
            let list = (dom?.isEmpty == false ? dom! : [1]).map(String.init).joined(separator: ",")
            return "\(mm) \(hh) \(list) * *"
        case .interval:
            let gap = every ?? 60
            let lastHour = (until ?? at) / 60
            if gap % 60 == 0 { return "\(mm) \(hh)-\(lastHour)/\(gap / 60) * * \(dow)" }
            return "*/\(gap) \(hh)-\(lastHour) * * \(dow)"
        case .daily, .weekdays, .weekly:
            return "\(mm) \(hh) * * \(dow)"
        }
    }

    /// Does the rule run on this local day?
    func runs(on day: Date) -> Bool {
        let calendar = Calendar.current
        switch repeatKind {
        case .monthly: return (dom ?? []).contains(calendar.component(.day, from: day))
        case .daily: return true
        case .weekdays:
            let weekday = calendar.component(.weekday, from: day) - 1
            return weekday >= 1 && weekday <= 5
        case .weekly, .interval:
            return days.contains(calendar.component(.weekday, from: day) - 1)
        }
    }

    /// Moves a rule to a new start time, keeping an interval window's length.
    func moved(to minute: Int) -> AutomationRoutineRule {
        var next = self
        guard repeatKind == .interval else {
            next.at = minute
            return next
        }
        let span = (until ?? at) - at
        let start = max(0, min(24 * 60 - 1 - span, minute))
        next.at = start
        next.until = start + span
        return next
    }

    /// A sensible rule for a new routine picked at a calendar slot.
    static func forSlot(day: Date, at minute: Int) -> AutomationRoutineRule {
        let dow = Calendar.current.component(.weekday, from: day) - 1
        return dow >= 1 && dow <= 5
            ? AutomationRoutineRule(repeatKind: .weekdays, at: minute, days: weekdays)
            : AutomationRoutineRule(repeatKind: .weekly, at: minute, days: [dow])
    }

    /// "Weekdays · 9:05 AM", "Every 2 h · 8a–6p weekdays".
    var label: String {
        let t = Self.fmtTime(at)
        switch repeatKind {
        case .daily: return "Every day · \(t)"
        case .weekdays: return "Weekdays · \(t)"
        case .weekly: return "\(days.map { Self.dayShort[$0] }.joined(separator: ", ")) · \(t)"
        case .monthly:
            let list = (dom ?? [1]).map(Self.ordinal).joined(separator: " & ")
            return "Monthly on the \(list) · \(t)"
        case .interval:
            let on = days.count == 7 ? "" : (days == Self.weekdays ? " weekdays" : " \(days.map { Self.dayShort[$0] }.joined(separator: ", "))")
            return "Every \(Self.everyLabel(every ?? 60)) · \(Self.fmtShort(at))–\(Self.fmtShort(until ?? at))\(on)"
        }
    }

    static func fmtTime(_ minute: Int) -> String {
        let h = minute / 60
        let label = h % 12 == 0 ? 12 : h % 12
        return "\(label):\(String(format: "%02d", minute % 60)) \(h < 12 ? "AM" : "PM")"
    }

    /// "9a", "9:05a", "6p".
    static func fmtShort(_ minute: Int) -> String {
        let h = minute / 60
        let m = minute % 60
        let label = h % 12 == 0 ? 12 : h % 12
        return "\(label)\(m > 0 ? ":\(String(format: "%02d", m))" : "")\(h < 12 ? "a" : "p")"
    }

    /// "9:00a", "4:30p": a time that always shows its minutes, for calendar chips.
    static func fmtClock(_ minute: Int) -> String {
        let h = minute / 60
        let label = h % 12 == 0 ? 12 : h % 12
        return "\(label):\(String(format: "%02d", minute % 60))\(h < 12 ? "a" : "p")"
    }

    static func everyLabel(_ every: Int) -> String {
        every % 60 == 0 ? "\(every / 60) h" : "\(every) min"
    }

    private static func ordinal(_ n: Int) -> String {
        let tens = n % 100
        if tens >= 11 && tens <= 13 { return "\(n)th" }
        switch n % 10 {
        case 1: return "\(n)st"
        case 2: return "\(n)nd"
        case 3: return "\(n)rd"
        default: return "\(n)th"
        }
    }
}

// MARK: - AutomationSchedule.tsx helpers

enum AutomationScheduleLogic {
    private static func isCron(_ expression: String) -> Bool {
        expression.split(whereSeparator: \.isWhitespace).count == 5
    }

    /// Where a task sits on the calendar. Loops often run every few minutes,
    /// which the routine editor doesn't offer, so any even spacing draws as a band.
    static func taskRule(_ raw: String?) -> AutomationRoutineRule? {
        let e = raw?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard !e.isEmpty else { return nil }
        if let known = AutomationRoutineRule.fromSchedule(type: isCron(e) ? "cron" : "interval", expression: e) { return known }
        if !isCron(e) {
            let every = Int(((RoutineOccurrences.intervalSeconds(e) ?? 0) / 60).rounded())
            guard every >= 1, every < 24 * 60 else { return nil }
            return AutomationRoutineRule(repeatKind: .interval, at: 0, until: 24 * 60 - every, every: every, days: AutomationRoutineRule.allDays)
        }
        guard let c = RoutineOccurrences.parseCron(e), c.months.count == 12, !c.domRestricted else { return nil }
        let times = c.hours.flatMap { h in c.minutes.map { m in h * 60 + m } }
        guard times.count >= 2 else { return nil }
        let every = times[1] - times[0]
        guard AutomationRoutineRule.evenlySpaced(times, every) else { return nil }
        let days = c.dowRestricted ? c.dows.filter { $0 < 7 }.sorted() : AutomationRoutineRule.allDays
        return AutomationRoutineRule(repeatKind: .interval, at: times[0], until: times.last, every: every, days: days)
    }

    /// A task's run times in [from, to), at most `limit`.
    static func occurrences(_ t: AutomationTaskItem, from: Date, to: Date, limit: Int = 50) -> [Date] {
        let e = t.scheduleExpression?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard t.isActive, t.type != .goal, !e.isEmpty else { return [] }
        if isCron(e) { return RoutineOccurrences.cronTimes(e, from: max(from, Date()), to: to, limit: limit) }
        guard let step = RoutineOccurrences.intervalSeconds(e) else { return [] }
        var at = libraryParseDate(t.nextRun) ?? Date().addingTimeInterval(step)
        if at < from { at = at.addingTimeInterval((from.timeIntervalSince(at) / step).rounded(.up) * step) }
        var out: [Date] = []
        while at < to && out.count < limit {
            out.append(at)
            at = at.addingTimeInterval(step)
        }
        return out
    }

    /// "2026-11-01" is a calendar date; read it in local time, not UTC midnight.
    static func goalDay(_ t: AutomationTaskItem) -> Date? {
        guard t.type == .goal, let raw = t.targetDate, raw.count >= 10 else { return nil }
        let parts = raw.prefix(10).split(separator: "-").compactMap { Int($0) }
        guard parts.count == 3 else { return nil }
        return Calendar.current.date(from: DateComponents(year: parts[0], month: parts[1], day: parts[2]))
    }

    /// "Today at 2:34 PM", "Tomorrow at 7:00 AM", "Mon, Nov 2 at 9:00 AM".
    static func when(_ date: Date) -> String {
        let time = date.formatted(date: .omitted, time: .shortened)
        let calendar = Calendar.current
        if calendar.isDateInToday(date) { return "Today at \(time)" }
        if calendar.isDateInTomorrow(date) { return "Tomorrow at \(time)" }
        return "\(shortDay(date)) at \(time)"
    }

    /// "Last run Today at 2:34 PM", or "No run yet".
    static func lastRunLine(_ raw: String?) -> String {
        guard let date = libraryParseDate(raw) else { return "No run yet" }
        return "Last run \(when(date))"
    }

    /// "Mon, Nov 2".
    static func shortDay(_ date: Date) -> String {
        date.formatted(.dateTime.weekday(.abbreviated).month(.abbreviated).day())
    }

    static func scheduleLine(_ t: AutomationTaskItem) -> String {
        if t.type == .goal {
            guard let day = goalDay(t) else { return "No target date" }
            return "Due \(day.formatted(.dateTime.month(.abbreviated).day()))"
        }
        if !t.isActive { return t.status == "error" ? "Stopped after an error" : "Paused" }
        let now = Date()
        if let next = occurrences(t, from: now, to: now.addingTimeInterval(31 * 86_400), limit: 1).first {
            return "Next \(when(next))"
        }
        let e = t.scheduleExpression?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return e.isEmpty ? "Runs when you start it" : e
    }

    /// "Every day · 9:00 AM", "Every 5 min · 12a–11:55p", or the raw schedule when it has no words.
    static func scheduleWords(_ t: AutomationTaskItem) -> String? {
        guard t.type != .goal else { return nil }
        let e = t.scheduleExpression?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard !e.isEmpty else { return "Runs when you start it" }
        return taskRule(e)?.label ?? e
    }

    /// A routine name without its `[bot:<name>] ` owner prefix, for display.
    static func displayName(_ name: String) -> String {
        guard name.hasPrefix("[bot:"), let close = name.firstIndex(of: "]") else { return name }
        return String(name[name.index(after: close)...]).trimmingCharacters(in: .whitespaces)
    }

    /// cronFrequency (AutomationTasksView.tsx): the word on a list card.
    static func cronFrequency(_ expression: String?) -> String {
        let f = (expression ?? "").split(whereSeparator: \.isWhitespace).map(String.init)
        guard f.count == 5, f[2] == "*", f[3] == "*" else { return "manual" }
        if f[1] == "*" { return f[4] == "*" ? "hourly" : "manual" }
        if f[4] == "*" { return "daily" }
        if f[4] == "1-5" { return "weekdays" }
        return f[4].count == 1 && "01234567".contains(f[4]) ? "weekly" : "manual"
    }
}

// MARK: - Colours

extension AutomationTaskItem.Kind {
    /// TYPE_COLOR (AutomationSchedule.tsx): goal blue, routine emerald, loop purple, heartbeat amber.
    var scheduleColor: Color {
        switch self {
        case .goal: return Color(hex: "#3B82F6")
        case .routine: return Color(hex: "#10B981")
        case .loop: return Color(hex: "#A855F7")
        case .heartbeat: return Color(hex: "#F59E0B")
        }
    }
}

enum SchedulerTint {
    /// FALLBACK_COLOR: a bot with no colour of its own.
    static let fallback = Color(hex: "#5E5E66")

    /// `tintText`: the colour pulled toward the primary text colour (62%), per
    /// light or dark, so a tinted chip stays readable in both.
    static func text(_ color: Color) -> Color {
        let tint = UIColor(color)
        return Color(uiColor: UIColor { traits in
            let ink = (UIColor(named: "TextPrimary") ?? .label).resolvedColor(with: traits)
            let resolved = tint.resolvedColor(with: traits)
            var (r1, g1, b1, a1): (CGFloat, CGFloat, CGFloat, CGFloat) = (0, 0, 0, 0)
            var (r2, g2, b2, a2): (CGFloat, CGFloat, CGFloat, CGFloat) = (0, 0, 0, 0)
            resolved.getRed(&r1, green: &g1, blue: &b1, alpha: &a1)
            ink.getRed(&r2, green: &g2, blue: &b2, alpha: &a2)
            return UIColor(red: r1 * 0.62 + r2 * 0.38, green: g1 * 0.62 + g2 * 0.38, blue: b1 * 0.62 + b2 * 0.38, alpha: 1)
        })
    }
}
