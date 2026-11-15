import Foundation
import UIKit

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

/// `cloud_models` on `GET /api/v1/billing/credits` and `/me/usage`
/// (CONTRACT-cloud-credits-2026-11-15): may this account run Allternit Cloud
/// models. Every field but `allowed` is optional on the wire.
struct CloudModelsEntitlement: Decodable, Equatable, Sendable {
    let allowed: Bool
    /// nil when allowed, else `credits_required` or `credits_exhausted`.
    let reason: String?
    let balanceUsd: Double?
    /// Catalog provider ids the account has its own key for: rows served by
    /// them are never locked (D5).
    let byokProviders: [String]

    init(allowed: Bool, reason: String? = nil, balanceUsd: Double? = nil, byokProviders: [String] = []) {
        self.allowed = allowed
        self.reason = reason
        self.balanceUsd = balanceUsd
        self.byokProviders = byokProviders
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        allowed = try c.decodeIfPresent(Bool.self, forKey: .allowed) ?? true
        reason = try c.decodeIfPresent(String.self, forKey: .reason)
        balanceUsd = try c.decodeIfPresent(Double.self, forKey: .balanceUsd)
        byokProviders = try c.decodeIfPresent([String].self, forKey: .byokProviders) ?? []
    }

    enum CodingKeys: String, CodingKey {
        case allowed, reason
        case balanceUsd = "balance_usd"
        case byokProviders = "byok_providers"
    }

    /// Whether a Cloud row served by `provider` is locked. An absent
    /// entitlement (older backend) means not locked (D10). A row whose
    /// provider is not known yet stays locked while the account is not allowed.
    static func isLocked(_ entitlement: CloudModelsEntitlement?, provider: String?) -> Bool {
        guard let entitlement, !entitlement.allowed else { return false }
        if let provider, entitlement.byokProviders.contains(provider) { return false }
        return true
    }
}

/// `GET /api/v1/billing/credits` (cloud-api billing_credits.rs, snake_case).
struct BillingCredits: Decodable, Equatable, Sendable {
    let balanceUsd: Double
    let monthToDateUsageUsd: Double
    /// Absent on a backend from before credits were required (D10).
    var cloudModels: CloudModelsEntitlement? = nil

    enum CodingKeys: String, CodingKey {
        case balanceUsd = "balance_usd"
        case monthToDateUsageUsd = "month_to_date_usage_usd"
        case cloudModels = "cloud_models"
    }
}

/// `GET /api/v1/billing/subscription` (cloud-api billing_subscriptions.rs).
/// `status` is "none" with plan "free" when there's no paid plan.
struct BillingSubscription: Decodable, Equatable, Sendable {
    let planId: String
    let label: String
    let planTier: String
    let status: String
    /// When the plan renews (RFC 3339), once Stripe has reported it.
    var renewsAt: String? = nil

    enum CodingKeys: String, CodingKey {
        case planId = "plan_id"
        case label
        case planTier = "plan_tier"
        case status
        case renewsAt = "renews_at"
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
    private var foregroundObserver: NSObjectProtocol? = nil

    private init() {
        // Credits bought on the web land while the app is in the background:
        // re-read them on return so a locked Cloud model unlocks.
        foregroundObserver = NotificationCenter.default.addObserver(
            forName: UIApplication.willEnterForegroundNotification, object: nil, queue: .main
        ) { _ in
            Task { @MainActor in
                let store = AccountPlanStore.shared
                guard store.entitlement != nil || store.credits != nil || store.subscription != nil else { return }
                store.load(force: true)
            }
        }
    }

    /// Allternit Cloud entitlement; nil when unknown (not locked, D10).
    var cloudModels: CloudModelsEntitlement? { credits?.cloudModels }

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

    /// "Choose a plan" target: the backend's upgradeUrl, else allternit.com/pricing.
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

    #if DEBUG
    private var isFixture = false
    /// Debug gallery fixtures; network loads stop replacing them.
    func setFixture(credits: BillingCredits) {
        isFixture = true
        self.credits = credits
    }
    #endif

    func load(force: Bool = false) {
        #if DEBUG
        if isFixture { return }
        #endif
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
            CloudCreditsGate.entitlementChanged(credits?.cloudModels)
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
