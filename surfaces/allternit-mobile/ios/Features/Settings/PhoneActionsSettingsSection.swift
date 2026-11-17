import SwiftUI

/// Settings > Device Access > iPhone actions (Phone Use, plan step 5): the
/// shortcuts a bot can run on this iPhone and Home access. No rail tab: it
/// is a section of the Device Access page.
///
/// Apple gives apps no way to install a shortcut or list installed ones, so
/// each row opens the shortcut's iCloud link and the person taps Add
/// Shortcut once. A row says "Added" after that shortcut first returns a
/// result here. Links are published by the owner (gate G3); until then a row
/// shows "Link coming".
struct PhoneActionsSettingsSection: View {
    @ObservedObject private var access = DeviceAccessStore.shared
    @Environment(\.openURL) private var openURL
    @State private var showAllApps = false

    private let catalog = PhoneActionsCatalog.shared
    private static let collapsedCount = 6

    var body: some View {
        SettingsSectionHeading(text: "iPhone actions")
        SettingsDescription(text: "Bots use Apple's own actions on this iPhone before anything else: shortcuts you add here, and Home. Sending a message, mail or call, buying something, and unlocking a door always need your approval on your computer.")

        homeCard

        SettingsSectionHeading(text: "Add shortcuts")
        SettingsDescription(text: "Each shortcut opens in the Shortcuts app; tap Add Shortcut once. The app shortcuts are optional: add the ones for apps you use.")

        SettingsCardBox {
            shortcutRow(
                title: catalog.actions.name,
                detail: "Messages, Mail, calls, Notes, Reminders, Calendar, Maps, Music, timers and phone settings. Built-in Apple actions only.",
                name: catalog.actions.name,
                link: catalog.actions.linkURL)
        }

        if !catalog.apps.isEmpty {
            SettingsCardBox {
                ForEach(visibleApps) { app in
                    shortcutRow(
                        title: app.name,
                        detail: "\(app.actionCount) action\(app.actionCount == 1 ? "" : "s") · optional",
                        name: app.shortcut,
                        link: app.linkURL)
                }
                if catalog.apps.count > Self.collapsedCount {
                    Button(showAllApps ? "Show fewer" : "Show all \(catalog.apps.count) apps") {
                        showAllApps.toggle()
                    }
                    .buttonStyle(SettingsQuietButtonStyle())
                }
            }
        }
    }

    private var visibleApps: [PhoneActionsCatalog.App] {
        showAllApps ? catalog.apps : Array(catalog.apps.prefix(Self.collapsedCount))
    }

    @ViewBuilder
    private func shortcutRow(title: String, detail: String, name: String, link: URL?) -> some View {
        SettingsLabeledRow(title: title, detail: detail) {
            if ShortcutBridge.shared.knownShortcuts.contains(name) {
                Text("Added")
                    .font(.system(size: 13, weight: .medium))
                    .foregroundColor(Color("TextSecondary"))
            } else if let link {
                Button("Add") { openURL(link) }
                    .buttonStyle(SettingsQuietButtonStyle())
                    .accessibilityLabel("Add the \(name) shortcut")
            } else {
                Text("Link coming")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
            }
        }
    }

    // MARK: - Home

    private var homeCard: some View {
        SettingsCardBox {
            SettingsLabeledRow(title: "Home access", detail: homeStatus) {
                homeControl
            }
        }
    }

    private var homeState: DeviceSystemAccess { access.system[.home] ?? .notDetermined }

    private var homeStatus: String {
        guard HomeAccessService.isBuiltIn else { return "Home arrives in the next Allternit update." }
        switch homeState {
        case .granted:
            return access.enabled[.home] == true ? "On. Bots can use your Home accessories." : "iOS access allowed. Turn on Home above to let bots use it."
        case .notDetermined: return "Not asked yet."
        case .denied: return "Denied in iOS. Bots cannot use Home until you allow it."
        case .restricted: return "Blocked on this iPhone (Screen Time or a profile)."
        }
    }

    @ViewBuilder
    private var homeControl: some View {
        if HomeAccessService.isBuiltIn {
            switch homeState {
            case .notDetermined:
                Button("Allow") { Task { await access.setEnabled(.home, true) } }
                    .buttonStyle(SettingsQuietButtonStyle())
                    .accessibilityLabel("Allow Home access")
            case .denied:
                Button("Open Settings") {
                    if let url = URL(string: UIApplication.openSettingsURLString) { openURL(url) }
                }
                .buttonStyle(SettingsQuietButtonStyle())
            case .granted, .restricted:
                EmptyView()
            }
        }
    }
}
