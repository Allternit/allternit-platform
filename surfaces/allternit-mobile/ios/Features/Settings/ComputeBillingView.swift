import SwiftUI

/// Settings → Billing: plan, usage bars and credits from the real
/// routes (`/api/v1/me/usage`, `/api/v1/hosted-runtimes/entitlement`,
/// `/api/v1/billing/credits`, `/api/v1/billing/subscription`).
///
/// Where the server has Apple switched on and StoreKit returns the products,
/// plans and credit packs are bought here with In-App Purchase on every
/// storefront (StorePurchaseManager), priced by the App Store, with Restore
/// Purchases and the subscription terms. The web "Choose a plan" / "Buy
/// credits" links show on the US App Store storefront only (StorefrontGate),
/// next to the store buttons. Elsewhere, without store buttons, the page
/// shows plan status with no link and no price.
///
/// `title: "Usage"` opens Settings → Usage instead (`UsagePageView`, phone
/// calm I15), so every caller keeps its route.
struct ComputeBillingView: View {
    @ObservedObject private var usageStore = UsageStore.shared
    @ObservedObject private var plan = AccountPlanStore.shared
    @ObservedObject private var storefront = StorefrontGate.shared
    @ObservedObject private var store = StorePurchaseManager.shared
    var title: String = "Billing"
    @State private var plans: [CatalogPlan] = []
    @State private var freeweightsPerks: FreeweightsPerks?

    /// `GET /api/v1/billing/plans` (Plus, Max, Ultra; Max keeps id `super`).
    struct CatalogPlan: Decodable, Identifiable {
        let id: String
        let label: String
        let priceUsd: Double
        let monthlyCreditsUsd: Double
        let rolloverCapUsd: Double
        enum CodingKeys: String, CodingKey {
            case id, label
            case priceUsd = "price_usd"
            case monthlyCreditsUsd = "monthly_credits_usd"
            case rolloverCapUsd = "rollover_cap_usd"
        }
    }
    /// One Freeweights perk line from `/billing/plans` `.freeweights.plans`.
    struct FreeweightsPerk: Decodable, Equatable {
        let key: String
        let label: String
        let value: Double?
        let unit: String?
    }
    /// The `freeweights` block: perks are listed per plan id, and shown only
    /// while `status` is `offered`.
    struct FreeweightsPerks: Decodable, Equatable {
        let status: String
        let plans: [String: [FreeweightsPerk]]?

        var isOffered: Bool { status == "offered" }

        /// Lines for a plan, empty while the perks are not offered.
        func lines(for planId: String) -> [String] {
            guard isOffered else { return [] }
            return (plans?[planId] ?? []).map { perk in
                guard let value = perk.value else { return perk.label }
                let number = value == value.rounded() ? String(Int(value)) : String(value)
                return perk.unit.map { "\(perk.label): \(number) \($0)" } ?? "\(perk.label): \(number)"
            }
        }
    }
    private struct PlansWire: Decodable {
        let plans: [CatalogPlan]
        let freeweights: FreeweightsPerks?
    }

    private var memberName: String {
        let first = AuthManager.shared.currentSession?.user?.firstName?.trimmingCharacters(in: .whitespaces) ?? ""
        return first.isEmpty ? "Member" : first
    }

    var body: some View {
        if title == "Usage" {
            // Settings → Usage (phone calm I15): its own page; the billing
            // list below is Settings → Billing.
            UsagePageView()
        } else {
            billingList
        }
    }

    private var billingList: some View {
        // Dot title that docks into the bar on scroll (UI feedback I12).
        DockingTitleList(title: title) {
            if title != "Usage" {
                Section {
                    MemberCardView(
                        planId: plan.subscription?.planId ?? "free",
                        member: memberName,
                        renewsAt: plan.subscription?.isPaid == true ? plan.subscription?.renewsAt : nil,
                        status: plan.subscription?.status,
                        width: 320
                    )
                    .frame(maxWidth: .infinity)
                    .listRowBackground(Color.clear)
                    .listRowInsets(EdgeInsets(top: 8, leading: 0, bottom: 8, trailing: 0))
                }
            }
            Section {
                HStack {
                    Text("Current plan")
                        .font(.subheadline)
                        .foregroundColor(Color("TextPrimary"))
                    Spacer()
                    if plan.isLoading && plan.planName == nil {
                        ProgressView()
                    } else {
                        Text(plan.planName ?? usageStore.planLabel ?? "Free")
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundColor(Color("TextSecondary"))
                            .padding(.horizontal, 10)
                            .padding(.vertical, 4)
                            .background(Color("BgSecondary"))
                            .clipShape(Capsule())
                    }
                }
                if let status = plan.subscription?.status, plan.subscription?.isPaid == true, status != "active" {
                    Text(Self.statusCopy(status))
                        .font(.caption)
                        .foregroundColor(status == "trialing" ? Color("TextSecondary") : Theme.statusWarning)
                }
                if title != "Usage", plan.subscription?.billedByLine != nil {
                    StoreBilledByView(subscription: plan.subscription)
                }
            } header: {
                Text("Plan")
            }

            Section {
                PlanUsageRows()
            } header: {
                Text("Usage")
            }

            // In-App Purchase: plan tickets priced by the App Store, while no
            // plan is billed on the web or by Google (D5).
            if title != "Usage" && storefront.showsStoreButtons(for: .plan)
                && StorePurchaseManager.planPurchaseAllowed(plan.subscription) {
                Section {
                    ForEach(store.plans) { item in
                        StorePlanTicket(item: item, store: store)
                            .listRowBackground(Color.clear)
                            .listRowInsets(EdgeInsets(top: 5, leading: 0, bottom: 5, trailing: 0))
                    }
                } header: {
                    Text(plan.subscription?.isPaid == true ? "Change plan" : "Choose a plan")
                }
            } else if storefront.allowsPurchaseLinks && title != "Usage" && !plans.isEmpty {
                // Plan tickets link out to checkout on the web (US storefront only).
                Section {
                    ForEach(plans) { p in
                        let current = plan.subscription?.planId == p.id
                        PlanTicketView(
                            planId: p.id,
                            label: p.label,
                            price: String(format: "$%.0f", p.priceUsd),
                            lines: [
                                String(format: "$%.0f in Allternit Cloud credits each month", p.monthlyCreditsUsd),
                                String(format: "Unused credits roll over, up to $%.0f", p.rolloverCapUsd),
                            ] + (freeweightsPerks?.lines(for: p.id) ?? []),
                            current: current,
                            actionTitle: plan.subscription?.isPaid == true ? "Switch on the web" : "Choose",
                            action: { UIApplication.shared.open(plan.upgradeURL) }
                        )
                        .listRowBackground(Color.clear)
                        .listRowInsets(EdgeInsets(top: 5, leading: 0, bottom: 5, trailing: 0))
                    }
                } header: {
                    Text(plan.subscription?.isPaid == true ? "Change plan" : "Choose a plan")
                }
            }

            if title != "Usage" && storefront.showsStoreButtons(for: .pack) {
                Section {
                    StorePackGrid(store: store)
                        .listRowBackground(Color.clear)
                        .listRowInsets(EdgeInsets(top: 4, leading: 0, bottom: 4, trailing: 0))
                } header: {
                    Text("Buy credits")
                }
            }

            if title != "Usage" && storefront.showsStoreButtons {
                Section {
                    StorePurchaseNoticeView(store: store)
                    StoreRestoreButton(store: store)
                    StoreTermsView(store: store)
                }
                .listRowBackground(Color.clear)
            }

            if storefront.allowsPurchaseLinks && title != "Usage" {
                Section {
                    PayLinkRows()
                } footer: {
                    Text("Opens allternit.com in your browser.")
                }
            }
        }
        .navigationTitle(title)
        .navigationBarTitleDisplayMode(.inline)
        .task {
            if let wire: PlansWire = try? await APIClient.shared.get(path: "billing/plans") {
                plans = wire.plans
                freeweightsPerks = wire.freeweights
            }
            UsageStore.shared.fetchUsageIfNeeded(force: true)
            AccountPlanStore.shared.load(force: true)
            StorefrontGate.shared.refresh()
            if title != "Usage" {
                StorePurchaseManager.shared.notice = nil
                await StorePurchaseManager.shared.loadCatalog()
            }
        }
        .refreshable {
            UsageStore.shared.fetchUsageIfNeeded(force: true)
            AccountPlanStore.shared.load(force: true)
        }
    }

    private static func statusCopy(_ status: String) -> String {
        switch status {
        case "trialing": return "Free trial"
        case "past_due": return "Payment past due."
        case "unpaid": return "Payment failed. Your plan is paused."
        default: return status.capitalized
        }
    }
}
