import Foundation

/// `GET /api/v1/hosted-runtimes/entitlement` (cloud-api hosted_runtimes.rs,
/// camelCase on the wire): the plan name and the cloud computer hours.
struct HostedEntitlement: Decodable, Equatable, Sendable {
    let planTierId: String
    let planDisplayName: String
    let maxHoursMonthly: Int
    let usedSecondsMonthly: Int
    let remainingSecondsMonthly: Int
    let upgradeUrl: String?
    let billingPortalUrl: String?
}

/// `GET /api/v1/billing/credits` (cloud-api billing_credits.rs, snake_case).
struct BillingCredits: Decodable, Equatable, Sendable {
    struct FreeInference: Decodable, Equatable, Sendable {
        let monthlyAllowanceUsd: Double
        let usedUsd: Double
        let remainingUsd: Double

        enum CodingKeys: String, CodingKey {
            case monthlyAllowanceUsd = "monthly_allowance_usd"
            case usedUsd = "used_usd"
            case remainingUsd = "remaining_usd"
        }
    }

    let balanceUsd: Double
    let monthToDateUsageUsd: Double
    let freeInference: FreeInference?

    enum CodingKeys: String, CodingKey {
        case balanceUsd = "balance_usd"
        case monthToDateUsageUsd = "month_to_date_usage_usd"
        case freeInference = "free_inference"
    }
}

/// `GET /api/v1/billing/subscription` (cloud-api billing_subscriptions.rs).
/// `status` is "none" with plan "free" when there's no paid plan.
struct BillingSubscription: Decodable, Equatable, Sendable {
    let planId: String
    let label: String
    let planTier: String
    let status: String

    enum CodingKeys: String, CodingKey {
        case planId = "plan_id"
        case label
        case planTier = "plan_tier"
        case status
    }

    /// A paid plan that still bills (or is trying to).
    var isPaid: Bool {
        planId != "free" && ["active", "trialing", "past_due", "unpaid"].contains(status)
    }
}

/// The account's plan, credits and cloud hours for Settings (the PWA's
/// useAllternitPlan + PlanUsageBars). Real sources only: each field stays nil
/// when its route fails (self-hosted, signed out, offline) and the UI omits
/// it instead of showing placeholder numbers. `/me/usage` stays in UsageStore.
@MainActor
final class AccountPlanStore: ObservableObject {
    static let shared = AccountPlanStore()

    @Published private(set) var entitlement: HostedEntitlement? = nil
    @Published private(set) var credits: BillingCredits? = nil
    @Published private(set) var subscription: BillingSubscription? = nil
    @Published private(set) var isLoading = false
    /// Set when every source failed, so the UI can say why.
    @Published private(set) var lastError: String? = nil

    private var loadTask: Task<Void, Never>? = nil

    private init() {}

    /// Plan display name: the subscription's label, else the entitlement's.
    var planName: String? {
        if let subscription, subscription.planId != "free" { return subscription.label }
        return entitlement?.planDisplayName ?? subscription?.label
    }

    /// Cloud computer hours used this month, 0…1; nil when the plan has none.
    var cloudHoursFraction: Double? {
        guard let entitlement, entitlement.maxHoursMonthly > 0 else { return nil }
        let used = Double(entitlement.usedSecondsMonthly) / 3600
        return used / Double(entitlement.maxHoursMonthly)
    }

    var cloudHoursDetail: String? {
        guard let entitlement, entitlement.maxHoursMonthly > 0 else { return nil }
        let used = Double(entitlement.usedSecondsMonthly) / 3600
        return String(format: "%.1f of %d h", used, entitlement.maxHoursMonthly)
    }

    /// "Get Pro" target: the backend's upgradeUrl, else allternit.com/pricing.
    var upgradeURL: URL {
        if let raw = entitlement?.upgradeUrl, !raw.isEmpty, let url = URL(string: raw) { return url }
        return StorefrontGate.upgradeURL
    }

    /// First day of next month, for "Resets Nov 1".
    static var monthlyResetLabel: String {
        let calendar = Calendar.current
        let now = Date()
        let start = calendar.date(from: calendar.dateComponents([.year, .month], from: now)) ?? now
        let next = calendar.date(byAdding: .month, value: 1, to: start) ?? now
        let formatter = DateFormatter()
        formatter.dateFormat = "MMM d"
        return formatter.string(from: next)
    }

    func load(force: Bool = false) {
        guard loadTask == nil else { return }
        if !force, entitlement != nil || credits != nil || subscription != nil { return }
        isLoading = true
        loadTask = Task { [weak self] in
            async let e: HostedEntitlement? = Self.fetch("hosted-runtimes/entitlement")
            async let c: BillingCredits? = Self.fetch("billing/credits")
            async let s: BillingSubscription? = Self.fetch("billing/subscription")
            let (entitlement, credits, subscription) = await (e, c, s)
            guard let self else { return }
            self.entitlement = entitlement
            self.credits = credits
            self.subscription = subscription
            self.lastError = (entitlement == nil && credits == nil && subscription == nil)
                ? "Plan details aren't available right now."
                : nil
            self.isLoading = false
            self.loadTask = nil
        }
    }

    /// One source; nil on any failure (the UI omits what's missing).
    private nonisolated static func fetch<T: Decodable & Sendable>(_ path: String) async -> T? {
        try? await APIClient.shared.get(path: path)
    }

    /// Drop everything (sign-out, account switch, delete).
    func reset() {
        loadTask?.cancel()
        loadTask = nil
        entitlement = nil
        credits = nil
        subscription = nil
        lastError = nil
        isLoading = false
    }
}
