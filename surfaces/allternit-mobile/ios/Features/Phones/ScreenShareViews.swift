import ReplayKit
import SwiftUI
import UIKit

/// The system broadcast picker, limited to the AllternitScreen extension and
/// with the microphone button hidden (the mic is never sent). Starting a
/// broadcast is always the person's tap on the system sheet this opens.
struct BroadcastPickerView: UIViewRepresentable {
    /// Set by the view so a styled SwiftUI label can open the same picker.
    let onMake: (RPSystemBroadcastPickerView) -> Void

    func makeUIView(context: Context) -> RPSystemBroadcastPickerView {
        let picker = RPSystemBroadcastPickerView(frame: CGRect(x: 0, y: 0, width: 44, height: 44))
        picker.preferredExtension = AppPhoneSharedState.extensionBundleId
        picker.showsMicrophoneButton = false
        onMake(picker)
        return picker
    }

    func updateUIView(_ uiView: RPSystemBroadcastPickerView, context: Context) {}
}

/// "Share screen" / "Stop sharing" as one button in the app's style. Start
/// opens the system picker (its own button, sent the tap); Stop ends the
/// broadcast through the extension.
struct ScreenShareButton: View {
    @ObservedObject private var phone = AppPhoneService.shared
    @State private var picker: RPSystemBroadcastPickerView?

    var body: some View {
        Button(action: tap) {
            HStack(spacing: 10) {
                Image(systemName: phone.isScreenLive ? "stop.circle.fill" : "rectangle.on.rectangle")
                    .font(.subheadline)
                    .frame(width: 20)
                Text(phone.isScreenLive ? "Stop sharing the screen" : "Share screen")
                    .font(.subheadline)
                Spacer()
            }
            .foregroundColor(phone.isScreenLive ? Theme.statusError : Color("TextPrimary"))
            .contentShape(Rectangle())
        }
        .background(
            // Kept in the hierarchy (the picker only presents from a window)
            // but invisible: the row above is what people see and tap.
            BroadcastPickerView { made in DispatchQueue.main.async { picker = made } }
                .frame(width: 1, height: 1)
                .opacity(0.01)
                .allowsHitTesting(false)
                .accessibilityHidden(true)
        )
    }

    private func tap() {
        if phone.isScreenLive {
            phone.stopScreenShare()
            return
        }
        guard let picker else { return }
        for case let button as UIButton in picker.subviews {
            button.sendActions(for: .touchUpInside)
        }
    }
}

/// Shown when a bot asks to see the screen (`screen.prompt`). It explains
/// what sharing does; the person decides on the system sheet.
struct ScreenSharePromptSheet: View {
    @ObservedObject private var phone = AppPhoneService.shared
    @Environment(\.dismiss) private var dismiss

    private let theme = ModeTheme(mode: .browser)

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack {
                Image(systemName: "eye")
                    .font(.title3)
                    .foregroundColor(theme.accent)
                Text("Your bot wants to see this screen")
                    .font(.headline)
                    .foregroundColor(Color("TextPrimary"))
                Spacer()
                Button(action: { dismiss() }) {
                    Image(systemName: "xmark.circle.fill")
                        .font(.title2)
                        .foregroundColor(Color("TextSecondary"))
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Not now")
            }

            Text("Sharing sends what's on this iPhone's screen, and app sound, to \(phone.credentials?.runtimeName ?? "your Desktop") until you stop. The microphone is never sent. iOS shows a red indicator the whole time, and you can stop from the banner at the top of Allternit or from Control Center.")
                .font(.subheadline)
                .foregroundColor(Color("TextSecondary"))
                .fixedSize(horizontal: false, vertical: true)

            ScreenShareButton()
                .padding(.horizontal, 14)
                .frame(height: 48)
                .background(Color("BgSecondary"))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                .overlay(
                    RoundedRectangle(cornerRadius: Theme.radiusMD)
                        .stroke(Color("BorderSubtle"), lineWidth: 1)
                )

            Spacer(minLength: 0)
        }
        .padding(20)
        .background(Color("BgPrimary"))
    }
}

/// The app-wide parts of the iPhone link: the "Your bot can see this screen"
/// banner with Stop while broadcasting (guideline 2.5.14), the share prompt
/// a bot can raise, and the control socket's foreground lifecycle. Applied
/// once to the workspace in AllternitApp.
struct AppPhoneOverlays: ViewModifier {
    @ObservedObject private var phone = AppPhoneService.shared
    @ObservedObject private var bridge = ACIBrowserBridge.shared
    @EnvironmentObject private var modeStore: AppModeStore
    @Environment(\.scenePhase) private var scenePhase

    func body(content: Content) -> some View {
        content
            .safeAreaInset(edge: .top, spacing: 0) {
                if phone.isScreenLive {
                    screenBanner
                        .transition(.move(edge: .top).combined(with: .opacity))
                }
            }
            .animation(.easeOut(duration: 0.2), value: phone.isScreenLive)
            .sheet(isPresented: $phone.isScreenPromptPresented) {
                ScreenSharePromptSheet()
                    .presentationDetents([.height(340)])
                    .presentationDragIndicator(.visible)
            }
            .onAppear { phone.setForeground(scenePhase == .active) }
            .onChange(of: scenePhase) { _, newPhase in
                phone.setForeground(newPhase == .active)
            }
            .onChange(of: bridge.requestedURL) { _, url in
                // A bot opened the browser: bring the ACI tab forward so
                // ACITabView can show it.
                if url != nil { modeStore.selectBarItem(.aci) }
            }
    }

    private var screenBanner: some View {
        HStack(spacing: 8) {
            Circle()
                .fill(Theme.statusError)
                .frame(width: 8, height: 8)
            Text("Your bot can see this screen")
                .font(.system(size: 13, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
            Spacer()
            Button("Stop") { phone.stopScreenShare() }
                .font(.system(size: 13, weight: .semibold))
                .foregroundColor(Theme.statusError)
                .buttonStyle(.plain)
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

extension View {
    func appPhoneOverlays() -> some View { modifier(AppPhoneOverlays()) }
}
