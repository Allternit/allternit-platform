import SwiftUI

/// Plan member card, plan tickets and per-account usage cards (Settings →
/// Billing and Usage). Same designs as the web (components/billing/PlanCard.tsx,
/// components/usage/UsageAccountCards.tsx). Card faces are the engraved Gizzi
/// art per tier (PlanCard* assets, made by allternit-ai
/// scripts/brand/plan-cards/make_plan_cards.py). The Max plan keeps id `super`.
enum PlanTier: String {
    case free, plus, max, ultra

    init(planId: String) {
        switch planId {
        case "super", "max": self = .max
        case "plus": self = .plus
        case "ultra": self = .ultra
        default: self = .free
        }
    }

    var asset: String { "PlanCard\(rawValue.capitalized)" }
    var stamp: String { rawValue.uppercased() }
    var text: Color {
        switch self {
        case .free: return Color(red: 0.106, green: 0.106, blue: 0.102)
        case .plus: return Color(red: 0.949, green: 0.945, blue: 0.925)
        case .max: return Color(red: 0.169, green: 0.110, blue: 0.027)
        case .ultra: return Color(red: 0.118, green: 0.125, blue: 0.141)
        }
    }
    var stampFill: Color { text }
    var stampText: Color {
        switch self {
        case .free: return .white
        case .plus: return Color(red: 0.067, green: 0.067, blue: 0.067)
        case .max: return Color(red: 0.906, green: 0.761, blue: 0.478)
        case .ultra: return Color(red: 0.945, green: 0.945, blue: 0.937)
        }
    }
    var lightInk: Bool { self == .plus }
}

struct MemberCardView: View {
    let planId: String
    let member: String
    var renewsAt: String? = nil
    var status: String? = nil
    var width: CGFloat = 320

    private var tier: PlanTier { PlanTier(planId: planId) }
    private var renews: String? {
        guard let renewsAt else { return nil }
        let f = ISO8601DateFormatter()
        guard let date = f.date(from: renewsAt) else { return nil }
        return date.formatted(.dateTime.month(.abbreviated).day()).uppercased()
    }
    private var stamp: String? { status == "past_due" ? "PAYMENT DUE" : status == "unpaid" ? "PAUSED" : nil }

    var body: some View {
        let h = width / 1.586
        ZStack(alignment: .topLeading) {
            Image(tier.asset).resizable().scaledToFill().frame(width: width, height: h).clipped()
            if tier == .ultra {
                RoundedRectangle(cornerRadius: width * 0.035)
                    .strokeBorder(AngularGradient(colors: [Color(white: 0.8), Color(red: 0.96, green: 0.9, blue: 0.72), Color(red: 0.75, green: 0.91, blue: 0.87), Color(red: 0.77, green: 0.82, blue: 1), Color(white: 0.8)], center: .center), lineWidth: 1)
                    .padding(5)
            }
            HStack(spacing: 8) {
                Image(tier.lightInk ? "PlanWordmarkLight" : "PlanWordmarkInk").resizable().scaledToFit().frame(height: width * 0.034)
                Text(tier.stamp)
                    .font(.system(size: width * 0.029, weight: .bold, design: .monospaced))
                    .tracking(1.4)
                    .foregroundColor(tier.stampText)
                    .padding(.horizontal, 5).padding(.vertical, 2)
                    .background(tier.stampFill, in: RoundedRectangle(cornerRadius: 3))
            }
            .padding(.leading, width * 0.055).padding(.top, h * 0.08)
            if let stamp {
                Text(stamp)
                    .font(.system(size: 11, weight: .bold, design: .monospaced))
                    .foregroundColor(Color(red: 0.83, green: 0.28, blue: 0.23))
                    .padding(.horizontal, 6).padding(.vertical, 2)
                    .background(Color.white.opacity(0.55))
                    .overlay(RoundedRectangle(cornerRadius: 4).stroke(Color(red: 0.83, green: 0.28, blue: 0.23), lineWidth: 1.5))
                    .rotationEffect(.degrees(-6))
                    .padding(.leading, width * 0.055).padding(.top, h * 0.32)
            }
            VStack(alignment: .leading, spacing: 2) {
                Spacer()
                Text("MEMBER").font(.system(size: width * 0.026, design: .monospaced)).tracking(1.6).foregroundColor(tier.text.opacity(0.6))
                Text(member.uppercased()).font(.system(size: width * 0.044, design: .monospaced)).foregroundColor(tier.text).lineLimit(1).frame(maxWidth: width * 0.5, alignment: .leading)
                if let renews, tier != .free {
                    Text("RENEWS").font(.system(size: width * 0.026, design: .monospaced)).tracking(1.6).foregroundColor(tier.text.opacity(0.6)).padding(.top, 4)
                    Text(renews).font(.system(size: width * 0.036, design: .monospaced)).foregroundColor(tier.text)
                }
            }
            .padding(.leading, width * 0.055).padding(.bottom, h * 0.07)
            .frame(height: h, alignment: .bottomLeading)
        }
        .frame(width: width, height: h)
        .clipShape(RoundedRectangle(cornerRadius: width * 0.045))
        .shadow(color: .black.opacity(0.22), radius: 14, y: 8)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(tier.stamp) plan card for \(member)\(renews.map { ", renews \($0)" } ?? "")\(stamp.map { ", \($0.lowercased())" } ?? "")")
    }
}

/// A plan as a ticket: details on the body, the action on a tear-off stub.
struct PlanTicketView: View {
    let planId: String
    let label: String
    let price: String
    let lines: [String]
    let current: Bool
    let actionTitle: String?
    let action: () -> Void

    private var tier: PlanTier { PlanTier(planId: planId) }

    var body: some View {
        HStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 8) {
                HStack(alignment: .firstTextBaseline, spacing: 5) {
                    Text(label.uppercased())
                        .font(.system(size: 11, weight: .bold, design: .monospaced)).tracking(1.4)
                        .foregroundColor(tier.stampText)
                        .padding(.horizontal, 6).padding(.vertical, 2)
                        .background(tier.stampFill, in: RoundedRectangle(cornerRadius: 3))
                    Spacer()
                    Text(price).font(.system(size: 20, design: .monospaced))
                    Text("/mo").font(.system(size: 10.5)).opacity(0.7)
                }
                ForEach(lines, id: \.self) { Text($0).font(.system(size: 12)) }
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            Rectangle().fill(tier.text.opacity(0.25)).frame(width: 1).padding(.vertical, 6)
            VStack {
                Spacer()
                if current {
                    Text("Current plan").font(.system(size: 11.5, weight: .semibold))
                        .padding(.vertical, 6).frame(maxWidth: .infinity)
                        .overlay(RoundedRectangle(cornerRadius: 11).stroke(tier.text, lineWidth: 1))
                } else if let actionTitle {
                    Button(action: action) {
                        Text(actionTitle).font(.system(size: 11.5, weight: .semibold)).multilineTextAlignment(.center)
                            .foregroundColor(tier.stampText)
                            .padding(.vertical, 7).frame(maxWidth: .infinity)
                            .background(tier.stampFill, in: RoundedRectangle(cornerRadius: 11))
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(8)
            .frame(width: 104)
        }
        .foregroundColor(tier.text)
        .background(
            Image(tier.asset).resizable().scaledToFill().scaleEffect(1.6, anchor: .topLeading).clipped()
        )
        .clipShape(RoundedRectangle(cornerRadius: 16))
        .overlay(RoundedRectangle(cornerRadius: 16).stroke(current ? Color(red: 0.851, green: 0.467, blue: 0.341) : .clear, lineWidth: 2))
        .accessibilityElement(children: .contain)
    }
}

/// Settings → Usage: one card per connected account with its ring, plan and
/// windows (`/api/v1/providers/:id/quota`), plus Allternit Cloud's allowance.
struct AccountUsageCardsView: View {
    @ObservedObject private var quotas = ProviderQuotaStore.shared
    @ObservedObject private var usage = UsageStore.shared
    @ObservedObject private var models = ModelStore.shared

    private var providerIds: [String] {
        var ids = ["allternit"]
        for m in models.models where !ids.contains(m.provider) && ModelProviderNames.rank(m.provider) != 2 { ids.append(m.provider) }
        return ids
    }

    var body: some View {
        VStack(spacing: 10) {
            ForEach(providerIds, id: \.self) { pid in
                card(pid)
            }
        }
        .onAppear {
            models.fetchModelsIfNeeded()
            quotas.load(providerIds)
        }
    }

    private func card(_ pid: String) -> some View {
        let windows = pid == "allternit" ? [ProviderQuotaStore.cloudWindow()].compactMap { $0 } : (quotas.byProvider[pid]?.windows ?? [])
        let sorted = windows.sorted { $0.usedRatio > $1.usedRatio }
        let plan = pid == "allternit" ? usage.planLabel : quotas.byProvider[pid]?.plan
        return HStack(alignment: .top, spacing: 12) {
            LimitRingView(window: sorted.first, inner: sorted.count > 1 ? sorted[1] : nil, size: 54) {
                ModelProviderMark(providerId: pid, size: 22)
            }
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 6) {
                    Text(ModelProviderNames.name(pid)).font(.system(size: 14.5, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                    if let plan {
                        Text(plan).font(.system(size: 10.5)).foregroundColor(Color("TextSecondary"))
                            .padding(.horizontal, 7).padding(.vertical, 1)
                            .background(Color("FillNeutral"), in: Capsule())
                    }
                }
                if windows.isEmpty {
                    Text("This account doesn't report limits.").font(.system(size: 12)).foregroundColor(Color("TextTertiary"))
                }
                ForEach(windows) { w in
                    VStack(alignment: .leading, spacing: 3) {
                        HStack {
                            Text(w.displayLabel).font(.system(size: 12)).foregroundColor(Color("TextSecondary"))
                            Spacer()
                            Text("\(Int((w.usedRatio * 100).rounded()))%\(w.resetDate.map { " · resets " + $0.formatted(.dateTime.weekday().hour().minute()) } ?? "")")
                                .font(.system(size: 12)).foregroundColor(Color("TextPrimary")).monospacedDigit()
                        }
                        ProgressView(value: w.usedRatio).tint(LimitRingView<EmptyView>.color(w.usedRatio))
                    }
                }
            }
        }
        .padding(12)
        .background(Color("FillNeutral").opacity(0.5), in: RoundedRectangle(cornerRadius: 16))
        .overlay(RoundedRectangle(cornerRadius: 16).stroke(Color("BorderSubtle"), lineWidth: 1))
    }
}
