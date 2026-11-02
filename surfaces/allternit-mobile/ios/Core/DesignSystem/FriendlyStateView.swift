import SwiftUI

/// A reusable polished empty / error / offline state.
/// Used across tab surfaces so every list and feed reads consistently when
/// there is no data or the backend cannot be reached.
struct FriendlyStateView: View {
    enum Style {
        case empty
        case error
        case offline
    }

    let style: Style
    let icon: String
    let title: String
    let message: String
    var actionTitle: String? = nil
    var action: (() -> Void)? = nil

    var body: some View {
        VStack(spacing: 16) {
            Image(systemName: icon)
                .font(.system(size: 28, weight: .medium))
                .foregroundColor(foregroundColor)
                .frame(width: 64, height: 64)
                .background(backgroundColor)
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG, style: .continuous))
                .overlay(
                    RoundedRectangle(cornerRadius: Theme.radiusLG, style: .continuous)
                        .stroke(borderColor, lineWidth: 1)
                )

            Text(title)
                .font(.system(.title3, design: .serif))
                .fontWeight(.medium)
                .foregroundColor(Color("TextPrimary"))
                .multilineTextAlignment(.center)

            Text(message)
                .font(.subheadline)
                .foregroundColor(Color("TextSecondary"))
                .multilineTextAlignment(.center)
                .padding(.horizontal, 40)

            if let actionTitle, let action {
                Button(action: {
                    let generator = UIImpactFeedbackGenerator(style: .light)
                    generator.impactOccurred()
                    action()
                }) {
                    Text(actionTitle)
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .padding(.horizontal, 18)
                        .frame(height: 40)
                        .background(Color("BgPanel"))
                        .clipShape(Capsule())
                        .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
                }
                .buttonStyle(.plain)
                .padding(.top, 4)
            }
        }
        .padding(.top, 48)
        .padding(.bottom, 24)
        .frame(maxWidth: .infinity)
    }

    private var foregroundColor: Color {
        switch style {
        case .empty: return Color("AccentPrimary")
        case .error: return Theme.statusError
        case .offline: return Theme.statusWarning
        }
    }

    private var backgroundColor: Color {
        switch style {
        case .empty: return Color("AccentPrimary").opacity(0.12)
        case .error: return Theme.statusError.opacity(0.12)
        case .offline: return Theme.statusWarning.opacity(0.12)
        }
    }

    private var borderColor: Color {
        switch style {
        case .empty: return Color("AccentPrimary").opacity(0.25)
        case .error: return Theme.statusError.opacity(0.25)
        case .offline: return Theme.statusWarning.opacity(0.25)
        }
    }
}

/// A compact inline empty / error / offline state for list rows and section
/// cells where the full `FriendlyStateView` centered hero would be too much.
struct FriendlyInlineStateView: View {
    enum Style {
        case empty
        case error
        case offline
    }

    let style: Style
    let icon: String
    let title: String
    let message: String
    var actionTitle: String? = nil
    var action: (() -> Void)? = nil

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                Image(systemName: icon)
                    .font(.system(size: 18, weight: .medium))
                    .foregroundColor(foregroundColor)
                    .frame(width: 36, height: 36)
                    .background(backgroundColor)
                    .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD, style: .continuous))

                VStack(alignment: .leading, spacing: 3) {
                    Text(title)
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))

                    Text(message)
                        .font(.subheadline)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(3)
                }

                Spacer(minLength: 0)
            }

            if let actionTitle, let action {
                Button(action: {
                    let generator = UIImpactFeedbackGenerator(style: .light)
                    generator.impactOccurred()
                    action()
                }) {
                    Text(actionTitle)
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .padding(.horizontal, 14)
                        .frame(height: 32)
                        .background(Color("BgPanel"))
                        .clipShape(Capsule())
                        .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.vertical, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var foregroundColor: Color {
        switch style {
        case .empty: return Color("AccentPrimary")
        case .error: return Theme.statusError
        case .offline: return Theme.statusWarning
        }
    }

    private var backgroundColor: Color {
        switch style {
        case .empty: return Color("AccentPrimary").opacity(0.12)
        case .error: return Theme.statusError.opacity(0.12)
        case .offline: return Theme.statusWarning.opacity(0.12)
        }
    }
}

/// Shared helper that turns raw URLSession / transport errors into product-
/// friendly copy for `FriendlyStateView` and other empty/error states.
enum FriendlyErrorMessage {
    static func from(_ error: String) -> String {
        let lowered = error.lowercased()
        // cloud-api 428 (routes/beta.rs): the account has no computer yet.
        if lowered.contains("data-plane node") {
            return "Connect a computer or start Allternit Cloud to use this."
        }
        if lowered.contains("could not connect") || lowered.contains("failed to connect") {
            return "Could not connect to the server. Check your connection and try again."
        }
        if lowered.contains("cancelled") || lowered.contains("canceled") {
            return "The request was cancelled."
        }
        if lowered.contains("not found") || lowered.contains("404") {
            return "That resource couldn't be found."
        }
        return error
    }
}

#Preview {
    VStack(spacing: 24) {
        FriendlyStateView(
            style: .empty,
            icon: "cpu",
            title: "No agents yet",
            message: "Create an agent from a template to get started.",
            actionTitle: "New from template",
            action: {}
        )
        FriendlyStateView(
            style: .offline,
            icon: "wifi.slash",
            title: "You're offline",
            message: "Connect to the internet to load your agents.",
            actionTitle: "Retry",
            action: {}
        )
    }
    .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
}
