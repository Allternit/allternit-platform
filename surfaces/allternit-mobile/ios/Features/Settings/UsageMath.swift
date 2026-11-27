import Foundation

// Settings → Usage selectors (phone calm I15). The iOS mirror of the web's
// `components/settings/PlanUsageBars.tsx` exports (ai #1161: `usagePercent`,
// `resetsText`, `windowTitle`, `sortWindows`, `USAGE_THRESHOLD_TEXT`) and of
// `lib/usage/provider-limits.ts` (`WARN_AT`, `LAND_AT`, `worstWindow`,
// `resetLabel`, `windowsFromCloudSummary`). Pure: no stores, no views, so
// `Tests/UsageMathTests.swift` runs them directly.

/// The usage thresholds the harness really uses: gizzi's `limits.warn_at`
/// (default 0.8, "approaching usage limit") and `limits.land_at` (default
/// 0.95, the session wraps up before the wall), defined in
/// `cmd/gizzi-code/src/runtime/context/config/config.ts`. The web reads the
/// same two numbers from `lib/usage/provider-limits.ts` (`WARN_AT`,
/// `LAND_AT`). No API field carries them.
enum UsageThresholds {
    static let warnAt = 0.8
    static let landAt = 0.95

    /// The Usage page footer: "Heads-up at 80%. Tasks wrap up at 95%."
    /// Same text as the web's `USAGE_THRESHOLD_TEXT`.
    static var footerText: String {
        "Heads-up at \(Int((warnAt * 100).rounded()))%. Tasks wrap up at \(Int((landAt * 100).rounded()))%."
    }
}

/// Where a window sits against the thresholds (web `levelOf`).
enum UsageLevel: Equatable {
    case ok, approaching, reached
}

/// Level of a used share (0…1).
func usageLevel(_ usedRatio: Double) -> UsageLevel {
    if usedRatio >= UsageThresholds.landAt { return .reached }
    if usedRatio >= UsageThresholds.warnAt { return .approaching }
    return .ok
}

/// The window closest to its limit; on a tie, the one that resets last
/// (web `worstWindow`).
func worstUsageWindow(_ windows: [ProviderLimitWindow]) -> ProviderLimitWindow? {
    var worst: ProviderLimitWindow?
    for w in windows {
        guard let current = worst else { worst = w; continue }
        let resetW = w.resetDate?.timeIntervalSince1970 ?? 0
        let resetCurrent = current.resetDate?.timeIntervalSince1970 ?? 0
        if w.usedRatio > current.usedRatio || (w.usedRatio == current.usedRatio && resetW > resetCurrent) {
            worst = w
        }
    }
    return worst
}

/// Percent used of the window closest to its limit, 0–100, or nil when no
/// window is known. The Settings `usage` row ("N% used") and the Usage page
/// read the same windows through this.
func usagePercent(_ windows: [ProviderLimitWindow]) -> Int? {
    guard let worst = worstUsageWindow(windows) else { return nil }
    return min(100, max(0, Int((worst.usedRatio * 100).rounded())))
}

/// Shortest window first: 5-hour, then weekly, then monthly (web
/// `sortWindows`). Windows of one kind keep the order the provider sent.
func sortUsageWindows(_ windows: [ProviderLimitWindow]) -> [ProviderLimitWindow] {
    func rank(_ w: ProviderLimitWindow) -> Int {
        if w.id == "5h" { return 0 }
        if w.id.hasPrefix("7d") { return 1 }
        if w.id.hasPrefix("month") { return 2 }
        return 3
    }
    return windows.enumerated()
        .sorted { a, b in
            let ra = rank(a.element), rb = rank(b.element)
            return ra != rb ? ra < rb : a.offset < b.offset
        }
        .map(\.element)
}

/// "This 5-hour window", "This week", "This week · Opus", "This month"
/// (web `windowTitle`, over the shared display label).
func usageWindowTitle(_ window: ProviderLimitWindow) -> String {
    let label = window.displayLabel
    for (prefix, title) in [("5-hour", "This 5-hour window"), ("Weekly", "This week"), ("Monthly", "This month")]
    where label.hasPrefix(prefix) {
        return title + label.dropFirst(prefix.count)
    }
    return label
}

/// "11:55 AM" today, "Sat 9:00 AM" within six days, else "Dec 1, 12:00 AM"
/// (web `resetLabel`).
func usageResetLabel(_ at: Date, now: Date, calendar: Calendar = .current, locale: Locale = .current) -> String {
    func format(_ template: String) -> String {
        let f = DateFormatter()
        f.locale = locale
        f.calendar = calendar
        f.timeZone = calendar.timeZone
        f.setLocalizedDateFormatFromTemplate(template)
        return f.string(from: at)
    }
    let time = format("jmm")
    if calendar.isDate(at, inSameDayAs: now) { return time }
    if at.timeIntervalSince(now) < 6 * 86_400 { return "\(format("EEE")) \(time)" }
    return "\(format("MMMd")), \(time)"
}

/// The reset line under a bar (web `resetsText`): "Resets in a moment",
/// "Resets in 2h 10m" (under six hours), "Resets at 12:50 PM" (later
/// today), else "Resets Thu 8:00 AM" / "Resets Dec 1, 12:00 AM". nil when
/// the window has no reset time.
func usageResetsText(_ resetAt: Date?, now: Date = Date(), calendar: Calendar = .current, locale: Locale = .current) -> String? {
    guard let resetAt else { return nil }
    let left = resetAt.timeIntervalSince(now)
    if left <= 60 { return "Resets in a moment" }
    if left < 6 * 3_600 {
        // Minutes are rounded first, so 1h 59m 45s reads "2h", never "1h 60m".
        let minutes = Int((left / 60).rounded())
        let h = minutes / 60, m = minutes % 60
        if h == 0 { return "Resets in \(m)m" }
        return m > 0 ? "Resets in \(h)h \(m)m" : "Resets in \(h)h"
    }
    let label = usageResetLabel(resetAt, now: now, calendar: calendar, locale: locale)
    return calendar.isDate(resetAt, inSameDayAs: now) ? "Resets at \(label)" : "Resets \(label)"
}

/// Allternit Cloud's limit windows from `GET /api/v1/me/usage` (web
/// `windowsFromCloudSummary`). Cloud reports one window: the plan's monthly
/// allowance (the wire names it `weekly*`, but cloud-api `me_usage.rs`
/// fills it with the monthly grant and resets it on the 1st). So it is
/// always the "month" window, never a weekly one. Empty when unmetered.
func cloudUsageWindows(_ snapshot: UsageSnapshot?) -> [ProviderLimitWindow] {
    guard let snapshot, snapshot.weeklyLimit > 0, snapshot.weeklyUsed.isFinite else { return [] }
    let ratio = min(1, max(0, snapshot.weeklyUsed / snapshot.weeklyLimit))
    return [ProviderLimitWindow(id: "month", label: "Monthly", usedRatio: ratio, resetAt: snapshot.resetsAt)]
}

/// First moment of next month in `calendar` (the cloud computer hours reset).
func usageNextMonthStart(_ now: Date = Date(), calendar: Calendar = .current) -> Date {
    let start = calendar.date(from: calendar.dateComponents([.year, .month], from: now)) ?? now
    return calendar.date(byAdding: .month, value: 1, to: start) ?? now
}

/// "12.5 h of 40 h": hours to one decimal, no trailing ".0" (web
/// `Math.round(seconds / 360) / 10`).
func usageHoursText(usedSeconds: Int, maxHours: Int) -> String {
    let tenths = (Double(usedSeconds) / 360).rounded()
    let hours = tenths / 10
    let used = tenths.truncatingRemainder(dividingBy: 10) == 0 ? String(Int(hours)) : String(format: "%.1f", hours)
    return "\(used) h of \(maxHours) h"
}

/// What "Buy credits" does, from the same two gates every purchase entry
/// point uses (`ComputeGateView` card C): the In-App Purchase sheet where
/// the App Store sells packs, else the web billing page where links are
/// allowed, else nothing (the button is hidden). The Settings Usage page
/// passes `allowsPurchaseLinks: false`: it never links out to pay
/// (App Store 3.1.1, Claude polish i3).
enum BuyCreditsRoute: Equatable {
    case storeSheet
    case web
    case hidden

    static func resolve(storeSellsPacks: Bool, allowsPurchaseLinks: Bool) -> BuyCreditsRoute {
        if storeSellsPacks { return .storeSheet }
        if allowsPurchaseLinks { return .web }
        return .hidden
    }
}

// MARK: - Usage windows and pace (cloud-api #3101, snake_case)

/// One window from `GET /api/v1/me/usage` `windows` (id `5h` / `7d` /
/// `month`). `limit_usd` and `used_ratio` are null when the plan sets no
/// limit for that window: the row then shows spend with no bar.
struct CloudUsageWindow: Decodable, Equatable, Identifiable {
    let id: String
    let label: String
    let usedUsd: Double
    let limitUsd: Double?
    let usedRatio: Double?
    let resetsAt: String
    let enforced: Bool

    enum CodingKeys: String, CodingKey {
        case id, label, enforced
        case usedUsd = "used_usd"
        case limitUsd = "limit_usd"
        case usedRatio = "used_ratio"
        case resetsAt = "resets_at"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        label = (try? c.decode(String.self, forKey: .label)) ?? id
        usedUsd = (try? c.decode(Double.self, forKey: .usedUsd)) ?? 0
        limitUsd = try? c.decodeIfPresent(Double.self, forKey: .limitUsd)
        usedRatio = try? c.decodeIfPresent(Double.self, forKey: .usedRatio)
        resetsAt = (try? c.decode(String.self, forKey: .resetsAt)) ?? ""
        enforced = (try? c.decode(Bool.self, forKey: .enforced)) ?? false
    }

    init(id: String, label: String, usedUsd: Double, limitUsd: Double?, usedRatio: Double?, resetsAt: String, enforced: Bool = false) {
        self.id = id; self.label = label; self.usedUsd = usedUsd; self.limitUsd = limitUsd
        self.usedRatio = usedRatio; self.resetsAt = resetsAt; self.enforced = enforced
    }

    var resetDate: Date? { parseUsageDate(resetsAt) }

    /// The bar's share, nil when the window has no limit (no bar).
    var barFraction: Double? {
        guard let limitUsd, limitUsd > 0 else { return nil }
        return min(1, max(0, usedRatio ?? usedUsd / limitUsd))
    }
}

/// `pace` from `GET /api/v1/me/usage`: the burn of the window closest to
/// its limit. `runs_out_at` is null when that window cannot run out.
struct CloudUsagePace: Decodable, Equatable {
    let windowId: String
    let burnPerHour: Double
    let runsOutAt: String?
    let status: String

    enum CodingKeys: String, CodingKey {
        case status
        case windowId = "window_id"
        case burnPerHour = "burn_per_hour"
        case runsOutAt = "runs_out_at"
    }

    var runsOutDate: Date? { runsOutAt.flatMap(parseUsageDate) }
}

/// The two additive fields of `/me/usage` the Usage page reads. Both are
/// optional so an older server (no windows yet) decodes to an empty reply.
struct CloudUsageLimits: Decodable, Equatable {
    let windows: [CloudUsageWindow]
    let pace: CloudUsagePace?

    enum CodingKeys: String, CodingKey { case windows, pace }

    init(windows: [CloudUsageWindow], pace: CloudUsagePace?) {
        self.windows = windows; self.pace = pace
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        windows = (try? c.decodeIfPresent([CloudUsageWindow].self, forKey: .windows)) ?? []
        pace = try? c.decodeIfPresent(CloudUsagePace.self, forKey: .pace)
    }
}

/// RFC 3339 with or without fractional seconds.
func parseUsageDate(_ text: String) -> Date? {
    let f = ISO8601DateFormatter()
    f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    if let d = f.date(from: text) { return d }
    f.formatOptions = [.withInternetDateTime]
    return f.date(from: text)
}

/// Row title per window id, as the web frame (`M1-usage-phone`) reads.
func cloudWindowTitle(_ window: CloudUsageWindow) -> String {
    switch window.id {
    case "5h": return "Current session"
    case "7d": return "This week"
    case "month": return "This month"
    default: return window.label
    }
}

/// 5h, then 7d, then month; unknown ids keep server order after them.
func sortCloudWindows(_ windows: [CloudUsageWindow]) -> [CloudUsageWindow] {
    let rank = ["5h": 0, "7d": 1, "month": 2]
    return windows.enumerated()
        .sorted { a, b in
            let ra = rank[a.element.id] ?? 3, rb = rank[b.element.id] ?? 3
            return ra != rb ? ra < rb : a.offset < b.offset
        }
        .map(\.element)
}

/// The pace line, only when `pace.runs_out_at` exists: "Heads up. At this
/// pace you'll run out tomorrow, before the week resets Sun 7:19 PM."
func usagePaceLine(_ limits: CloudUsageLimits?, now: Date = Date(), calendar: Calendar = .current, locale: Locale = .current) -> String? {
    guard let pace = limits?.pace, let out = pace.runsOutDate else { return nil }
    let when: String
    if calendar.isDate(out, inSameDayAs: now) {
        when = "today"
    } else if let tomorrow = calendar.date(byAdding: .day, value: 1, to: now), calendar.isDate(out, inSameDayAs: tomorrow) {
        when = "tomorrow"
    } else {
        when = usageResetLabel(out, now: now, calendar: calendar, locale: locale)
    }
    let window = limits?.windows.first { $0.id == pace.windowId }
    let noun: String
    switch pace.windowId {
    case "5h": noun = "session"
    case "7d": noun = "week"
    default: noun = "month"
    }
    if let reset = window?.resetDate, reset > out {
        return "Heads up. At this pace you'll run out \(when), before the \(noun) resets \(usageResetLabel(reset, now: now, calendar: calendar, locale: locale))."
    }
    return "Heads up. At this pace you'll run out \(when)."
}
