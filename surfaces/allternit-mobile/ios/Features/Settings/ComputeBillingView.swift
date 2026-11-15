import SwiftUI

/// Settings → Billing (and Usage): plan, usage bars and credits from the real
/// routes (`/api/v1/me/usage`, `/api/v1/hosted-runtimes/entitlement`,
/// `/api/v1/billing/credits`, `/api/v1/billing/subscription`).
///
/// Purchases happen on the web. The "Choose a plan" / "Buy credits" links show on
/// the US App Store storefront only (StorefrontGate); elsewhere the page
/// shows plan status with no link and no price.
struct ComputeBillingView: View {
    @ObservedObject private var usageStore = UsageStore.shared
    @ObservedObject private var plan = AccountPlanStore.shared
    @ObservedObject private var storefront = StorefrontGate.shared
    var title: String = "Billing"
    @State private var plans: [CatalogPlan] = []

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
    private struct PlansWire: Decodable { let plans: [CatalogPlan] }

    private var memberName: String {
        let first = AuthManager.shared.currentSession?.user?.firstName?.trimmingCharacters(in: .whitespaces) ?? ""
        return first.isEmpty ? "Member" : first
    }

    var body: some View {
        List {
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
            } header: {
                Text("Plan")
            }

            if title == "Usage" {
                Section {
                    AccountUsageCardsView()
                        .listRowBackground(Color.clear)
                        .listRowInsets(EdgeInsets(top: 4, leading: 0, bottom: 4, trailing: 0))
                } header: {
                    Text("Accounts")
                } footer: {
                    Text("Heads-up at 80% · tasks wrap up at 95%")
                }
            }

            Section {
                PlanUsageRows()
            } header: {
                Text("Usage")
            }

            // Plan tickets link out to checkout on the web (US storefront only).
            if storefront.allowsPurchaseLinks && title != "Usage" && !plans.isEmpty {
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
                            ],
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

            if storefront.allowsPurchaseLinks && title != "Usage" {
                Section {
                    PayLinkRows()
                } footer: {
                    Text("Opens allternit.com in your browser.")
                }
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .navigationTitle(title)
        .navigationBarTitleDisplayMode(.inline)
        .task {
            if let wire: PlansWire = try? await APIClient.shared.get(path: "billing/plans") { plans = wire.plans }
            UsageStore.shared.fetchUsageIfNeeded(force: true)
            AccountPlanStore.shared.load(force: true)
            StorefrontGate.shared.refresh()
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
