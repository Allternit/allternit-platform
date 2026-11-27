import SwiftUI

/// Allternit Open on the phone (CONTRACT-open-tier-2026-11-26, D14): the copy,
/// the show/hide rule for open model rows, the Status row with the allowance
/// meter, and the Open Research ticket. Terms are accepted on the web only:
/// no terms screen and no store product here.
enum OpenModelsCopy {
    static let termsName = "Accept terms on the web"
    static let termsSub = "Open models need terms. Opens ai.allternit.com"
    static let termsURL = URL(string: "https://ai.allternit.com/settings/open-models")!
    static let contactURL = URL(string: "https://services.allternit.com")!

    static let planRequired = "Open models on Allternit Cloud need a paid plan. On Free, run open models on your own device with Freeweights."
    static let allowanceUsed = "This month's open-model allowance is used. Add credits to keep going."
    static let boughtRequired = "On Plus, open models run on credits you buy. Add credits to use them."
    static let termsRequired = "Accept the open-model terms on the web to use open models."
    static let researchRequired = "This model is part of Open Research, available by agreement."

    /// The contract §4 message for an `open_models.reason`; nil for no lock or
    /// for `terms_required` (that one is the web link row).
    static func message(forReason reason: String?) -> String? {
        switch reason {
        case "plan_required", "open_plan_required": return planRequired
        case "allowance_used": return allowanceUsed
        case "bought_credits_required": return boughtRequired
        default: return nil
        }
    }

    static func isOpenReason(_ reason: String?) -> Bool { message(forReason: reason) != nil }

    /// One "Open models" line per plan, on its plan ticket.
    static func planLine(forPlan planId: String) -> String {
        switch PlanTier(planId: planId) {
        case .free: return "Open models: on your device"
        case .plus: return "Open models: with credits you buy"
        case .max: return "Open models: up to $55/mo of plan credits"
        case .ultra: return "Open models: up to $110/mo of plan credits"
        }
    }

    static func money(_ usd: Double) -> String { String(format: "$%.2f", usd) }

    /// "Dec 26" from an RFC 3339 `period_end`.
    static func resetLabel(_ periodEnd: String?) -> String? {
        guard let periodEnd else { return nil }
        let f = ISO8601DateFormatter()
        guard let date = f.date(from: periodEnd) ?? ISO8601DateFormatter.fractional.date(from: periodEnd) else { return nil }
        return date.formatted(.dateTime.month(.abbreviated).day())
    }
}

private extension ISO8601DateFormatter {
    static let fractional: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f
    }()
}

enum OpenModelsGate {
    /// Open rows list only after terms are accepted on the web, and on the US
    /// storefront (App Store link-out rule). Absent entitlement = as today.
    static func listsOpenRows(_ e: OpenModelsEntitlement?, usStorefront: Bool) -> Bool {
        guard let e else { return false }
        return e.termsAccepted && usStorefront
    }

    /// The single "Accept terms on the web" row, in place of the open rows.
    /// US storefront only: elsewhere no open rows and no link (D12).
    static func showsTermsLink(_ e: OpenModelsEntitlement?, usStorefront: Bool) -> Bool {
        e != nil && usStorefront && e?.termsAccepted != true
    }

    /// Research rows only with the flag.
    static func listsResearchRows(_ e: OpenModelsEntitlement?) -> Bool { e?.research == true }
}

/// Settings → Billing → Status: the open-model allowance meter.
/// Shown only once the server reports `open_models` on a paid plan.
struct OpenModelsStatusRows: View {
    let entitlement: OpenModelsEntitlement

    private var hasPlanAllowance: Bool { entitlement.planAllowanceUsd > 0 }

    private var fraction: Double {
        guard hasPlanAllowance else { return 0 }
        return min(max(entitlement.usedUsd / entitlement.planAllowanceUsd, 0), 1)
    }

    private var detail: String {
        var parts: [String] = []
        if hasPlanAllowance {
            parts.append("Plan allowance")
            if let reset = OpenModelsCopy.resetLabel(entitlement.periodEnd) { parts.append("resets \(reset)") }
        }
        parts.append("bought credits \(OpenModelsCopy.money(entitlement.boughtBalanceUsd))")
        return parts.joined(separator: " · ")
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text("Open models").font(.subheadline).foregroundColor(Color("TextPrimary"))
                Spacer()
                if hasPlanAllowance {
                    Text("\(OpenModelsCopy.money(entitlement.usedUsd)) of \(OpenModelsCopy.money(entitlement.planAllowanceUsd))")
                        .font(.caption).foregroundColor(Color("TextSecondary"))
                }
            }
            if hasPlanAllowance {
                ProgressView(value: fraction)
                    .tint(Color("TextPrimary"))
                    .accessibilityLabel("Open model allowance used")
                    .accessibilityValue("\(Int(fraction * 100)) percent")
            }
            Text(detail).font(.caption).foregroundColor(Color("TextSecondary"))
            if let message = OpenModelsCopy.message(forReason: entitlement.reason) {
                Text(message).font(.caption).foregroundColor(Color("TextSecondary"))
            }
        }
        .padding(.vertical, 2)
        .accessibilityElement(children: .combine)
    }
}

/// Open Research: the fifth plan card, same ticket style, "By agreement".
struct OpenResearchTicketView: View {
    /// "Ask us" is a link-out: US storefront only.
    @ObservedObject private var storefront = StorefrontGate.shared

    var body: some View {
        PlanTicketView(
            planId: "ultra",
            label: "Open Research",
            price: "By agreement",
            lines: ["Larger models, higher rate limits",
                    "For verified security and red teams",
                    "Open models: all, on agreed credits"],
            current: false,
            actionTitle: storefront.allowsPurchaseLinks ? "Ask us" : nil,
            action: { UIApplication.shared.open(OpenModelsCopy.contactURL) },
            showsPerMonth: false,
            showsOpenLine: false
        )
    }
}
