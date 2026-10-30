import ClerkKit
import ClerkKitUI
import SwiftUI

/// Why a native Settings page couldn't load, sorted the way the web panels
/// word it (memory-drive-client.ts `kindForStatus`, twin-api.ts
/// `twinErrorText`): signed out, no computer online, no network, or a plain
/// server error.
enum SettingsLoadFailure: Equatable, Sendable {
    case signedOut
    /// The data lives on the person's computer and none answered (relay
    /// `runtime_unavailable` / `runtime_offline`, or no computer paired).
    case computerOffline(String)
    /// The phone itself has no connection.
    case offline
    case error(String)

    static func from(_ error: Error) -> SettingsLoadFailure {
        if let relay = error as? RuntimeRelayError {
            switch relay {
            case .noComputer, .unreachable:
                return .computerOffline(relay.errorDescription ?? "")
            case .socketUnavailable(let message):
                return .error(message)
            }
        }
        if let api = error as? APIError {
            switch api {
            case .httpError(let status, let message):
                if status == 401 { return .signedOut }
                if status == 502 || status == 503 || status == 504 {
                    return .computerOffline(message ?? "")
                }
                return .error(message ?? "Something went wrong. Try again.")
            case .transport(let inner):
                return from(inner)
            case .decoding:
                return .error("This computer answered in a way the app doesn't understand. Update Allternit Desktop, then try again.")
            case .invalidResponse:
                return .error("The server returned an invalid response.")
            }
        }
        if error is URLError { return .offline }
        return .error(FriendlyErrorMessage.from(error.localizedDescription))
    }

    /// One sentence for an inline error under a form.
    var message: String {
        switch self {
        case .signedOut: return "Your Allternit session expired. Sign in again to continue."
        case .computerOffline(let detail):
            return detail.isEmpty ? Self.computerOfflineText : detail
        case .offline: return "Couldn't reach Allternit. Check your connection and try again."
        case .error(let message): return message
        }
    }

    static let computerOfflineText = "This lives on your computer, and none is online. Open Allternit Desktop or start your cloud computer, then try again."
}

/// The full-width state for a failed load, with a retry.
struct SettingsFailureView: View {
    let failure: SettingsLoadFailure
    /// What couldn't load, e.g. "your skills".
    let subject: String
    let retry: () -> Void

    var body: some View {
        switch failure {
        case .signedOut:
            SettingsSignedOutView(message: "Sign in to see \(subject).")
        case .computerOffline:
            FriendlyInlineStateView(
                style: .offline,
                icon: "desktopcomputer.trianglebadge.exclamationmark",
                title: "Your computer is offline",
                message: failure.message,
                actionTitle: "Try again",
                action: retry
            )
        case .offline:
            FriendlyInlineStateView(
                style: .offline,
                icon: "wifi.slash",
                title: "You're offline",
                message: failure.message,
                actionTitle: "Try again",
                action: retry
            )
        case .error:
            FriendlyInlineStateView(
                style: .error,
                icon: "exclamationmark.triangle",
                title: "Couldn't load \(subject)",
                message: failure.message,
                actionTitle: "Try again",
                action: retry
            )
        }
    }
}

/// Signed-out state with a Sign in button. Settings is itself a sheet, so the
/// app-level Clerk sheet can't present over it: this presents the same
/// AuthView from here.
struct SettingsSignedOutView: View {
    let message: String
    @EnvironmentObject private var authManager: AuthManager
    @State private var isPresentingAuth = false

    var body: some View {
        FriendlyInlineStateView(
            style: .empty,
            icon: "person.crop.circle.badge.exclamationmark",
            title: "Not signed in",
            message: message,
            actionTitle: authManager.isClerkConfigured ? "Sign in" : nil,
            action: authManager.isClerkConfigured ? { isPresentingAuth = true } : nil
        )
        .sheet(isPresented: $isPresentingAuth, onDismiss: { authManager.refreshAuthState() }) {
            if authManager.isClerkConfigured {
                AuthView(mode: .signIn)
                    .environment(Clerk.shared)
            }
        }
    }
}

extension AuthManager {
    /// Signed in, or the DEBUG `-skip-auth` bypass.
    var canCallAccountAPIs: Bool { isSignedIn || isSkippingAuth }
}

/// Percent-encodes one query value (keeps `/` and `~` readable, escapes
/// `&`, `=`, `+`, `#`, `?`).
func settingsQueryValue(_ value: String) -> String {
    var allowed = CharacterSet.urlQueryAllowed
    allowed.remove(charactersIn: "&=+#?")
    return value.addingPercentEncoding(withAllowedCharacters: allowed) ?? value
}

/// Percent-encodes one path segment.
func settingsPathSegment(_ value: String) -> String {
    var allowed = CharacterSet.urlPathAllowed
    allowed.remove(charactersIn: "/?#")
    return value.addingPercentEncoding(withAllowedCharacters: allowed) ?? value
}
