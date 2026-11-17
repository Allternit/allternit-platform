import SwiftUI

/// Settings > Device Access > "Be a keyboard for another iPhone" (Phone Use
/// route `bt-apple`, KeyboardModeService). No rail tab: a section of the
/// Device Access page. Start and Stop are the person's; the banner
/// (KeyboardModeBanner) shows while it runs, anywhere in the app.
struct KeyboardModeSettingsSection: View {
    @ObservedObject private var keyboard = KeyboardModeService.shared
    @ObservedObject private var phone = AppPhoneService.shared
    @Environment(\.openURL) private var openURL

    var body: some View {
        SettingsSectionHeading(text: "Be a keyboard for another iPhone")
        SettingsDescription(text: "This device shows up as a Bluetooth keyboard and mouse called \(KeyboardModeService.localName). Pair your main iPhone with it, and your assistant can move the pointer, tap and type on that iPhone while its screen is shared with Allternit. It never controls this device. Sending anything still needs your approval on your computer.")

        SettingsCardBox {
            SettingsLabeledRow(title: "Allternit Keyboard", detail: stateLine) {
                if keyboard.isRunning {
                    Button("Stop") { keyboard.stop() }
                        .buttonStyle(SettingsQuietButtonStyle())
                        .accessibilityLabel("Stop being a keyboard")
                } else {
                    Button("Start") { keyboard.start() }
                        .buttonStyle(SettingsPrimaryButtonStyle())
                        .accessibilityLabel("Start being a keyboard")
                }
            }

            if case .failed(let e) = keyboard.mode, e.code == "unauthorized" {
                Button("Open Settings") {
                    if let url = URL(string: UIApplication.openSettingsURLString) { openURL(url) }
                }
                .buttonStyle(SettingsQuietButtonStyle())
            }

            VStack(alignment: .leading, spacing: 6) {
                step("On the other iPhone: Settings > Bluetooth > \(KeyboardModeService.localName).")
                step("Turn on AssistiveTouch there too (Settings > Accessibility > Touch), so the pointer shows.")
                step("Keep Allternit open on this device while your assistant works. The screen stays on until you stop.")
                if !phone.isPaired {
                    step("Connect this device to your computer first under Let your bots use this iPhone; that is how your assistant reaches the keyboard.")
                }
            }

            Text("This uses Bluetooth in a way Apple does not document. An Apple update may stop it working; your assistant then uses another way to reach your iPhone.")
                .font(.system(size: 13))
                .foregroundColor(Color("TextSecondary"))
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func step(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 13))
            .foregroundColor(Color("TextPrimary"))
            .fixedSize(horizontal: false, vertical: true)
    }

    private var stateLine: String {
        KeyboardModeStateText.line(keyboard.linkState)
    }
}

/// The one-line state, shared by the settings row and the banner.
enum KeyboardModeStateText {
    static func line(_ s: KeyboardLinkState) -> String {
        switch s.availability {
        case .ready: return s.connected ? "On · your iPhone is connected." : "On · waiting for your iPhone to connect."
        case .off: return "Off."
        case .starting: return "Starting…"
        case .refused: return "Not available on this iPhone or iPad: iOS refused the keyboard. An Apple update may have changed this."
        case .bluetoothOff: return "Turn on Bluetooth on this device."
        case .noPermission: return "Allow Bluetooth for Allternit in Settings to use this."
        case .unsupported: return "This device has no Bluetooth Low Energy."
        case .failed: return s.message ?? "It could not start. Try again."
        }
    }

    static func short(_ s: KeyboardLinkState) -> String {
        switch s.availability {
        case .ready: return s.connected ? "Connected" : "Waiting for your iPhone"
        case .starting: return "Starting…"
        default: return "Not working"
        }
    }
}

/// "Allternit Keyboard is on" with Stop, at the top of the app while keyboard
/// mode runs (applied in AppPhoneOverlays).
struct KeyboardModeBanner: View {
    @ObservedObject private var keyboard = KeyboardModeService.shared

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "keyboard")
                .font(.system(size: 12, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
                .accessibilityHidden(true)
            Text("\(KeyboardModeService.localName) is on · \(KeyboardModeStateText.short(keyboard.linkState))")
                .font(.system(size: 13, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(1)
            Spacer()
            Button("Stop") { keyboard.stop() }
                .font(.system(size: 13, weight: .semibold))
                .foregroundColor(Theme.statusError)
                .buttonStyle(.plain)
                .accessibilityLabel("Stop being a keyboard")
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
        .background(Color("BgSecondary"))
        .overlay(alignment: .bottom) {
            Rectangle().fill(Color("BorderSubtle")).frame(height: 1)
        }
        .accessibilityElement(children: .contain)
    }
}
