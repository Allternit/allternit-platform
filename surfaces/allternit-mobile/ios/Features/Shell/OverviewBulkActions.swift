import Foundation

/// Overview select-mode actions the store did not have yet (phone calm step 25):
/// unarchive and delete, plus the Archived view's list. The web does the same in
/// `unarchiveOverviewItem` and `useModeOverview(mode, { archived: true })`.
extension ShellSessionsStore {
    /// Archived chats and code threads of a mode (never terminals or agent runs).
    func archivedSessions(for mode: ShellMode) -> [AgentSession] {
        guard mode == .home || mode == .code else { return [] }
        return sessions.filter { ($0.archived || !$0.active) && !$0.isTerminal && Self.belongs($0, to: mode) }
    }

    /// Put an archived session back in the open list: `metadata.archived` off and
    /// `active: true` (Code's archive is `active: false`, so this undoes both).
    func unarchive(id: String) async throws {
        struct Body: Encodable {
            struct Metadata: Encodable { let archived: Bool }
            let active: Bool
            let metadata: Metadata
        }
        try await APIClient.shared.patch(
            path: "agent-sessions/\(Self.escapedId(id))",
            body: Body(active: true, metadata: .init(archived: false))
        )
        await load()
    }

    /// Delete for good, as the drawer's swipe-delete does. Optimistic; the list
    /// comes back if the write fails. Plan chats (pth_…) live in the plan store.
    func delete(id: String) async throws {
        let before = remove(id: id)
        RailPinsStore.shared.remove(id: id)
        do {
            if PlanThreadClient.isThreadId(id) {
                try await PlanThreadClient.shared.delete(id: id)
            } else {
                try await APIClient.shared.delete(path: "agent-sessions/\(Self.escapedId(id))")
            }
        } catch {
            restore(before)
            throw error
        }
    }

    private static func escapedId(_ id: String) -> String {
        id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id
    }
}
