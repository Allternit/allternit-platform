import SwiftUI

/// One usage meter row (the PWA's PlanUsageBars `UsageBar`): label, detail
/// and percent on top, a thin bar under it. Red at/over 100%.
struct PlanUsageBar: View {
    let label: String
    let detail: String
    /// 0…1+ (clamped for the bar, shown rounded).
    let fraction: Double
    var footnote: String? = nil

    private var percent: Int { Int((max(0, fraction) * 100).rounded()) }
    private var tone: Color {
        if fraction >= 1 { return Theme.statusError }
        if fraction >= 0.8 { return Theme.statusWarning }
        return Color("TextPrimary")
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline) {
                Text(label)
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                Spacer(minLength: 8)
                Text(detail)
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
                Text("\(percent)%")
                    .font(.caption.weight(.semibold))
                    .foregroundColor(fraction >= 1 ? Theme.statusError : Color("TextPrimary"))
                    .monospacedDigit()
            }
            GeometryReader { proxy in
                ZStack(alignment: .leading) {
                    Capsule().fill(Color("BgSecondary"))
                    Capsule()
                        .fill(tone)
                        .frame(width: proxy.size.width * min(max(fraction, 0), 1))
                }
            }
            .frame(height: 6)
            .accessibilityElement()
            .accessibilityLabel(label)
            .accessibilityValue("\(percent) percent used")
            if let footnote {
                Text(footnote)
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }
        }
        .padding(.vertical, 4)
    }
}

/// Plan + usage + credits rows for a Settings List section. Real sources
/// only; a row whose source failed is left out. Used by Settings → Usage and
/// Settings → Billing.
struct PlanUsageRows: View {
    @ObservedObject private var plan = AccountPlanStore.shared
    @ObservedObject private var usage = UsageStore.shared

    var body: some View {
        if let percent = usage.percentUsed {
            PlanUsageBar(
                label: usage.periodLabel,
                detail: "",
                fraction: percent / 100,
                footnote: usage.resetsLabel.map { "Resets \($0)" }
            )
        }
        if let fraction = plan.cloudHoursFraction, let detail = plan.cloudHoursDetail {
            PlanUsageBar(
                label: "Cloud computer · this month",
                detail: detail,
                fraction: fraction,
                footnote: "Resets \(AccountPlanStore.monthlyResetLabel)"
            )
        }
        if let free = plan.credits?.freeInference, free.monthlyAllowanceUsd > 0 {
            PlanUsageBar(
                label: "Free AI allowance",
                detail: String(format: "$%.2f of $%.2f", free.usedUsd, free.monthlyAllowanceUsd),
                fraction: free.usedUsd / free.monthlyAllowanceUsd,
                footnote: "Resets \(AccountPlanStore.monthlyResetLabel)"
            )
        }
        if let credits = plan.credits {
            HStack {
                Text("Credits")
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                Spacer()
                Text(String(format: "$%.2f balance · $%.2f used this month", credits.balanceUsd, credits.monthToDateUsageUsd))
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
                    .multilineTextAlignment(.trailing)
            }
        } else if let snapshot = usage.snapshot, let credits = snapshot.credits {
            HStack {
                Text("Credits")
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                Spacer()
                Text(String(format: "%g", credits))
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
            }
        }
        if usage.percentUsed == nil, plan.cloudHoursFraction == nil, plan.credits == nil {
            if plan.isLoading || usage.isLoading {
                HStack(spacing: 8) {
                    ProgressView()
                    Text("Loading usage…")
                        .font(.subheadline)
                        .foregroundColor(Color("TextSecondary"))
                }
            } else {
                FriendlyInlineStateView(
                    style: .empty,
                    icon: "gauge.with.dots.needle.33percent",
                    title: "No usage to show",
                    message: {
                        if case .unavailable(let message) = usage.availability {
                            return FriendlyErrorMessage.from(message)
                        }
                        return plan.lastError ?? "Usage isn't metered for this account."
                    }()
                )
            }
        }
    }
}

/// "Get Pro" / "Buy credits" rows. On the US storefront they link out to
/// allternit.com; elsewhere the App Store doesn't allow the link, so nothing
/// is shown (the caller shows plan status only).
struct PayLinkRows: View {
    @ObservedObject private var storefront = StorefrontGate.shared
    @ObservedObject private var plan = AccountPlanStore.shared
    @Environment(\.openURL) private var openURL

    var body: some View {
        if storefront.allowsPurchaseLinks {
            if plan.subscription?.isPaid != true {
                linkRow("Get Pro", systemImage: "star") { openURL(plan.upgradeURL) }
            }
            linkRow("Buy credits", systemImage: "creditcard") { openURL(StorefrontGate.creditsURL) }
            if plan.subscription?.isPaid == true {
                linkRow("Manage billing", systemImage: "doc.text") { openURL(StorefrontGate.creditsURL) }
            }
        }
    }

    private func linkRow(_ title: String, systemImage: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 10) {
                Image(systemName: systemImage)
                    .font(.subheadline)
                    .frame(width: 20)
                Text(title)
                    .font(.subheadline)
                Spacer()
                Image(systemName: "arrow.up.right")
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }
            .foregroundColor(Color("TextPrimary"))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}
