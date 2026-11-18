import Foundation

/// Settings → Usage answers for `-calm-usage -sample-workspace` (DEBUG
/// screenshots, phone calm I15), in the wire shape each store decodes:
/// `me/usage` (cloud-api me_usage.rs, camelCase), `hosted-runtimes/entitlement`
/// (camelCase), `billing/credits` and `billing/subscription` (snake_case),
/// `providers/:id/quota` (gizzi quota relay). Only with `-calm-usage`, so
/// other sample screenshots keep their unmetered look.
enum SampleUsage {
    /// True for the launch when both `-sample-workspace` and `-calm-usage`
    /// are passed to a DEBUG build. Always false in Release.
    static let isEnabled: Bool = {
        #if DEBUG
        SampleWorkspace.isEnabled && launchArgumentEnabled(LaunchArgument.calmUsage)
        #else
        false
        #endif
    }()

    /// Connected accounts the sample page lists (the sample model catalog
    /// has Allternit Cloud only).
    static var accountProviderIds: [String] { isEnabled ? ["plan-claude", "plan-chatgpt"] : [] }

    /// A read the sample answers, or nil to fall through.
    static func read(parts: [String], now: Date = Date()) -> Any? {
        guard isEnabled else { return nil }
        let iso = ISO8601DateFormatter()
        var utc = Calendar(identifier: .gregorian)
        utc.timeZone = TimeZone(identifier: "UTC") ?? .current
        switch parts {
        case ["me", "usage"]:
            // Plus: $22 monthly grant, $7.48 used (34%), resets on the 1st
            // at 00:00 UTC like cloud-api compose_usage.
            return [
                "plan": "plus", "label": "Plus", "status": "active", "planTier": "plus",
                "weeklyUsed": 7.48, "weeklyLimit": 22.0,
                "resetsAt": iso.string(from: usageNextMonthStart(now, calendar: utc)),
                "credits": 18.4, "monthToDateUsageUsd": 3.75, "recentTransactions": [Any](),
                "cloud_models": ["allowed": true, "byok_providers": [String]()],
            ] as [String: Any]
        case ["hosted-runtimes", "entitlement"]:
            return [
                "planTierId": "plus", "planDisplayName": "Plus",
                "maxHoursMonthly": 40, "usedSecondsMonthly": 45_000, "remainingSecondsMonthly": 99_000,
                "upgradeUrl": NSNull(), "billingPortalUrl": NSNull(),
            ] as [String: Any]
        case ["billing", "credits"]:
            return [
                "balance_usd": 18.4, "month_to_date_usage_usd": 3.75,
                "cloud_models": ["allowed": true, "byok_providers": [String]()],
            ] as [String: Any]
        case ["billing", "subscription"]:
            return [
                "plan_id": "plus", "label": "Plus", "plan_tier": "plus", "status": "active",
                "renews_at": iso.string(from: usageNextMonthStart(now, calendar: utc)),
                "billing_provider": "apple",
            ] as [String: Any]
        case ["providers", "plan-claude", "quota"]:
            // 5-hour at 42% (resets in 2h 10m), weekly at 83% (past the
            // heads-up threshold), weekly Opus at 97% (past wrap-up).
            return quota(plan: "Max", windows: [
                ("5h", "5-hour", 0.42, now.addingTimeInterval(2 * 3_600 + 10 * 60)),
                ("7d", "Weekly", 0.83, now.addingTimeInterval(3 * 86_400 + 5 * 3_600)),
                ("7d-opus", "Weekly (Opus)", 0.97, now.addingTimeInterval(3 * 86_400 + 5 * 3_600)),
            ], iso: iso)
        case ["providers", "plan-chatgpt", "quota"]:
            return quota(plan: "Plus", windows: [
                ("5h", "5-hour", 0.18, now.addingTimeInterval(4 * 3_600 + 40 * 60)),
                ("7d", "Weekly", 0.31, now.addingTimeInterval(5 * 86_400)),
            ], iso: iso)
        default:
            return nil
        }
    }

    private static func quota(plan: String, windows: [(String, String, Double, Date)], iso: ISO8601DateFormatter) -> [String: Any] {
        [
            "status": "ok",
            "quota": [
                "plan": plan,
                "windows": windows.map { id, label, ratio, reset in
                    ["id": id, "label": label, "usedRatio": ratio, "resetAt": iso.string(from: reset)] as [String: Any]
                },
            ] as [String: Any],
        ]
    }
}
