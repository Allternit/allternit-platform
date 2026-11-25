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
        // Allternit Cloud reports its monthly allowance only (no weekly
        // window): same window, title and reset line as Settings → Usage.
        if let window = cloudUsageWindows(usage.snapshot).first {
            PlanUsageBar(
                label: usageWindowTitle(window),
                detail: "",
                fraction: window.usedRatio,
                footnote: usageResetsText(window.resetDate)
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

/// "Choose a plan" / "Buy credits" rows. On the US storefront they link out to
/// allternit.com; elsewhere the App Store doesn't allow the link, so nothing
/// is shown (the caller shows plan status only, and the In-App Purchase
/// rows where the App Store sells). A plan Apple or Google bills is managed
/// in that store (StoreBilledByView), so "Manage billing" is web plans only.
struct PayLinkRows: View {
    @ObservedObject private var storefront = StorefrontGate.shared
    @ObservedObject private var plan = AccountPlanStore.shared
    @Environment(\.openURL) private var openURL

    var body: some View {
        if storefront.allowsPurchaseLinks {
            if plan.subscription?.isPaid != true {
                linkRow("Choose a plan", systemImage: "star") { openURL(plan.upgradeURL) }
            }
            linkRow("Buy credits", systemImage: "creditcard") { openURL(StorefrontGate.creditsURL) }
            if plan.subscription?.isPaid == true, (plan.subscription?.billingProvider ?? "stripe") == "stripe" {
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

// MARK: - Settings → Usage (phone calm I15, mirrors web PlanUsageBars, ai #1161)

/// One usage bar (web `UsageWindowBar`): title, "N% used", a bar that grows
/// once, and the reset line. Neutral fill, amber from the heads-up threshold,
/// red from the wrap-up one (`UsageThresholds`). Reduced motion shows the bar
/// at its width.
struct UsageWindowBar: View {
    let label: String
    /// 0…1 (clamped).
    let fraction: Double
    var reset: String? = nil
    var compact = false

    @ReducedMotion private var reduced
    @State private var grown = false

    private var clamped: Double { min(1, max(0, fraction)) }
    private var percent: Int { Int((clamped * 100).rounded()) }
    private var tone: Color {
        switch usageLevel(clamped) {
        case .reached: return Theme.statusError
        case .approaching: return Theme.statusWarning
        case .ok: return Color("TextPrimary")
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: compact ? 5 : 7) {
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                Text(label)
                    .font(.system(size: compact ? 12 : 14))
                    .foregroundColor(Color(compact ? "TextSecondary" : "TextPrimary"))
                    .lineLimit(1)
                    .frame(maxWidth: .infinity, alignment: .leading)
                Text("\(percent)% used")
                    .font(.system(size: compact ? 12 : 13))
                    .foregroundColor(Color("TextSecondary"))
                    .monospacedDigit()
            }
            GeometryReader { proxy in
                ZStack(alignment: .leading) {
                    Capsule().fill(Color("FillNeutral"))
                    Capsule()
                        .fill(tone)
                        .frame(width: proxy.size.width * clamped)
                        .scaleEffect(x: grown || reduced ? 1 : 0.001, anchor: .leading)
                }
            }
            .frame(height: compact ? 4 : 6)
            if let reset {
                Text(reset)
                    .font(.system(size: compact ? 11 : 12))
                    .foregroundColor(Color("TextTertiary"))
            }
        }
        .padding(.vertical, compact ? 4 : 8)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(label)
        .accessibilityValue(reset.map { "\(percent)% used. \($0)" } ?? "\(percent)% used")
        .onAppear {
            guard !grown, !reduced else { return }
            withAnimation(.timingCurve(0.22, 1, 0.36, 1, duration: 0.6)) { grown = true }
        }
    }
}

/// A Usage page card: white (graphite in dark) with a hairline, like the
/// web phone layout's plan and credits cards. Never tan.
struct UsageCard<Content: View>: View {
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 0) { content }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(EdgeInsets(top: 8, leading: 14, bottom: 8, trailing: 14))
            .background(Color("BgPrimary"), in: RoundedRectangle(cornerRadius: 16, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
    }
}

/// A section label with an optional muted detail ("Allternit Cloud  Plus").
struct UsageSectionLabel: View {
    let title: String
    var detail: String? = nil

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(title)
                .font(.system(size: 15, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
            if let detail {
                Text(detail)
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextTertiary"))
                    .lineLimit(1)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(EdgeInsets(top: 18, leading: 4, bottom: 8, trailing: 4))
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(.isHeader)
    }
}

/// One calm line for an empty or failed section (no status numbers).
struct UsageNote: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.system(size: 13))
            .foregroundColor(Color("TextSecondary"))
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.vertical, 8)
    }
}

/// Settings → Usage, in the web's order: Allternit Cloud (plan window with
/// its reset, cloud computer hours), Connected accounts (the Cloud card,
/// then every subscription or key account that reports limits, 5-hour and
/// weekly bars where the provider reports them), Credits (balance, Buy
/// credits), then the real thresholds. Sources:
/// - Plan window: `GET /api/v1/me/usage` (`UsageStore`, `cloudUsageWindows`).
///   Cloud reports a monthly window only, so there is no Cloud weekly bar.
/// - Cloud computer hours: `GET /api/v1/hosted-runtimes/entitlement`.
/// - Connected accounts: `GET /api/v1/providers/:id/quota` (`ProviderQuotaStore`).
/// - Credits: `GET /api/v1/billing/credits` (`AccountPlanStore`).
/// - Buy credits: `StorePurchaseSheet(focus: .pack)` or the web billing
///   page, exactly as `ComputeGateView` decides (`BuyCreditsRoute`). No
///   purchase code lives here.
/// Calm states: placeholder bars while loading, one plain line when there is
/// nothing; a compute-gate refusal on a read is an empty state, not an error.
struct UsagePageView: View {
    @ObservedObject private var usage = UsageStore.shared
    @ObservedObject private var plan = AccountPlanStore.shared
    @ObservedObject private var storefront = StorefrontGate.shared
    @ObservedObject private var auth = AuthManager.shared
    @Environment(\.openURL) private var openURL
    @State private var isStoreSheetPresented = false

    private var cloudWindows: [ProviderLimitWindow] { sortUsageWindows(cloudUsageWindows(usage.snapshot)) }

    private var buyRoute: BuyCreditsRoute {
        BuyCreditsRoute.resolve(
            storeSellsPacks: storefront.showsStoreButtons(for: .pack),
            allowsPurchaseLinks: storefront.allowsPurchaseLinks
        )
    }

    var body: some View {
        PhoneSettingsPage(title: "Usage") {
            UsageSectionLabel(title: "Allternit Cloud", detail: auth.isSignedIn ? plan.planName ?? usage.planLabel : nil)
            UsageCard { cloudBody }

            UsageSectionLabel(title: "Connected accounts")
            AccountUsageCardsView()

            if auth.isSignedIn {
                UsageSectionLabel(title: "Credits")
                UsageCard { creditsBody }
            }

            Text(UsageThresholds.footerText)
                .font(.system(size: 12))
                .foregroundColor(Color("TextTertiary"))
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(EdgeInsets(top: 14, leading: 4, bottom: 0, trailing: 4))
        }
        .refreshable { await reload(force: true) }
        .task { await reload(force: true) }
        .sheet(isPresented: $isStoreSheetPresented) { StorePurchaseSheet(focus: .pack) }
    }

    // MARK: Allternit Cloud

    @ViewBuilder
    private var cloudBody: some View {
        let windows = cloudWindows
        let hours = plan.cloudHoursFraction
        if !auth.isSignedIn {
            UsageNote(text: "Sign in to see your plan usage.")
        } else if windows.isEmpty && hours == nil && (usage.isLoading || plan.isLoading || usage.availability == .unknown) {
            placeholderBars(2)
        } else if windows.isEmpty && hours == nil {
            // Unmetered, metering off on this server, or a read the server
            // refused: one plain line, never the server's status.
            UsageNote(text: "No plan limits to show. Allternit Cloud reports them once your plan has an allowance.")
        } else {
            ForEach(windows) { w in
                UsageWindowBar(label: usageWindowTitle(w), fraction: w.usedRatio, reset: usageResetsText(w.resetDate))
            }
            if let hours, let entitlement = plan.entitlement {
                UsageWindowBar(
                    label: "Cloud computer · this month",
                    fraction: hours,
                    reset: [
                        usageHoursText(usedSeconds: entitlement.usedSecondsMonthly, maxHours: entitlement.maxHoursMonthly),
                        usageResetsText(usageNextMonthStart()),
                    ].compactMap { $0 }.joined(separator: " · ")
                )
            }
        }
    }

    // MARK: Credits

    @ViewBuilder
    private var creditsBody: some View {
        HStack(alignment: .center, spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                if let credits = plan.credits {
                    Text(Self.usd(credits.balanceUsd))
                        .font(.system(size: 22, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .monospacedDigit()
                    Text("\(Self.usd(credits.monthToDateUsageUsd)) used this month")
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextTertiary"))
                } else if let balance = usage.snapshot?.credits {
                    // Older servers: the balance rides on `/me/usage` only.
                    Text(Self.usd(balance))
                        .font(.system(size: 22, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .monospacedDigit()
                } else if plan.isLoading {
                    Text("$00.00")
                        .font(.system(size: 22, weight: .semibold))
                        .redacted(reason: .placeholder)
                        .accessibilityLabel("Loading your credits")
                } else if plan.lastError == nil {
                    EmptyArt(kind: .coins, size: EmptyArtSize.inline.points)
                    UsageNote(text: "No credit balance yet.")
                } else {
                    UsageNote(text: "Couldn't load your credits. Pull down to try again.")
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            if buyRoute != .hidden {
                Button(action: buyCredits) {
                    Label("Buy credits", systemImage: "plus")
                        .labelStyle(.titleAndIcon)
                        .frame(minHeight: 44)
                }
                .buttonStyle(SettingsQuietButtonStyle())
                .accessibilityHint(buyRoute == .storeSheet ? "Shows the credit packs" : "Opens billing in your browser")
            }
        }
        .padding(.vertical, 6)
    }

    private func buyCredits() {
        Haptics.light()
        switch buyRoute {
        case .storeSheet: isStoreSheetPresented = true
        case .web: openURL(StorefrontGate.creditsURL)
        case .hidden: break
        }
    }

    // MARK: Helpers

    private func placeholderBars(_ count: Int) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(0..<count, id: \.self) { _ in
                UsageWindowBar(label: "This month", fraction: 0.4, reset: "Resets soon")
            }
        }
        .redacted(reason: .placeholder)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Loading usage")
    }

    private func reload(force: Bool) async {
        UsageStore.shared.fetchUsageIfNeeded(force: force)
        if auth.isSignedIn { AccountPlanStore.shared.load(force: force) }
        if force { ProviderQuotaStore.shared.refreshAll() }
        StorefrontGate.shared.refresh()
        // Sets which items the App Store sells here (Buy credits route).
        await StorePurchaseManager.shared.loadCatalog()
    }

    private static func usd(_ value: Double) -> String { String(format: "$%.2f", value) }
}
