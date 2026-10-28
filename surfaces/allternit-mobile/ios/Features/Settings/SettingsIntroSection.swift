import SwiftUI
import UIKit
import UserNotifications

/// Settings → "Getting started": push notification status, and replays of
/// the first-run setup and "Meet your bots".
struct SettingsIntroSection: View {
    /// Closes the Settings sheet before a replay covers the app.
    let onDismissSettings: () -> Void

    @State private var status: UNAuthorizationStatus = .notDetermined
    @State private var isAsking = false

    var body: some View {
        Section {
            HStack {
                Label("Notifications", systemImage: "bell")
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                Spacer()
                notificationsTrailing
            }

            Button {
                onDismissSettings()
                // Let the sheet finish closing before the intro covers the app.
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.45) {
                    BotsIntroStore.shared.replay()
                }
            } label: {
                Label("Meet your bots", systemImage: "person.3")
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
            }

            Button {
                onDismissSettings()
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.45) {
                    // Root swaps back to the first-run setup (AllternitApp).
                    OnboardingStore.shared.isComplete = false
                }
            } label: {
                Label("Replay setup", systemImage: "arrow.counterclockwise")
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
            }
        } header: {
            Text("Getting started")
        } footer: {
            if let error = PushNotificationManager.shared.lastRegistrationError, status == .authorized {
                Text("Push isn't set up for this phone yet: \(error)")
            }
        }
        .task { status = await PushNotificationManager.shared.authorizationStatus() }
    }

    @ViewBuilder
    private var notificationsTrailing: some View {
        switch status {
        case .authorized, .provisional, .ephemeral:
            Text("On")
                .font(.subheadline)
                .foregroundColor(Color("TextSecondary"))
        case .denied:
            Button("Open iOS Settings") {
                if let url = URL(string: UIApplication.openSettingsURLString) {
                    UIApplication.shared.open(url)
                }
            }
            .font(.subheadline)
        default:
            if isAsking {
                ProgressView()
            } else {
                Button("Turn on") {
                    isAsking = true
                    Task {
                        _ = await PushNotificationManager.shared.requestPermissionAndRegister()
                        status = await PushNotificationManager.shared.authorizationStatus()
                        isAsking = false
                    }
                }
                .font(.subheadline)
            }
        }
    }
}
