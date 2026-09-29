import SwiftUI

/// Wire shape of `GET /api/v1/me/usage`
/// (cmd/allternit-api/src/me_routes.rs get_my_usage). `resetsAt` stays a
/// string on the wire — APIClient decodes with a plain JSONDecoder (no date
/// strategy), so parsing happens in `resetsAtDate`.
struct UsageSnapshot: Decodable, Equatable, Sendable {
    /// Plan tier id from the entitlements service ("free", "pro", …).
    let plan: String
    /// Usage consumed in the current window (backend-defined units). Named
    /// "weekly" on the wire; Allternit Cloud's window is monthly — see
    /// `UsageStore.periodLabel`.
    let weeklyUsed: Double
    /// Weekly window cap; 0 means "no metering window" and disables all
    /// percent-derived UI.
    let weeklyLimit: Double
    /// ISO-8601 timestamp the weekly window resets at; nil when the backend
    /// doesn't report one.
    let resetsAt: String?
    /// Pay-as-you-go credit balance; nil when the deployment has no credits.
    let credits: Double?
}

/// App-wide usage meter (Phase 5), fed by `GET /api/v1/me/usage`. Fetched
/// once on launch (MainWorkspaceView) and refreshed after every completed
/// stream (ChatViewModel.finishStreaming), since streaming consumes quota.
///
/// When metering isn't configured, the route answers 503 with a named JSON
/// state (`{"error":"usage_metering_unavailable","message":…}`,
/// me_routes.rs) — the store parks in `.unavailable` carrying the backend's
/// message, which the UI shows as plain text. The store NEVER invents
/// numbers.
@MainActor
final class UsageStore: ObservableObject {
    static let shared = UsageStore()

    enum Availability: Equatable {
        /// Nothing fetched yet this launch.
        case unknown
        case available
        /// The backend reports metering as not configured (or the fetch
        /// failed) — carries the backend's message when it sent one.
        case unavailable(message: String)
    }

    @Published private(set) var snapshot: UsageSnapshot? = nil
    @Published private(set) var availability: Availability = .unknown
    @Published private(set) var isLoading = false

    private let defaults: UserDefaults
    private var fetchTask: Task<Void, Never>? = nil
    private var hasFetchedThisLaunch = false

    #if DEBUG
    /// `-usage-percent <n>` fixture (DEBUG only) — see init.
    private let fixtureSnapshot: UsageSnapshot?
    #endif

    private enum Keys {
        /// yyyy-MM-dd stamp of the day the usage banner was dismissed;
        /// dismissal persists for the day, not forever.
        static let bannerDismissedOn = "allternit-usage-banner-dismissed-on"
    }

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        #if DEBUG
        // `-usage-percent <n>` (DEBUG only): force a fixture snapshot so the
        // ≥80% banner, the 100% wall, and the settings Usage section can be
        // screenshot-verified without a metered backend. This is a test
        // fixture, NOT a graceful degrade — it overrides the network fetch
        // entirely and never ships in release builds.
        if let raw = defaults.string(forKey: "usage-percent"),
           let percent = Double(raw) {
            let limit = 100.0
            fixtureSnapshot = UsageSnapshot(
                plan: "Free",
                weeklyUsed: limit * percent / 100.0,
                weeklyLimit: limit,
                resetsAt: ISO8601DateFormatter().string(from: Self.debugResetsAt),
                credits: 0
            )
        } else {
            fixtureSnapshot = nil
        }
        #endif
    }

    #if DEBUG
    /// Fixture reset point: next Monday 09:00 local.
    private static var debugResetsAt: Date {
        Calendar.current.nextDate(
            after: Date(),
            matching: DateComponents(hour: 9, weekday: 2),
            matchingPolicy: .nextTime
        ) ?? Date().addingTimeInterval(3 * 86_400)
    }
    #endif

    // MARK: - Fetching

    /// Loads the snapshot once per launch unless forced; concurrent callers
    /// share the in-flight request (same idiom as ModelStore's catalog
    /// fetch).
    func fetchUsageIfNeeded(force: Bool = false) {
        #if DEBUG
        if let fixtureSnapshot {
            snapshot = fixtureSnapshot
            availability = .available
            return
        }
        #endif
        guard force || !hasFetchedThisLaunch, fetchTask == nil else { return }
        hasFetchedThisLaunch = true
        isLoading = true
        fetchTask = Task { [weak self] in
            guard let self else { return }
            defer {
                self.isLoading = false
                self.fetchTask = nil
            }
            do {
                let fetched: UsageSnapshot = try await APIClient.shared.get(path: "me/usage")
                self.snapshot = fetched
                self.availability = .available
            } catch is CancellationError {
                // View went away mid-flight — keep current state.
                return
            } catch let APIError.httpError(_, message) {
                // The backend's named state (e.g. 503
                // `usage_metering_unavailable`) arrives with its message.
                self.snapshot = nil
                self.availability = .unavailable(
                    message: message ?? "Usage metering isn't available on this backend."
                )
            } catch {
                self.snapshot = nil
                self.availability = .unavailable(message: error.localizedDescription)
            }
        }
    }

    /// Refetch after a completed stream — streaming consumes quota.
    func refresh() {
        fetchUsageIfNeeded(force: true)
    }

    // MARK: - Derived state

    /// Percent of the metering window consumed (0…∞); nil when unmetered.
    var percentUsed: Double? {
        guard let snapshot, snapshot.weeklyLimit > 0 else { return nil }
        return snapshot.weeklyUsed / snapshot.weeklyLimit * 100
    }

    /// Rounded percent for copy ("94%").
    var percentText: String? {
        percentUsed.map { "\(Int($0.rounded()))%" }
    }

    /// Hard wall: at/above 100% the composer locks.
    var isAtLimit: Bool {
        (percentUsed ?? 0) >= 100
    }

    /// The ≥80% nudge banner — hidden at/above the wall (the wall card
    /// carries that state) and for the rest of the day once dismissed.
    var shouldShowBanner: Bool {
        guard let percent = percentUsed, percent >= 80, !isAtLimit else { return false }
        return !isBannerDismissedToday
    }

    /// "Monday 9:00 AM" within the week, "Oct 1" further out (monthly plans).
    var resetsLabel: String? {
        guard let date = resetsAtDate else { return nil }
        let formatter = DateFormatter()
        formatter.dateFormat = date.timeIntervalSinceNow > 6 * 86_400 ? "MMM d" : "EEEE h:mm a"
        return formatter.string(from: date)
    }

    /// Section title for the meter. The wire fields are named `weekly*`, but
    /// Allternit Cloud fills them with the plan's monthly grant and resets on
    /// the 1st (cloud me_usage.rs compose_usage) — so name the period from
    /// when it resets, not from the field name.
    var periodLabel: String {
        guard let date = resetsAtDate else { return "Usage" }
        var utc = Calendar(identifier: .gregorian)
        utc.timeZone = TimeZone(identifier: "UTC") ?? .current
        let parts = utc.dateComponents([.day, .hour, .minute], from: date)
        if parts.day == 1 && parts.hour == 0 && parts.minute == 0 { return "Monthly usage" }
        return date.timeIntervalSinceNow <= 7 * 86_400 ? "Weekly usage" : "Usage"
    }

    /// Parsed `resetsAt` (ISO-8601, with or without fractional seconds).
    var resetsAtDate: Date? {
        guard let raw = snapshot?.resetsAt else { return nil }
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = formatter.date(from: raw) { return date }
        formatter.formatOptions = [.withInternetDateTime]
        return formatter.date(from: raw)
    }

    /// Display plan name for the account capsule ("Free" / "Pro").
    var planLabel: String? {
        snapshot?.plan.capitalized
    }

    // MARK: - Banner dismissal (per-day)

    private static var todayStamp: String {
        let formatter = DateFormatter()
        formatter.dateFormat = "yyyy-MM-dd"
        return formatter.string(from: Date())
    }

    private var isBannerDismissedToday: Bool {
        defaults.string(forKey: Keys.bannerDismissedOn) == Self.todayStamp
    }

    /// Dismiss the banner until tomorrow (UserDefaults-backed; sends
    /// objectWillChange so the feed re-renders immediately).
    func dismissBannerForToday() {
        defaults.set(Self.todayStamp, forKey: Keys.bannerDismissedOn)
        objectWillChange.send()
    }
}
