import SwiftUI
import UIKit
import LocalAuthentication

/// Face ID lock (SPEC-hermes-parity section 5). Off by default. When on, the
/// app locks at launch and each time it goes to the background, and unlocks
/// with Face ID, falling back to the device passcode. The lock screen lives
/// in its own window so it also covers sheets and the app switcher snapshot.
@MainActor
final class AppLockStore: ObservableObject {
    static let shared = AppLockStore()

    private static let defaultsKey = "allternit-face-id-lock"

    @Published private(set) var isEnabled: Bool
    @Published private(set) var isLocked: Bool
    /// Shown on the lock screen and under the Settings toggle.
    @Published private(set) var message: String?

    private var covered = false
    private var authenticating = false
    private var window: UIWindow?
    private weak var previousKeyWindow: UIWindow?

    private init() {
        let enabled = UserDefaults.standard.bool(forKey: Self.defaultsKey)
        isEnabled = enabled
        isLocked = enabled
    }

    enum AuthResult { case success, cancelled, unavailable(String), failed }

    // MARK: - Scene

    /// First appearance of the app: show the lock and ask to unlock.
    func appeared() {
        refreshWindow()
        if isLocked { Task { await unlock() } }
    }

    func sceneChanged(_ phase: ScenePhase) {
        guard isEnabled else { return }
        switch phase {
        case .active:
            covered = false
            refreshWindow()
            if isLocked { Task { await unlock() } }
        case .inactive:
            covered = true
            refreshWindow()
        case .background:
            covered = true
            isLocked = true
            refreshWindow()
        @unknown default:
            break
        }
    }

    // MARK: - Unlock

    func unlock() async {
        guard isLocked, !authenticating else { return }
        authenticating = true
        message = nil
        defer { authenticating = false }
        switch await Self.authenticate(reason: "Unlock Allternit") {
        case .success:
            isLocked = false
            covered = false
            refreshWindow()
        case .cancelled:
            break
        case .failed:
            message = "That didn't work. Try again."
        case .unavailable:
            // Nothing left to unlock with (the passcode was removed): never trap the owner.
            UserDefaults.standard.set(false, forKey: Self.defaultsKey)
            isEnabled = false
            isLocked = false
            covered = false
            refreshWindow()
        }
    }

    // MARK: - Settings

    /// Turns the lock on or off after a Face ID / passcode check.
    func setEnabled(_ on: Bool) async {
        message = nil
        guard on != isEnabled else { return }
        switch await Self.authenticate(reason: on ? "Turn on the Allternit lock" : "Turn off the Allternit lock") {
        case .success:
            UserDefaults.standard.set(on, forKey: Self.defaultsKey)
            isEnabled = on
            isLocked = false
        case .unavailable(let reason):
            message = reason
        case .failed:
            message = "That didn't work. The lock was not changed."
        case .cancelled:
            break
        }
    }

    // MARK: - LocalAuthentication

    nonisolated private static func authenticate(reason: String) async -> AuthResult {
        let context = LAContext()
        var error: NSError?
        guard context.canEvaluatePolicy(.deviceOwnerAuthentication, error: &error) else {
            return .unavailable("Set a passcode on this iPhone first, then turn the lock on.")
        }
        do {
            let ok = try await context.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: reason)
            return ok ? .success : .failed
        } catch let laError as LAError {
            switch laError.code {
            case .userCancel, .appCancel, .systemCancel: return .cancelled
            case .passcodeNotSet: return .unavailable("Set a passcode on this iPhone first, then turn the lock on.")
            default: return .failed
            }
        } catch {
            return .failed
        }
    }

    // MARK: - Lock window

    private func refreshWindow() {
        let shouldShow = isEnabled && (isLocked || covered)
        if shouldShow {
            guard window == nil,
                  let scene = UIApplication.shared.connectedScenes.compactMap({ $0 as? UIWindowScene }).first else { return }
            previousKeyWindow = scene.windows.first { $0.isKeyWindow }
            let host = UIHostingController(rootView: AppLockScreen(store: self))
            host.view.backgroundColor = .clear
            let lockWindow = UIWindow(windowScene: scene)
            lockWindow.windowLevel = .alert + 1
            lockWindow.rootViewController = host
            lockWindow.makeKeyAndVisible()
            window = lockWindow
        } else if let lockWindow = window {
            lockWindow.isHidden = true
            window = nil
            previousKeyWindow?.makeKey()
        }
    }
}

/// The lock screen: plain, no account data.
struct AppLockScreen: View {
    @ObservedObject var store: AppLockStore

    var body: some View {
        ZStack {
            Color("BgPrimary").ignoresSafeArea()
            VStack(spacing: 14) {
                Image(systemName: "lock.fill")
                    .font(.system(size: 34))
                    .foregroundColor(Color("TextSecondary"))
                    .accessibilityHidden(true)
                Text("Allternit is locked")
                    .font(.title3.weight(.semibold))
                    .foregroundColor(Color("TextPrimary"))
                if let message = store.message {
                    Text(message)
                        .font(.footnote)
                        .foregroundColor(Color("TextSecondary"))
                        .multilineTextAlignment(.center)
                }
                if store.isLocked {
                    Button { Task { await store.unlock() } } label: {
                        Label("Unlock", systemImage: "faceid")
                            .font(.body.weight(.semibold))
                            .foregroundColor(Color("TextPrimary"))
                            .padding(.horizontal, 22)
                            .frame(minHeight: 48)
                            .background(Capsule().fill(Color("BgPanel")))
                            .overlay(Capsule().stroke(Color("BorderSubtle")))
                    }
                    .buttonStyle(.plain)
                    .accessibilityHint("Uses Face ID or your passcode")
                }
            }
            .padding(32)
        }
    }
}
