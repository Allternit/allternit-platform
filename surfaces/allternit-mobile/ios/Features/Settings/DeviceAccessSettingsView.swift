import SwiftUI

/// Settings > Device Access: which parts of this iPhone your bots may reach.
/// Calendar, Reminders, Contacts, Shortcuts and Home (no Health). Each switch
/// is off until you turn it on, and shows what iOS itself says about access.
/// The iPhone actions section below (PhoneActionsSettingsSection) adds the
/// shortcuts and Home access.
struct DeviceAccessSettingsView: View {
    @ObservedObject private var access = DeviceAccessStore.shared
    @Environment(\.scenePhase) private var scenePhase
    @Environment(\.openURL) private var openURL

    var body: some View {
        PhoneSettingsPage(title: "Device Access") {
            SettingsDescription(text: "Let your bots use parts of this iPhone. Each one is off until you turn it on, and you can turn it off at any time.")

            ForEach(DeviceAccessKind.allCases) { kind in
                SettingsCardBox {
                    SettingsLabeledRow(title: kind.title, detail: kind.detail) {
                        Toggle(kind.title, isOn: binding(kind))
                            .labelsHidden()
                            .tint(Color("BrandAccent"))
                            .disabled(kind == .home && !HomeAccessService.isBuiltIn)
                    }
                    statusLine(kind)
                }
            }

            SettingsDescription(text: "Bots reach your iPhone only while the Allternit app is open on it. If the app is in the background, a bot is told to ask you to open it. Adding an event or reminder, sending anything and changing a lock always need your approval on your computer first.")

            PhoneActionsSettingsSection()

            if KeyboardModeService.isBuiltIn {
                KeyboardModeSettingsSection()
            }
        }
        .onAppear { access.refresh() }
        .onChange(of: scenePhase) { _, phase in
            if phase == .active { access.refresh() }
        }
    }

    private func binding(_ kind: DeviceAccessKind) -> Binding<Bool> {
        Binding(
            get: { access.enabled[kind] == true },
            set: { on in Task { await access.setEnabled(kind, on) } }
        )
    }

    @ViewBuilder
    private func statusLine(_ kind: DeviceAccessKind) -> some View {
        let system = access.system[kind] ?? .notDetermined
        if kind == .home && !HomeAccessService.isBuiltIn {
            Text("Home arrives in the next Allternit update.")
                .font(.system(size: 13))
                .foregroundColor(Color("TextSecondary"))
        } else if kind == .shortcuts {
            Text(access.enabled[kind] == true ? "On. Add shortcuts under iPhone actions below." : "Off.")
                .font(.system(size: 13))
                .foregroundColor(Color("TextSecondary"))
        } else {
            switch system {
            case .granted:
                Text(access.enabled[kind] == true ? "On. iOS access allowed." : "Off. iOS access allowed.")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
            case .notDetermined:
                Text("Not asked yet. iOS asks when you turn it on.")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
            case .denied, .restricted:
                VStack(alignment: .leading, spacing: 8) {
                    Text(system == .restricted
                         ? "Blocked on this iPhone (Screen Time or a profile). Bots cannot use \(kind.title)."
                         : "Denied in iOS. Bots cannot use \(kind.title) until you allow it.")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                    if system == .denied {
                        Button("Open Settings") {
                            if let url = URL(string: UIApplication.openSettingsURLString) { openURL(url) }
                        }
                        .buttonStyle(SettingsQuietButtonStyle())
                    }
                }
            }
        }
    }
}
