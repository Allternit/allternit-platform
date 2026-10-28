import SwiftUI
import UIKit
@preconcurrency import UserNotifications

// MARK: - Appearance

/// Light / Dark / System, the PWA's Appearance tiles. Dark is the graphite
/// theme (the color assets' dark variants). Applied by overriding every
/// window's interface style, so no view needs `.preferredColorScheme`.
@MainActor
final class AppearanceStore: ObservableObject {
    enum Mode: String, CaseIterable, Identifiable {
        case light, dark, system
        var id: String { rawValue }
        var label: String {
            switch self {
            case .light: return "Light"
            case .dark: return "Dark"
            case .system: return "System"
            }
        }
        var style: UIUserInterfaceStyle {
            switch self {
            case .light: return .light
            case .dark: return .dark
            case .system: return .unspecified
            }
        }
    }

    static let shared = AppearanceStore()
    private static let key = "allternit-appearance-theme"

    @Published var theme: Mode {
        didSet {
            UserDefaults.standard.set(theme.rawValue, forKey: Self.key)
            apply()
        }
    }

    private init() {
        theme = Mode(rawValue: UserDefaults.standard.string(forKey: Self.key) ?? "") ?? .system
        // Windows created later (new scenes) pick the saved theme up too.
        NotificationCenter.default.addObserver(
            forName: UIScene.didActivateNotification, object: nil, queue: .main
        ) { _ in
            MainActor.assumeIsolated { AppearanceStore.shared.apply() }
        }
    }

    /// Call at launch (and it runs on every change) to apply the saved theme.
    func apply() {
        for scene in UIApplication.shared.connectedScenes {
            guard let windowScene = scene as? UIWindowScene else { continue }
            for window in windowScene.windows {
                window.overrideUserInterfaceStyle = theme.style
            }
        }
    }
}

/// Three picture tiles; tap picks.
struct AppearanceTiles: View {
    @ObservedObject private var store = AppearanceStore.shared

    var body: some View {
        HStack(spacing: 10) {
            ForEach(AppearanceStore.Mode.allCases) { option in
                Button(action: {
                    UISelectionFeedbackGenerator().selectionChanged()
                    store.theme = option
                }) {
                    VStack(spacing: 6) {
                        tilePicture(option)
                            .frame(height: 64)
                            .overlay(
                                RoundedRectangle(cornerRadius: 10)
                                    .stroke(store.theme == option ? Color("AccentPrimary") : Color("BorderSubtle"),
                                            lineWidth: store.theme == option ? 2 : 1)
                            )
                        Text(option.label)
                            .font(.caption.weight(store.theme == option ? .semibold : .regular))
                            .foregroundColor(Color("TextPrimary"))
                    }
                    .frame(maxWidth: .infinity)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(option.label)
                .accessibilityAddTraits(store.theme == option ? [.isSelected, .isButton] : .isButton)
            }
        }
        .padding(.vertical, 6)
    }

    @ViewBuilder
    private func tilePicture(_ option: AppearanceStore.Mode) -> some View {
        switch option {
        case .light:
            miniPage(bg: .white, line: Color(white: 0.85))
        case .dark:
            // Graphite, not brown.
            miniPage(bg: Color(white: 0.13), line: Color(white: 0.32))
        case .system:
            HStack(spacing: 0) {
                miniPage(bg: .white, line: Color(white: 0.85), corners: [.topLeft, .bottomLeft])
                miniPage(bg: Color(white: 0.13), line: Color(white: 0.32), corners: [.topRight, .bottomRight])
            }
        }
    }

    private func miniPage(bg: Color, line: Color, corners: UIRectCorner = .allCorners) -> some View {
        ZStack(alignment: .topLeading) {
            bg
            VStack(alignment: .leading, spacing: 5) {
                Capsule().fill(line).frame(width: 34, height: 5)
                Capsule().fill(line).frame(width: 22, height: 5)
                Circle().fill(Color("AccentPrimary")).frame(width: 8, height: 8)
            }
            .padding(10)
        }
        .clipShape(RoundedCornerShape(radius: 10, corners: corners))
    }
}

private struct RoundedCornerShape: Shape {
    let radius: CGFloat
    let corners: UIRectCorner
    func path(in rect: CGRect) -> Path {
        Path(UIBezierPath(roundedRect: rect, byRoundingCorners: corners,
                          cornerRadii: CGSize(width: radius, height: radius)).cgPath)
    }
}

// MARK: - Notifications

/// Settings → Account → Notifications: whether iOS lets Allternit notify,
/// with the one action that fits (ask, or open iOS Settings).
struct NotificationsSettingsView: View {
    @Environment(\.openURL) private var openURL
    @Environment(\.scenePhase) private var scenePhase
    @State private var status: UNAuthorizationStatus? = nil

    var body: some View {
        List {
            Section {
                HStack {
                    Text("Notifications")
                        .font(.subheadline)
                        .foregroundColor(Color("TextPrimary"))
                    Spacer()
                    if let status {
                        Text(Self.label(status))
                            .font(.subheadline)
                            .foregroundColor(Color("TextSecondary"))
                    } else {
                        ProgressView()
                    }
                }
                switch status {
                case .notDetermined:
                    Button("Turn on notifications") { Task { await request() } }
                        .font(.subheadline.weight(.semibold))
                case .denied:
                    Button("Open iOS Settings") {
                        if let url = URL(string: UIApplication.openSettingsURLString) { openURL(url) }
                    }
                    .font(.subheadline.weight(.semibold))
                default:
                    EmptyView()
                }
            } footer: {
                Text("Allternit notifies you when a bot finishes, needs an answer, or a computer goes offline.")
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .navigationTitle("Notifications")
        .navigationBarTitleDisplayMode(.inline)
        .task { await refresh() }
        .onChange(of: scenePhase) { _, phase in
            if phase == .active { Task { await refresh() } }
        }
    }

    @MainActor
    private func refresh() async {
        status = await UNUserNotificationCenter.current().notificationSettings().authorizationStatus
    }

    @MainActor
    private func request() async {
        let granted = (try? await UNUserNotificationCenter.current()
            .requestAuthorization(options: [.alert, .badge, .sound])) ?? false
        if granted { PushNotificationManager.shared.registerForRemoteNotifications() }
        await refresh()
    }

    private static func label(_ status: UNAuthorizationStatus) -> String {
        switch status {
        case .authorized: return "On"
        case .provisional: return "Quiet"
        case .ephemeral: return "On"
        case .denied: return "Off"
        case .notDetermined: return "Not set"
        @unknown default: return "Unknown"
        }
    }
}
