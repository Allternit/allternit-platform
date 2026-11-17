import Foundation

/// The Phone Use result envelope (docs/contracts/CONTRACT-phone-use-2026-11-16.md
/// §4): `{ok, route, verified, detail, needs_you?}`. The app answers every new
/// `device.*` request (shortcuts, Home, describe, screen read) with one of these
/// as the control socket's `result`; the broadcast extension sends one for
/// `device.screen.read`. On this phone the route is always `official`.
///
/// Compiled into the app and AllternitScreen (Shared/AppPhone): Foundation only.
enum PhoneUseEnvelope {
    static let route = "official"

    /// The contract's `NeedsYouCode` values this phone can raise.
    enum NeedsYou: String, Sendable {
        case shortcutNotInstalled = "SHORTCUT_NOT_INSTALLED"
        case broadcastStopped = "BROADCAST_STOPPED"
        case appNotForeground = "APP_NOT_FOREGROUND"
    }

    /// The fixed user-facing sentence for each code (contract §5).
    static func sentence(_ code: NeedsYou, name: String? = nil) -> String {
        switch code {
        case .shortcutNotInstalled:
            return "Add the \"\(name ?? "Allternit Actions")\" shortcut in Allternit Settings > Device Access > iPhone actions."
        case .broadcastStopped:
            return "Start the screen broadcast in the Allternit app on your iPhone."
        case .appNotForeground:
            return "Open the Allternit app on your iPhone."
        }
    }

    /// A finished action or read. `verified` is true only when the result
    /// itself confirms the effect (contract §4).
    static func ok(_ detail: [String: Any], verified: Bool = true) -> [String: Any] {
        ["ok": true, "route": route, "verified": verified, "detail": detail]
    }

    /// A failure with a short code and a plain sentence; no user step.
    static func failure(_ error: String, _ message: String, extra: [String: Any] = [:]) -> [String: Any] {
        var detail = extra
        detail["error"] = error
        detail["message"] = message
        return ["ok": false, "route": route, "verified": false, "detail": detail]
    }

    /// The person has to do something first.
    static func needsYou(_ code: NeedsYou, name: String? = nil, error: String? = nil,
                         extra: [String: Any] = [:]) -> [String: Any] {
        let message = sentence(code, name: name)
        var detail = extra
        if let error { detail["error"] = error }
        detail["message"] = message
        return ["ok": false, "route": route, "verified": false, "detail": detail,
                "needs_you": ["code": code.rawValue, "message": message]]
    }
}
