import SwiftUI

/// A reusable polished empty / error / offline state.
/// Used across tab surfaces so every list and feed reads consistently when
/// there is no data or the backend cannot be reached.
struct FriendlyStateView: View {
    enum Style {
        case empty
        case error
        case offline
    }

    var style: Style = .empty
    var icon: String = "tray"
    let title: String
    var message: String = ""
    var actionTitle: String? = nil
    var action: (() -> Void)? = nil
    /// Line art in place of the icon tile (empty style only). Nil keeps the tile.
    var art: EmptyArtKind? = nil
    /// Art size: full 96 pt, or `.panel` (64 pt) inside a card or list section.
    var size: EmptyArtSize = .full
    var paper: Color = Color("BgPrimary")
    /// Connect scene in place of the art / icon tile (no-computer states).
    /// A compute-gate refusal gets the asleep scene on its own.
    var scene: ConnectSceneState? = nil

    /// What actually shows: error text made calm, and a compute-gate refusal
    /// turned into the no-computer empty state (contract C5).
    private var shown: CalmStateText {
        CalmStateText(isError: style != .empty, icon: icon, title: title, message: message)
    }

    private var effectiveStyle: Style { shown.isNoComputer ? .empty : style }

    /// The connect kit, from the no-computer state's Connect (contract C4).
    @State private var isConnectPresented = false

    /// No computer: Connect opens the connect kit instead of the caller's
    /// Retry (a retry can't help until a computer answers).
    private var shownActionTitle: String? { shown.isNoComputer ? "Connect" : actionTitle }
    private var shownAction: (() -> Void)? {
        shown.isNoComputer ? { isConnectPresented = true } : action
    }

    var body: some View {
        let shown = self.shown
        VStack(spacing: 16) {
            if let scene {
                ConnectScene(state: scene)
            } else if shown.isNoComputer {
                ConnectScene(state: .asleep)
            } else if let art, effectiveStyle == .empty {
                EmptyArt(kind: art, size: size, paper: paper)
            } else {
                Image(systemName: shown.icon)
                    .font(.system(size: 28, weight: .medium))
                    .foregroundColor(foregroundColor)
                    .frame(width: 64, height: 64)
                    .background(backgroundColor)
                    .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG, style: .continuous))
                    .overlay(
                        RoundedRectangle(cornerRadius: Theme.radiusLG, style: .continuous)
                            .stroke(borderColor, lineWidth: 1)
                    )
            }

            Text(shown.title)
                .font(.system(.title3, design: .serif))
                .fontWeight(.medium)
                .foregroundColor(Color("TextPrimary"))
                .multilineTextAlignment(.center)

            if !shown.message.isEmpty {
                Text(shown.message)
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
                    .multilineTextAlignment(.center)
                    .padding(.horizontal, 40)
            }

            if let actionTitle = shownActionTitle, let action = shownAction {
                Button(action: {
                    Haptics.light()
                    action()
                }) {
                    Text(actionTitle)
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .padding(.horizontal, 18)
                        .frame(height: 40)
                        .background(Color("BgPanel"))
                        .clipShape(Capsule())
                        .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
                }
                .buttonStyle(.plain)
                .padding(.top, 4)
            }
        }
        .padding(.top, 48)
        .padding(.bottom, 24)
        .frame(maxWidth: .infinity)
        .sheet(isPresented: $isConnectPresented) { ConnectComputerSheet() }
    }

    private var foregroundColor: Color {
        switch effectiveStyle {
        case .empty: return Color("AccentPrimary")
        case .error: return Theme.statusError
        case .offline: return Theme.statusWarning
        }
    }

    private var backgroundColor: Color {
        switch effectiveStyle {
        case .empty: return Color("AccentPrimary").opacity(0.12)
        case .error: return Theme.statusError.opacity(0.12)
        case .offline: return Theme.statusWarning.opacity(0.12)
        }
    }

    private var borderColor: Color {
        switch effectiveStyle {
        case .empty: return Color("AccentPrimary").opacity(0.25)
        case .error: return Theme.statusError.opacity(0.25)
        case .offline: return Theme.statusWarning.opacity(0.25)
        }
    }
}

/// A compact inline empty / error / offline state for list rows and section
/// cells where the full `FriendlyStateView` centered hero would be too much.
struct FriendlyInlineStateView: View {
    enum Style {
        case empty
        case error
        case offline
    }

    var style: Style = .empty
    var icon: String = "tray"
    let title: String
    var message: String = ""
    var actionTitle: String? = nil
    var action: (() -> Void)? = nil
    /// Line art (36 pt) beside the line, in place of the icon tile (empty style only).
    var art: EmptyArtKind? = nil
    var paper: Color = Color("BgPrimary")

    /// Same calm presentation as `FriendlyStateView.shown`.
    private var shown: CalmStateText {
        CalmStateText(isError: style != .empty, icon: icon, title: title, message: message)
    }

    private var effectiveStyle: Style { shown.isNoComputer ? .empty : style }

    /// Same Connect as `FriendlyStateView` on the no-computer state.
    @State private var isConnectPresented = false
    private var shownActionTitle: String? { shown.isNoComputer ? "Connect" : actionTitle }
    private var shownAction: (() -> Void)? {
        shown.isNoComputer ? { isConnectPresented = true } : action
    }

    var body: some View {
        let shown = self.shown
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                if let art, effectiveStyle == .empty {
                    EmptyArt(kind: art, size: .inline, paper: paper)
                } else {
                    Image(systemName: shown.icon)
                        .font(.system(size: 18, weight: .medium))
                        .foregroundColor(foregroundColor)
                        .frame(width: 36, height: 36)
                        .background(backgroundColor)
                        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD, style: .continuous))
                }

                VStack(alignment: .leading, spacing: 3) {
                    Text(shown.title)
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))

                    if !shown.message.isEmpty {
                        Text(shown.message)
                            .font(.subheadline)
                            .foregroundColor(Color("TextSecondary"))
                            .lineLimit(3)
                    }
                }

                Spacer(minLength: 0)
            }

            if let actionTitle = shownActionTitle, let action = shownAction {
                Button(action: {
                    Haptics.light()
                    action()
                }) {
                    Text(actionTitle)
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .padding(.horizontal, 14)
                        .frame(height: 32)
                        .background(Color("BgPanel"))
                        .clipShape(Capsule())
                        .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.vertical, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .sheet(isPresented: $isConnectPresented) { ConnectComputerSheet() }
    }

    private var foregroundColor: Color {
        switch effectiveStyle {
        case .empty: return Color("AccentPrimary")
        case .error: return Theme.statusError
        case .offline: return Theme.statusWarning
        }
    }

    private var backgroundColor: Color {
        switch effectiveStyle {
        case .empty: return Color("AccentPrimary").opacity(0.12)
        case .error: return Theme.statusError.opacity(0.12)
        case .offline: return Theme.statusWarning.opacity(0.12)
        }
    }
}

/// Shared helper that turns raw URLSession / transport errors into product-
/// friendly copy for `FriendlyStateView` and other empty/error states.
enum FriendlyErrorMessage {
    /// Same as `from(_: String)`, starting from the error itself.
    static func from(_ error: Error) -> String {
        from(plainErrorText(error))
    }

    static func from(_ raw: String) -> String {
        // Status numbers, status phrases and codes never reach the screen
        // (contract C5): what is left is the message, or the fallback.
        let error = calmMessageText(raw) ?? ""
        if error == CalmCopy.computeRequired { return error }
        let lowered = error.lowercased()
        // cloud-api 428 (routes/beta.rs): the account has no computer yet.
        if lowered.contains("data-plane node") {
            return "Connect a computer or start Allternit Cloud to use this."
        }
        if lowered.contains("could not connect") || lowered.contains("failed to connect") {
            return "Could not connect to the server. Check your connection and try again."
        }
        if lowered.contains("cancelled") || lowered.contains("canceled") {
            return "The request was cancelled."
        }
        if lowered.contains("not found") {
            return "That resource couldn't be found."
        }
        return plain(error)
    }

    /// Like `from(_:)` but names what failed, so a raw code or abbreviation
    /// ("nf", "HTTP 500") never reaches the screen: `doing` = "load engines".
    static func from(_ error: String, doing: String) -> String {
        let message = from(error)
        return message == fallback ? "Couldn't \(doing). Check your connection and try again." : message
    }

    static let fallback = "Something went wrong. Check your connection and try again."

    /// Keeps a message only if it already reads as a sentence; raw codes,
    /// status numbers and bare abbreviations become the generic sentence.
    private static func plain(_ error: String) -> String {
        let trimmed = error.trimmingCharacters(in: .whitespacesAndNewlines)
        let lowered = trimmed.lowercased()
        let words = trimmed.split(separator: " ").count
        let rawMarkers = ["http", "status code", "nsurl", "error domain", "decoding", "json", "sqlx", "errno", "code=", "decode", "couldn't be read", "correct format", "server response", "header", "bearer", "token", "authorization", "unauthorized"]
        if words < 4 || rawMarkers.contains(where: { lowered.contains($0) })
            || trimmed.contains(where: \.isNumber) && words < 6 {
            return fallback
        }
        return trimmed
    }
}

/// What a friendly state shows once made calm (contract C5): an error or
/// offline message loses any status number or code, and a compute-gate
/// refusal ("Agents need a computer.") shows as the no-computer empty state
/// instead of an error, so every list that renders its load error here
/// reads calm without a per-screen change.
struct CalmStateText: Equatable {
    let icon: String
    let title: String
    let message: String
    let isNoComputer: Bool

    init(isError: Bool, icon: String, title: String, message: String) {
        guard isError, !message.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            self.init(icon: icon, title: title, message: message, isNoComputer: false)
            return
        }
        let calm = plainErrorText(message: message, fallback: CalmCopy.fallback)
        if calm == CalmCopy.computeRequired || isComputeGateRefusal(APIError.httpError(statusCode: 409, message: message)) {
            self.init(icon: "desktopcomputer", title: CalmCopy.noComputer,
                      message: CalmCopy.noComputerDetail, isNoComputer: true)
            return
        }
        self.init(icon: icon, title: stripStatusText(title).isEmpty ? title : stripStatusText(title),
                  message: calm, isNoComputer: false)
    }

    private init(icon: String, title: String, message: String, isNoComputer: Bool) {
        self.icon = icon
        self.title = title
        self.message = message
        self.isNoComputer = isNoComputer
    }
}

/// Reads refused by the compute gate (409 `compute_required`, 428), shared
/// app-wide. A refused read shows no error; the no-computer chip (step 20)
/// reads `refused` instead. Mirrors web `phone-ui-store` `computeReadRefused`.
@MainActor
final class ComputeReadState: ObservableObject {
    static let shared = ComputeReadState()

    @Published private(set) var refused = false

    /// A GET/HEAD answered with a compute-gate refusal.
    func noteRefused() {
        if !refused { refused = true }
    }

    /// A computer answered (or the account paired one): the flag is stale.
    func clear() {
        if refused { refused = false }
    }

    /// Reads only: actions open the gate sheet instead (same rule as
    /// `ComputeStatusStore.ignores409`).
    nonisolated static func isRead(method: String?) -> Bool {
        guard let m = method?.uppercased() else { return false }
        return m == "GET" || m == "HEAD"
    }
}

#Preview {
    VStack(spacing: 24) {
        FriendlyStateView(
            style: .empty,
            title: "No agents yet",
            message: "Create an agent from a template to get started.",
            actionTitle: "New from template",
            action: {},
            art: .gizzi
        )
        FriendlyStateView(
            style: .offline,
            icon: "wifi.slash",
            title: "You're offline",
            message: "Connect to the internet to load your agents.",
            actionTitle: "Retry",
            action: {}
        )
    }
    .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
}
