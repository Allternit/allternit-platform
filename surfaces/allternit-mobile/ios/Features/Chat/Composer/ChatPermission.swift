import SwiftUI

/// How freely one chat works (web lib/chat/permission-mode.ts, Cowork folded
/// into Chat 2026-10-27). One setting per session, stored on the session's
/// `metadata.permissionMode`; before a session exists the choice is held as
/// the pending mode and stamped onto the new session's create.
enum ChatPermissionMode: String, CaseIterable, Sendable {
    case auto
    case ask = "default"
    case readOnly = "plan"

    var label: String {
        switch self {
        case .auto: return "Auto"
        case .ask: return "Ask before actions"
        case .readOnly: return "Read-only"
        }
    }

    /// Short value for the + sheet's "Permission ›" row (web PERMISSION_SHORT_LABEL).
    var shortLabel: String {
        switch self {
        case .auto: return "Auto"
        case .ask: return "Ask"
        case .readOnly: return "Read-only"
        }
    }

    var description: String {
        switch self {
        case .auto: return "Keeps working; asks only before deletes and installs"
        case .ask: return "Asks before running commands or edits"
        case .readOnly: return "Only reads and researches"
        }
    }
}

/// The pending launch-screen choice plus what each session was set to. The
/// server holds the truth on `metadata.permissionMode`; the per-session map
/// here remembers it so the pill reads back without a session refetch.
@MainActor
final class ChatPermissionStore: ObservableObject {
    static let shared = ChatPermissionStore()

    /// The choice for a chat that doesn't exist yet (web usePendingPermissionMode,
    /// default Auto, not persisted).
    @Published var pendingMode: ChatPermissionMode = .auto
    /// The pending mode was picked by hand. Until then a chat on a picked
    /// computer uses that computer's own default (Start anywhere D4).
    @Published private(set) var pendingChosen = false
    @Published private(set) var modeBySession: [String: ChatPermissionMode]

    private let defaults = UserDefaults.standard
    private static let key = "allternit-chat-permission-modes"

    private init() {
        let raw = defaults.dictionary(forKey: Self.key) as? [String: String] ?? [:]
        modeBySession = raw.compactMapValues(ChatPermissionMode.init(rawValue:))
    }

    func mode(for sessionId: String?) -> ChatPermissionMode {
        guard let sessionId else { return pendingMode }
        return modeBySession[sessionId] ?? .auto
    }

    /// A new session took the pending choice (sent on its create).
    func bind(sessionId: String, mode: ChatPermissionMode) {
        pendingChosen = false
        guard modeBySession[sessionId] == nil else { return }
        remember(mode, for: sessionId)
    }

    /// A pick (the + sheet's Permission page): pending without a session, else a PATCH of the
    /// session's metadata (non-blocking, like the web's optimistic merge).
    func choose(_ mode: ChatPermissionMode, sessionId: String?) {
        guard let sessionId else {
            pendingMode = mode
            pendingChosen = true
            return
        }
        remember(mode, for: sessionId)
        Task {
            try? await AgentChatClient().updateSessionMetadata(
                sessionId: sessionId,
                metadata: ["permissionMode": mode.rawValue]
            )
        }
    }

    /// The mode a new chat starts with: the hand pick, else the picked
    /// computer's default, else the pending mode.
    func startMode(computerDefault: ChatPermissionMode?) -> ChatPermissionMode {
        pendingChosen ? pendingMode : (computerDefault ?? pendingMode)
    }

    private func remember(_ mode: ChatPermissionMode, for sessionId: String) {
        modeBySession[sessionId] = mode
        defaults.set(modeBySession.mapValues(\.rawValue), forKey: Self.key)
    }
}

/// The old composer-row "◈ Auto" pill (web ChatPermissionPill, compact
/// form): a quiet text trigger opening Auto · Ask before actions · Read-only.
/// Kept with no caller since the phone calm pass (contract C2): the choice
/// now lives in the composer's + sheet, "Permission · Auto ›"
/// (ComposerPlusSheet's Permission page, same `ChatPermissionStore`).
struct ChatPermissionPill: View {
    let sessionId: String?
    @ObservedObject private var store = ChatPermissionStore.shared

    var body: some View {
        let mode = store.mode(for: sessionId)
        Menu {
            ForEach(ChatPermissionMode.allCases, id: \.self) { option in
                Button(action: { store.choose(option, sessionId: sessionId) }) {
                    if option == mode {
                        Label {
                            Text(option.label)
                            Text(option.description)
                        } icon: {
                            Image(systemName: "checkmark")
                        }
                    } else {
                        Text(option.label)
                        Text(option.description)
                    }
                }
            }
        } label: {
            HStack(spacing: 4) {
                Image(systemName: "checkmark.shield")
                    .font(.system(size: 14))
                Text(mode.shortLabel)
                    .font(.system(size: 14))
            }
            .foregroundColor(Color("TextSecondary"))
            .padding(.horizontal, 6)
            .frame(height: 44)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Permissions: \(mode.label)")
    }
}
