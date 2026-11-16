import LocalAuthentication
import SwiftUI
import AllternitFilesKit

// The three sheets Files opens when the File Provider answers
// `.notAuthenticated` (design 5B): wake consent, Face ID unlock, sign in.

/// The wake prompt's body, shared by the wake sheet and the action sheets
/// that need the computer awake (Summarize, Send to agent).
struct WakeCard: View {
    let computerName: String
    @Binding var remember: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            SheetText("\(computerName) is asleep. Waking it usually takes under a minute. While it is awake it bills credits at your plan's awake rate (the Allternit app shows the live rate). Browsing the saved list stays free.")
            Toggle(isOn: $remember) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Wake automatically from now on").font(.system(size: 15)).foregroundStyle(SheetColor.text)
                    Text("Change it in Allternit settings").font(.system(size: 13)).foregroundStyle(SheetColor.secondary)
                }
            }
            .tint(SheetColor.text)
        }
    }
}

/// Wakes the computer behind a pending wake, after the user agreed.
@MainActor
final class WakeRunner: ObservableObject {
    enum Phase: Equatable { case ask, waking, awake, failed(String) }

    @Published var phase: Phase = .ask
    @Published var remember = false

    /// Asks cloud-api for a wake ticket (the only call that wakes a
    /// computer, SPEC section 9). The ticket goes to the shared keychain, so
    /// the File Provider's next request carries it.
    func wake(_ context: SheetContext, hint: WakeHint?, reason: WakeReason, itemId: ItemId?) async -> Bool {
        guard let client = context.client else {
            phase = .failed("Sign in to the Allternit app first.")
            return false
        }
        phase = .waking
        do {
            if remember { _ = try? await client.setWakeSetting(.auto) }
            try await client.requestWake(reason: reason, itemId: itemId, hint: hint)
            phase = .awake
            return true
        } catch let error as FilesError where error.code == .wakeDenied {
            phase = .failed("Waking \(context.computerName) from Files is turned off. Change it in Allternit settings.")
        } catch {
            phase = .failed(error.localizedDescription)
        }
        return false
    }
}

struct WakeSheet: View {
    let context: SheetContext
    @StateObject private var runner = WakeRunner()

    private var pending: FilesPendingWake? {
        context.computerId.flatMap { context.store.pendingWake(computerId: $0) }
    }

    var body: some View {
        SheetScaffold(symbol: runner.phase == .awake ? "sun.max.fill" : "moon.zzz.fill",
                      tint: runner.phase == .awake ? SheetColor.warning : SheetColor.text,
                      title: runner.phase == .awake ? "Waking \(context.computerName)" : "Wake your cloud computer?",
                      subtitle: runner.phase == .awake ? nil : "\(context.computerName) is asleep") {
            switch runner.phase {
            case .awake:
                SheetText("Your file opens as soon as it is up. You can close this.")
            default:
                WakeCard(computerName: context.computerName, remember: $runner.remember)
                if case .failed(let message) = runner.phase { ErrorLine(text: message) }
            }
        } actions: {
            if runner.phase == .awake {
                SheetButton(title: "Done") { context.finish() }
            } else {
                SheetButton(title: "Wake", symbol: "bolt.fill", busy: runner.phase == .waking) {
                    Task {
                        if await runner.wake(context, hint: pending?.hint, reason: pending?.reason ?? .openFile,
                                             itemId: pending?.itemId) {
                            context.signalChanges()
                            try? await Task.sleep(nanoseconds: 900_000_000)
                            context.finish()
                        }
                    }
                }
                SheetButton(title: "Not now", primary: false) { context.cancel() }
            }
        }
    }
}

/// Face ID (or Touch ID, or the passcode) for a protected root. A success
/// opens protected roots on this computer for five minutes: requests carry
/// `X-Allternit-Unlock` (`FilesSharedStore.markUnlocked`).
struct UnlockSheet: View {
    let context: SheetContext
    @State private var failure: String?
    @State private var busy = false
    @State private var unlocked = false

    private var rootName: String {
        context.computerId.flatMap { context.store.pendingUnlock(computerId: $0)?.rootName } ?? "This folder"
    }

    private var method: (label: String, symbol: String) {
        let auth = LAContext()
        _ = auth.canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: nil)
        switch auth.biometryType {
        case .faceID: return ("Unlock with Face ID", "faceid")
        case .touchID: return ("Unlock with Touch ID", "touchid")
        case .opticID: return ("Unlock with Optic ID", "opticid")
        default: return ("Unlock with passcode", "lock.open.fill")
        }
    }

    var body: some View {
        SheetScaffold(symbol: unlocked ? "lock.open.fill" : "lock.fill",
                      tint: unlocked ? SheetColor.success : SheetColor.text,
                      title: unlocked ? "\(rootName) unlocked" : "Unlock \(rootName)",
                      subtitle: context.computerName) {
            if unlocked {
                SheetText("It stays open on this iPhone for 5 minutes.")
            } else {
                SheetText("\(rootName) is protected. Unlock it to open it on this iPhone. It stays open for 5 minutes, then locks again.")
                if let failure { ErrorLine(text: failure) }
            }
        } actions: {
            if !unlocked {
                SheetButton(title: method.label, symbol: method.symbol, busy: busy) { Task { await unlock() } }
                SheetButton(title: "Not now", primary: false) { context.cancel() }
            }
        }
        .task { await unlock() }
    }

    private func unlock() async {
        guard !busy, !unlocked, let computerId = context.computerId else { return }
        busy = true
        defer { busy = false }
        let auth = LAContext()
        auth.localizedCancelTitle = "Not now"
        var error: NSError?
        guard auth.canEvaluatePolicy(.deviceOwnerAuthentication, error: &error) else {
            failure = "Set a passcode on this iPhone to open protected folders."
            return
        }
        do {
            let ok = try await auth.evaluatePolicy(.deviceOwnerAuthentication,
                                                   localizedReason: "Unlock \(rootName) on \(context.computerName)")
            guard ok else { return }
            context.store.markUnlocked(computerId: computerId)
            unlocked = true
            context.signalChanges()
            try? await Task.sleep(nanoseconds: 700_000_000)
            context.finish()
        } catch let laError as LAError where laError.code == .userCancel || laError.code == .appCancel || laError.code == .systemCancel {
            failure = nil
        } catch {
            failure = "Couldn't unlock. Try again."
        }
    }
}

/// No Files token on this iPhone, or cloud-api refused it. The extension
/// can't sign in (Clerk lives in the app), so the sheet sends the user to
/// the app, which mints a new Files key on its next foreground
/// (`FilesDriveBridge`), and checks again when they come back.
struct SignInSheet: View {
    let context: SheetContext
    let message: String?
    @State private var note: String?

    private var expired: Bool { context.store.token() != nil && context.store.tokenRejected }

    var body: some View {
        SheetScaffold(symbol: "person.crop.circle.badge.exclamationmark",
                      title: expired ? "Reconnect Files" : "Sign in to Allternit",
                      subtitle: context.computerId == nil ? nil : context.computerName) {
            SheetText(expired
                      ? "Files needs a fresh connection to your account. Open Allternit once and it reconnects by itself."
                      : "Sign in to the Allternit app on this iPhone. Your computers then show up here as locations.")
            if let note { SheetText(note, secondary: false) }
        } actions: {
            SheetButton(title: "Open Allternit", symbol: "arrow.up.forward.app") {
                context.openApp(URL(string: "allternit://computers")!) { opened in
                    if !opened {
                        DispatchQueue.main.async { note = "Open the Allternit app from your Home Screen, then come back here." }
                    }
                }
            }
            SheetButton(title: "Try again", primary: false) {
                if context.store.token() != nil && !context.store.tokenRejected {
                    context.signalChanges()
                    context.finish()
                } else {
                    note = "Still not connected. Open Allternit first."
                }
            }
        }
    }
}
