import XCTest
@testable import Allternit

/// Settings → Usage selectors (phone calm I15): percent, thresholds, reset
/// text, window titles and order, the Cloud window, and Buy credits routing.
final class UsageMathTests: XCTestCase {
    private var utc: Calendar = {
        var c = Calendar(identifier: .gregorian)
        c.timeZone = TimeZone(identifier: "UTC")!
        return c
    }()
    private let enUS = Locale(identifier: "en_US")

    private func date(_ y: Int, _ mo: Int, _ d: Int, _ h: Int = 0, _ mi: Int = 0, _ s: Int = 0) -> Date {
        utc.date(from: DateComponents(year: y, month: mo, day: d, hour: h, minute: mi, second: s))!
    }

    /// ICU puts a narrow no-break space before AM/PM on newer systems.
    private func norm(_ text: String?) -> String? {
        text?.replacingOccurrences(of: "\u{202F}", with: " ")
    }

    private func window(_ id: String, _ label: String, _ ratio: Double, reset: Date? = nil) -> ProviderLimitWindow {
        ProviderLimitWindow(id: id, label: label, usedRatio: ratio, resetAt: reset.map { ISO8601DateFormatter().string(from: $0) })
    }

    // MARK: Thresholds

    func testFooterTextComesFromTheThresholds() {
        XCTAssertEqual(UsageThresholds.warnAt, 0.8)
        XCTAssertEqual(UsageThresholds.landAt, 0.95)
        XCTAssertEqual(UsageThresholds.footerText, "Heads-up at 80%. Tasks wrap up at 95%.")
    }

    func testLevelBoundaries() {
        XCTAssertEqual(usageLevel(0.79), .ok)
        XCTAssertEqual(usageLevel(0.8), .approaching)
        XCTAssertEqual(usageLevel(0.949), .approaching)
        XCTAssertEqual(usageLevel(0.95), .reached)
        XCTAssertEqual(usageLevel(1), .reached)
    }

    // MARK: Percent

    func testPercentIsTheWorstWindowRoundedAndClamped() {
        XCTAssertNil(usagePercent([]))
        XCTAssertEqual(usagePercent([window("5h", "5-hour", 0.42), window("7d", "Weekly", 0.834)]), 83)
        XCTAssertEqual(usagePercent([window("month", "Monthly", 1.2)]), 100)
        XCTAssertEqual(usagePercent([window("month", "Monthly", -0.1)]), 0)
    }

    func testWorstWindowTieGoesToTheOneResettingLast() {
        let soon = window("5h", "5-hour", 0.5, reset: date(2026, 11, 17, 12))
        let later = window("7d", "Weekly", 0.5, reset: date(2026, 11, 20, 9))
        XCTAssertEqual(worstUsageWindow([soon, later])?.id, "7d")
        XCTAssertEqual(worstUsageWindow([later, soon])?.id, "7d")
    }

    // MARK: Titles and order

    func testWindowTitles() {
        XCTAssertEqual(usageWindowTitle(window("5h", "5-hour", 0)), "This 5-hour window")
        XCTAssertEqual(usageWindowTitle(window("7d", "Weekly", 0)), "This week")
        XCTAssertEqual(usageWindowTitle(window("7d-opus", "Weekly (Opus)", 0)), "This week · Opus")
        XCTAssertEqual(usageWindowTitle(window("month", "Monthly", 0)), "This month")
        XCTAssertEqual(usageWindowTitle(window("1d", "Daily", 0)), "Daily")
    }

    func testSortIsShortestFirstAndStableWithinAKind() {
        let sorted = sortUsageWindows([
            window("month", "Monthly", 0.1),
            window("7d-opus", "Weekly (Opus)", 0.9),
            window("5h", "5-hour", 0.2),
            window("7d", "Weekly", 0.3),
            window("x", "Other", 0.4),
        ])
        XCTAssertEqual(sorted.map(\.id), ["5h", "7d-opus", "7d", "month", "x"])
    }

    // MARK: Reset text

    func testResetTextUnderSixHoursCountsDown() {
        let now = date(2026, 11, 17, 8)
        XCTAssertNil(usageResetsText(nil, now: now, calendar: utc, locale: enUS))
        XCTAssertEqual(usageResetsText(now.addingTimeInterval(30), now: now, calendar: utc, locale: enUS), "Resets in a moment")
        XCTAssertEqual(usageResetsText(now.addingTimeInterval(45 * 60), now: now, calendar: utc, locale: enUS), "Resets in 45m")
        XCTAssertEqual(usageResetsText(now.addingTimeInterval(2 * 3_600 + 10 * 60), now: now, calendar: utc, locale: enUS), "Resets in 2h 10m")
        XCTAssertEqual(usageResetsText(now.addingTimeInterval(3 * 3_600), now: now, calendar: utc, locale: enUS), "Resets in 3h")
        // Rounded minutes first: never "1h 60m".
        XCTAssertEqual(usageResetsText(now.addingTimeInterval(3_600 + 59 * 60 + 45), now: now, calendar: utc, locale: enUS), "Resets in 2h")
    }

    func testResetTextLaterTodayThisWeekAndFurtherOut() {
        let now = date(2026, 11, 17, 8) // Tuesday
        XCTAssertEqual(norm(usageResetsText(date(2026, 11, 17, 20, 30), now: now, calendar: utc, locale: enUS)), "Resets at 8:30 PM")
        XCTAssertEqual(norm(usageResetsText(date(2026, 11, 19, 9), now: now, calendar: utc, locale: enUS)), "Resets Thu 9:00 AM")
        XCTAssertEqual(norm(usageResetsText(date(2026, 12, 1), now: now, calendar: utc, locale: enUS)), "Resets Dec 1, 12:00 AM")
    }

    // MARK: Allternit Cloud

    func testCloudReportsOneMonthlyWindowOnly() {
        XCTAssertEqual(cloudUsageWindows(nil), [])
        let unmetered = UsageSnapshot(plan: "free", weeklyUsed: 0, weeklyLimit: 0, resetsAt: nil, credits: nil)
        XCTAssertEqual(cloudUsageWindows(unmetered), [])

        let plus = UsageSnapshot(plan: "plus", weeklyUsed: 7.48, weeklyLimit: 22, resetsAt: "2026-12-01T00:00:00Z", credits: 18.4)
        let windows = cloudUsageWindows(plus)
        XCTAssertEqual(windows.map(\.id), ["month"])
        XCTAssertEqual(windows.first?.usedRatio ?? 0, 0.34, accuracy: 0.0001)
        XCTAssertEqual(windows.first?.resetDate, date(2026, 12, 1))
        XCTAssertEqual(usageWindowTitle(windows[0]), "This month")
        XCTAssertFalse(windows.contains { $0.id.hasPrefix("7d") })

        let over = UsageSnapshot(plan: "plus", weeklyUsed: 30, weeklyLimit: 22, resetsAt: nil, credits: nil)
        XCTAssertEqual(cloudUsageWindows(over).first?.usedRatio, 1)
    }

    /// The Settings `usage` row and the page's top bar read one selector.
    func testSettingsRowPercentMatchesThePage() {
        let plus = UsageSnapshot(plan: "plus", weeklyUsed: 7.48, weeklyLimit: 22, resetsAt: nil, credits: nil)
        let pageBar = cloudUsageWindows(plus).first.map { Int(($0.usedRatio * 100).rounded()) }
        XCTAssertEqual(usagePercent(cloudUsageWindows(plus)), 34)
        XCTAssertEqual(usagePercent(cloudUsageWindows(plus)), pageBar)
    }

    // MARK: Cloud computer hours

    func testHoursText() {
        XCTAssertEqual(usageHoursText(usedSeconds: 45_000, maxHours: 40), "12.5 h of 40 h")
        XCTAssertEqual(usageHoursText(usedSeconds: 36_000, maxHours: 40), "10 h of 40 h")
        XCTAssertEqual(usageHoursText(usedSeconds: 0, maxHours: 8), "0 h of 8 h")
    }

    func testNextMonthStartRollsTheYear() {
        XCTAssertEqual(usageNextMonthStart(date(2026, 12, 15, 10), calendar: utc), date(2027, 1, 1))
        XCTAssertEqual(usageNextMonthStart(date(2026, 11, 17), calendar: utc), date(2026, 12, 1))
    }

    // MARK: Buy credits

    func testBuyCreditsFollowsTheExistingPurchaseGates() {
        XCTAssertEqual(BuyCreditsRoute.resolve(storeSellsPacks: true, allowsPurchaseLinks: true), .storeSheet)
        XCTAssertEqual(BuyCreditsRoute.resolve(storeSellsPacks: true, allowsPurchaseLinks: false), .storeSheet)
        XCTAssertEqual(BuyCreditsRoute.resolve(storeSellsPacks: false, allowsPurchaseLinks: true), .web)
        XCTAssertEqual(BuyCreditsRoute.resolve(storeSellsPacks: false, allowsPurchaseLinks: false), .hidden)
    }
}
