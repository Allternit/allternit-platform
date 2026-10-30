import SwiftUI

/// Settings → Billing (and Usage): plan, usage bars and credits from the real
/// routes (`/api/v1/me/usage`, `/api/v1/hosted-runtimes/entitlement`,
/// `/api/v1/billing/credits`, `/api/v1/billing/subscription`).
///
/// Purchases happen on the web. The "Get Pro" / "Buy credits" links show on
/// the US App Store storefront only (StorefrontGate); elsewhere the page
/// shows plan status with no link and no price.
struct ComputeBillingView: View {
    @ObservedObject private var usageStore = UsageStore.shared
    @ObservedObject private var plan = AccountPlanStore.shared
    @ObservedObject private var storefront = StorefrontGate.shared
    var title: String = "Billing"

    var body: some View {
        List {
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

            Section {
                PlanUsageRows()
            } header: {
                Text("Usage")
            }

            if storefront.allowsPurchaseLinks {
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
