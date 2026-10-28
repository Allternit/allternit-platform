import SwiftUI
import UIKit

/// Starts ring polling for the signed-in workspace and puts the call
/// screens in their own window above everything (sheets included), the
/// iPhone version of the web's full-screen `InappCallLayer`. Attached to
/// the workspace next to `DeepLinkHost`.
struct InappCallHost: ViewModifier {
    @ObservedObject private var store = InappCallStore.shared
    @Environment(\.scenePhase) private var scenePhase

    func body(content: Content) -> some View {
        content
            .onAppear {
                store.start()
                CallWindow.shared.sync(visible: store.ringing != nil || store.active != nil)
            }
            .onDisappear {
                store.stop()
                CallWindow.shared.sync(visible: false)
            }
            .onChange(of: scenePhase) { _, phase in
                store.setForeground(phase == .active)
            }
            .onChange(of: store.ringing?.room) { _, _ in
                CallWindow.shared.sync(visible: store.ringing != nil || store.active != nil)
            }
            .onChange(of: store.active?.room) { _, _ in
                CallWindow.shared.sync(visible: store.ringing != nil || store.active != nil)
            }
    }
}

/// An alert-level window that holds `InappCallScreen` while there is a ring
/// or a call; hidden (and released) otherwise so it never eats touches.
@MainActor
final class CallWindow {
    static let shared = CallWindow()
    private var window: UIWindow?

    private init() {}

    func sync(visible: Bool) {
        if visible {
            guard window == nil, let scene = Self.activeScene() else { return }
            let window = UIWindow(windowScene: scene)
            window.windowLevel = .alert + 1
            let host = UIHostingController(rootView: InappCallScreen())
            host.view.backgroundColor = .clear
            window.rootViewController = host
            window.overrideUserInterfaceStyle = AppearanceStore.shared.theme.style
            window.makeKeyAndVisible()
            self.window = window
            UIAccessibility.post(notification: .screenChanged, argument: nil)
        } else if let window {
            window.isHidden = true
            self.window = nil
            Self.activeScene()?.windows.first(where: { $0.windowLevel == .normal })?.makeKey()
        }
    }

    private static func activeScene() -> UIWindowScene? {
        let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
        return scenes.first(where: { $0.activationState == .foregroundActive }) ?? scenes.first
    }
}

/// Incoming call, the call itself, and how it ended.
struct InappCallScreen: View {
    @ObservedObject private var store = InappCallStore.shared
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        ZStack {
            Color("BgPrimary").ignoresSafeArea()
            if let call = store.active {
                activeView(call)
            } else if let ring = store.ringing {
                incomingView(ring)
            }
        }
    }

    // MARK: Incoming

    private func incomingView(_ ring: InappCallStore.IncomingCall) -> some View {
        VStack {
            header(label: ring.from.label, status: "Allternit call", pulsing: true)
            Spacer()
            HStack {
                roundButton(symbol: "phone.down.fill", color: .red, caption: "Decline",
                            label: "Decline call from \(ring.from.label)") {
                    store.decline(ring)
                }
                Spacer()
                roundButton(symbol: "phone.fill", color: .green, caption: "Answer",
                            label: "Answer call from \(ring.from.label)") {
                    Task { await store.answer(ring) }
                }
            }
            .frame(maxWidth: 280)
        }
        .padding(.horizontal, 24)
        .padding(.top, 96)
        .padding(.bottom, 48)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Incoming call from \(ring.from.label)")
    }

    // MARK: Active / ended

    @ViewBuilder
    private func activeView(_ call: InappCallStore.ActiveCall) -> some View {
        VStack {
            switch call.phase {
            case .live:
                TimelineView(.periodic(from: .now, by: 1)) { context in
                    header(label: call.label, status: elapsed(since: call.startedAt, now: context.date), pulsing: false)
                }
            default:
                header(label: call.label, status: statusText(call),
                       pulsing: call.phase == .connecting || call.phase == .calling)
            }
            if let detail = detailText(call) {
                Text(detail)
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
                    .multilineTextAlignment(.center)
                    .padding(.top, 12)
                    .padding(.horizontal, 12)
            }
            Spacer()
            controls(call)
        }
        .padding(.horizontal, 24)
        .padding(.top, 96)
        .padding(.bottom, 48)
    }

    @ViewBuilder
    private func controls(_ call: InappCallStore.ActiveCall) -> some View {
        switch call.phase {
        case .calling:
            HStack(spacing: 28) {
                toggleButton(symbol: "speaker.wave.2.fill", on: call.speaker, caption: "Speaker",
                             label: call.speaker ? "Turn speaker off" : "Turn speaker on") {
                    store.setSpeaker(!call.speaker)
                }
                roundButton(symbol: "phone.down.fill", color: .red, caption: "Cancel",
                            label: "Cancel call to \(call.label)") {
                    store.hangUp()
                }
            }
        case .connecting, .live:
            HStack(spacing: 28) {
                toggleButton(symbol: call.muted ? "mic.slash.fill" : "mic.fill", on: call.muted,
                             caption: call.muted ? "Unmute" : "Mute",
                             label: call.muted ? "Unmute microphone" : "Mute microphone") {
                    store.setMuted(!call.muted)
                }
                .disabled(call.phase != .live)
                toggleButton(symbol: "speaker.wave.2.fill", on: call.speaker, caption: "Speaker",
                             label: call.speaker ? "Turn speaker off" : "Turn speaker on") {
                    store.setSpeaker(!call.speaker)
                }
                roundButton(symbol: "phone.down.fill", color: .red,
                            caption: call.outgoing && call.phase == .connecting ? "Cancel" : "End",
                            label: call.outgoing && call.phase == .connecting
                                ? "Cancel call to \(call.label)" : "Hang up call with \(call.label)") {
                    store.hangUp()
                }
            }
        case .micDenied:
            VStack(spacing: 12) {
                primaryButton("Open Settings") { store.openSystemSettings() }
                secondaryButton(call.outgoing ? "Not now" : "Decline") { store.declineActive() }
            }
        case .unavailable:
            VStack(spacing: 12) {
                if call.outgoing {
                    secondaryButton("Done") { store.dismiss() }
                } else {
                    secondaryButton("Decline") { store.declineActive() }
                    secondaryButton("Not now") { store.dismiss() }
                }
            }
        case .missed, .ended:
            VStack(spacing: 12) {
                if !call.threadId.isEmpty, !call.botId.isEmpty {
                    primaryButton("Open conversation") { store.openConversation() }
                }
                secondaryButton("Done") { store.dismiss() }
            }
        }
    }

    private func statusText(_ call: InappCallStore.ActiveCall) -> String {
        switch call.phase {
        case .calling: return "Calling…"
        case .connecting: return "Connecting…"
        case .live: return "On call"
        case .ended: return call.endTitle ?? "Call ended"
        case .missed: return "Missed call"
        case .micDenied: return "Microphone is off"
        case .unavailable: return call.outgoing ? "Can't call here" : "Can't answer here"
        }
    }

    private func detailText(_ call: InappCallStore.ActiveCall) -> String? {
        switch call.phase {
        case .ended:
            return call.note
        case .missed:
            return "\(call.label) called while you were away."
        case .micDenied:
            return call.outgoing
                ? "To call, allow the microphone for Allternit in Settings."
                : "To answer calls, allow the microphone for Allternit in Settings. The call is still ringing on your other devices."
        case .unavailable:
            return call.outgoing
                ? "This version of the iPhone app can't carry call audio yet. Call from ai.allternit.com or Allternit Desktop."
                : "This version of the iPhone app can't carry call audio yet. Answer on ai.allternit.com or Allternit Desktop — it's still ringing there."
        case .calling:
            return "Allternit call"
        case .connecting, .live:
            return nil
        }
    }

    private func elapsed(since start: Date?, now: Date) -> String {
        guard let start else { return "On call" }
        let seconds = max(0, Int(now.timeIntervalSince(start)))
        return String(format: "%d:%02d", seconds / 60, seconds % 60)
    }

    // MARK: Pieces

    private func header(label: String, status: String, pulsing: Bool) -> some View {
        VStack(spacing: 12) {
            PulsingInitial(label: label, pulsing: pulsing && !reduceMotion)
            Text(label)
                .font(.system(size: 26, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
                .multilineTextAlignment(.center)
            Text(status)
                .font(.subheadline.monospacedDigit())
                .foregroundColor(Color("TextTertiary"))
                .accessibilityAddTraits(.updatesFrequently)
        }
    }

    private func roundButton(symbol: String, color: Color, caption: String, label: String,
                             action: @escaping () -> Void) -> some View {
        VStack(spacing: 8) {
            Button(action: action) {
                Image(systemName: symbol)
                    .font(.system(size: 26, weight: .semibold))
                    .foregroundColor(.white)
                    .frame(width: 68, height: 68)
                    .background(Circle().fill(color))
            }
            .accessibilityLabel(label)
            Text(caption)
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
                .accessibilityHidden(true)
        }
    }

    private func toggleButton(symbol: String, on: Bool, caption: String, label: String,
                              action: @escaping () -> Void) -> some View {
        VStack(spacing: 8) {
            Button(action: action) {
                Image(systemName: symbol)
                    .font(.system(size: 22, weight: .medium))
                    .foregroundColor(on ? Color("BgPrimary") : Color("TextPrimary"))
                    .frame(width: 60, height: 60)
                    .background(Circle().fill(on ? Color("TextPrimary") : Color("BgSecondary")))
            }
            .accessibilityLabel(label)
            .accessibilityAddTraits(on ? .isSelected : [])
            Text(caption)
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
                .accessibilityHidden(true)
        }
    }

    private func primaryButton(_ title: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title)
                .font(.headline)
                .foregroundColor(.white)
                .frame(maxWidth: 280)
                .padding(.vertical, 14)
                .background(Color("AccentPrimary"))
                .clipShape(Capsule())
        }
    }

    private func secondaryButton(_ title: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title)
                .font(.headline)
                .foregroundColor(Color("TextPrimary"))
                .frame(maxWidth: 280)
                .padding(.vertical, 14)
                .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
        }
    }
}

/// The caller's initial in a neutral circle; pulses while ringing.
private struct PulsingInitial: View {
    let label: String
    let pulsing: Bool
    @State private var expanded = false

    var body: some View {
        Text(String(label.trimmingCharacters(in: .whitespaces).first ?? "?").uppercased())
            .font(.system(size: 34, weight: .medium))
            .foregroundColor(Color("TextPrimary"))
            .frame(width: 96, height: 96)
            .background(Circle().fill(Color("BgSecondary")))
            .scaleEffect(pulsing && expanded ? 1.06 : 1)
            .animation(pulsing ? .easeInOut(duration: 0.9).repeatForever(autoreverses: true) : .default, value: expanded)
            .onAppear { expanded = pulsing }
            .onChange(of: pulsing) { _, now in expanded = now }
            .accessibilityHidden(true)
    }
}
