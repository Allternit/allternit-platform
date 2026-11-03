import SwiftUI

/// App permissions that get an app-owned priming sheet BEFORE the iOS system
/// dialog (Claude iOS parity). Priming shows once per permission — the
/// UserDefaults flag flips on the first presentation, whichever button the
/// user taps; afterwards the system prompt (or its result) drives the flow.
enum AppPermission: String, Sendable, Identifiable, CaseIterable {
    case photos
    case camera
    case microphone
    case notifications

    /// For `.sheet(item:)` presentation of the priming sheet.
    var id: String { rawValue }

    private var primingShownKey: String { "allternit-priming-shown-\(rawValue)" }

    var hasPrimed: Bool { UserDefaults.standard.bool(forKey: primingShownKey) }
    func markPrimed() { UserDefaults.standard.set(true, forKey: primingShownKey) }

    /// `-reset-onboarding` (DEBUG): clears every priming flag so the
    /// first-run sheets show again.
    static func resetAllPriming() {
        for permission in allCases {
            UserDefaults.standard.removeObject(forKey: permission.primingShownKey)
        }
    }

    var icon: String {
        switch self {
        case .photos: return "photo.on.rectangle"
        case .camera: return "camera"
        case .microphone: return "mic"
        case .notifications: return "bell"
        }
    }

    var title: String {
        switch self {
        case .photos: return "Access your photos?"
        case .camera: return "Use your camera?"
        case .microphone: return "Dictate messages?"
        case .notifications: return "Let your bots reach you?"
        }
    }

    /// One-line why, shown under the title.
    var message: String {
        switch self {
        case .photos: return "Pick photos to attach to your chat, and see recent ones right in the composer."
        case .camera: return "Take a photo and attach it straight to your chat."
        case .microphone: return "Allternit transcribes what you say into the message field. Audio is only used while you dictate."
        case .notifications: return "Know when a task finishes or a bot needs your approval. No marketing messages. Change this in Settings."
        }
    }
}

/// Reusable permission-priming sheet: mode-accent icon, one-line why,
/// Continue / Not Now. Only Continue proceeds to the system permission
/// request; either choice marks the permission primed (shows once).
struct PermissionPrimingSheet: View {
    let permission: AppPermission
    /// Called after Continue (the caller then requests the system permission).
    let onContinue: () -> Void

    @Environment(\.dismiss) private var dismiss

    var body: some View {
        if permission == .notifications {
            notificationsBody
        } else {
            standardBody
        }
    }

    /// Notifications: real examples of what a bot sends, then the ask.
    private var notificationsBody: some View {
        VStack(spacing: 0) {
            VStack(spacing: 10) {
                exampleRow(icon: "magnifyingglass", tint: Color(red: 0.86, green: 0.91, blue: 0.97), title: "Research finished", detail: "Sources gathered, ready to read")
                    .padding(.leading, 28)
                exampleRow(icon: "chevron.left.forwardslash.chevron.right", tint: Color(red: 0.88, green: 0.93, blue: 0.82), title: "Code task done", detail: "Changes ready for review")
                    .padding(.trailing, 28)
                exampleRow(icon: "hand.raised", tint: Color(red: 0.96, green: 0.86, blue: 0.86), title: "A bot needs your approval", detail: "It waits until you answer")
                    .padding(.leading, 28)
            }
            .padding(.horizontal, 16)
            .padding(.top, 36)
            .padding(.bottom, 22)
            .frame(maxWidth: .infinity)
            .background(LinearGradient(colors: [Color(white: 0.95), Color(white: 0.87)], startPoint: .top, endPoint: .bottom))
            .accessibilityHidden(true)

            VStack(alignment: .leading, spacing: 10) {
                Text(permission.title)
                    .font(.system(size: 26, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                Text(permission.message)
                    .font(.system(size: 15))
                    .foregroundColor(Color("TextSecondary"))
                Spacer()
                Button(action: {
                    UIImpactFeedbackGenerator(style: .light).impactOccurred()
                    permission.markPrimed()
                    dismiss()
                    onContinue()
                }) {
                    Text("Turn on notifications")
                        .font(.system(.body, weight: .semibold))
                        .foregroundColor(Color("BgPrimary"))
                        .frame(maxWidth: .infinity)
                        .frame(height: 50)
                        .background(Color("TextPrimary"), in: Capsule())
                }
                Button(action: {
                    permission.markPrimed()
                    dismiss()
                }) {
                    Text("Not now")
                        .font(.system(.body, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .frame(maxWidth: .infinity)
                        .frame(height: 50)
                        .overlay(Capsule().stroke(Color("BorderStrong"), lineWidth: 1))
                }
            }
            .padding(20)
        }
        .background(Color("BgPrimary").ignoresSafeArea())
        .presentationDetents([.large])
    }

    private func exampleRow(icon: String, tint: Color, title: String, detail: String) -> some View {
        HStack(spacing: 10) {
            Image(systemName: icon)
                .font(.system(size: 15))
                .foregroundColor(.black.opacity(0.7))
                .frame(width: 38, height: 38)
                .background(tint, in: RoundedRectangle(cornerRadius: 10))
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.system(size: 14, weight: .semibold)).foregroundColor(.black.opacity(0.88))
                Text(detail).font(.system(size: 12.5)).foregroundColor(.black.opacity(0.55))
            }
            Spacer(minLength: 0)
        }
        .padding(9)
        .background(Color.white.opacity(0.85), in: RoundedRectangle(cornerRadius: 16))
    }

    private var standardBody: some View {
        VStack(spacing: 20) {
            Spacer()

            Image(systemName: permission.icon)
                .font(.system(size: 28, weight: .medium))
                .foregroundColor(Color("AccentPrimary"))
                .frame(width: 72, height: 72)
                .background(Color("AccentPrimary").opacity(0.14))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))
                .overlay(
                    RoundedRectangle(cornerRadius: Theme.radiusLG)
                        .stroke(Color("AccentPrimary").opacity(0.28), lineWidth: 1)
                )

            VStack(spacing: 8) {
                Text(permission.title)
                    .font(.system(.title3, design: .serif))
                    .fontWeight(.medium)
                    .foregroundColor(Color("TextPrimary"))

                Text(permission.message)
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
                    .multilineTextAlignment(.center)
                    .padding(.horizontal, 24)
            }

            Spacer()

            VStack(spacing: 10) {
                Button(action: {
                    let generator = UIImpactFeedbackGenerator(style: .light)
                    generator.impactOccurred()
                    permission.markPrimed()
                    dismiss()
                    onContinue()
                }) {
                    Text("Continue")
                        .font(.system(.body, weight: .semibold))
                        .foregroundColor(.black)
                        .frame(maxWidth: .infinity)
                        .frame(height: 48)
                        .background(Color("AccentPrimary"))
                        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                }

                Button(action: {
                    permission.markPrimed()
                    dismiss()
                }) {
                    Text("Not Now")
                        .font(.system(.body, weight: .medium))
                        .foregroundColor(Color("TextSecondary"))
                        .frame(maxWidth: .infinity)
                        .frame(height: 44)
                }
            }
            .padding(.horizontal, 20)
            .padding(.bottom, 12)
        }
        .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
        .presentationDetents([.height(380)])
    }
}
