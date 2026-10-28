import SwiftUI

/// Dismissible usage banner (Claude's "94% of weekly limit" pattern),
/// pinned to the top of the chat feed below TransientErrorBanner once usage
/// crosses 80% of the weekly window. At/above 100% the UsageWallCard takes
/// over and this hides (UsageStore.shouldShowBanner).
struct UsageLimitBanner: View {
    /// Rounded percent copy, e.g. "94%".
    let percentText: String
    /// "Monday 9:00 AM"-style reset label; nil when the backend didn't
    /// report a reset time.
    let resetsLabel: String?
    let onUpgrade: () -> Void
    let onDismiss: () -> Void
    /// "Upgrade" links out to the web, allowed on the US storefront only.
    @ObservedObject private var storefront = StorefrontGate.shared

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: "gauge.with.dots.needle.67percent")
                .font(.subheadline)
                .foregroundColor(Theme.statusWarning)

            VStack(alignment: .leading, spacing: 1) {
                Text("You've used \(percentText) of your usage limit")
                    .font(.subheadline)
                    .fontWeight(.medium)
                    .foregroundColor(Color("TextPrimary"))
                if let resetsLabel {
                    Text("Resets \(resetsLabel)")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
            }

            Spacer(minLength: 8)

            if storefront.allowsPurchaseLinks {
                Button(action: {
                    let generator = UIImpactFeedbackGenerator(style: .light)
                    generator.impactOccurred()
                    onUpgrade()
                }) {
                    Text("Upgrade")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundColor(.black)
                        .padding(.horizontal, 12)
                        .padding(.vertical, 6)
                        .background(Color("AccentPrimary"))
                        .clipShape(Capsule())
                }
            }

            Button(action: onDismiss) {
                Image(systemName: "xmark")
                    .font(.caption)
                    .fontWeight(.semibold)
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 28, height: 28)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .background(Color("BgSecondary"))
    }
}

/// Hard usage wall (ChatGPT pattern): at/above 100% of the weekly window
/// the composer locks and this card sits at the bottom of the feed with the
/// two ways out — add credits or upgrade.
struct UsageWallCard: View {
    /// "Monday 9:00 AM"-style reset label; nil when unknown.
    let resetsLabel: String?
    let onAddCredits: () -> Void
    let onGetPro: () -> Void
    /// Credits / Pro buttons link out to the web: US storefront only.
    @ObservedObject private var storefront = StorefrontGate.shared

    private var message: String {
        if storefront.allowsPurchaseLinks {
            return resetsLabel.map { "Your limit resets \($0). Add credits or upgrade to Pro to keep going." }
                ?? "Add credits or upgrade to Pro to keep going."
        }
        return resetsLabel.map { "Your limit resets \($0)." } ?? "Your limit resets soon."
    }

    var body: some View {
        VStack(spacing: 14) {
            Image(systemName: "gauge.with.dots.needle.100percent")
                .font(.system(size: 22, weight: .medium))
                .foregroundColor(Theme.statusWarning)
                .frame(width: 52, height: 52)
                .background(Theme.statusWarning.opacity(0.14))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))

            VStack(spacing: 6) {
                Text("You're out of usage for now")
                    .font(.system(.headline, design: .serif))
                    .foregroundColor(Color("TextPrimary"))

                Text(message)
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
                    .multilineTextAlignment(.center)
            }

            if storefront.allowsPurchaseLinks {
                HStack(spacing: 10) {
                    Button(action: {
                        let generator = UIImpactFeedbackGenerator(style: .light)
                        generator.impactOccurred()
                        onAddCredits()
                    }) {
                        Text("Add credits")
                            .font(.system(.subheadline, weight: .semibold))
                            .foregroundColor(Color("AccentPrimary"))
                            .frame(maxWidth: .infinity)
                            .frame(height: 40)
                            .background(
                                RoundedRectangle(cornerRadius: Theme.radiusMD)
                                    .stroke(Color("AccentPrimary"), lineWidth: 1)
                            )
                    }

                    Button(action: {
                        let generator = UIImpactFeedbackGenerator(style: .light)
                        generator.impactOccurred()
                        onGetPro()
                    }) {
                        Text("Get Pro")
                            .font(.system(.subheadline, weight: .semibold))
                            .foregroundColor(.black)
                            .frame(maxWidth: .infinity)
                            .frame(height: 40)
                            .background(Color("AccentPrimary"))
                            .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                    }
                }
            }
        }
        .padding(18)
        .frame(maxWidth: .infinity)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusLG)
                .stroke(Theme.borderWarmDefault, lineWidth: 1)
        )
    }
}
