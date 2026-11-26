import SwiftUI

/// Local in-memory state for Code Canvas / workspace parity.
///
/// Phase 1: one default workspace whose tiles are derived from the existing
/// code-session list. No disk persistence — the web canvas is also local-only
/// today, and inventing a backend spec is out of scope.
@MainActor
final class CodeCanvasStore: ObservableObject {
    static let shared = CodeCanvasStore()

    @Published private(set) var workspaces: [CodeWorkspace] = []
    @Published private(set) var activeWorkspaceId: String? = nil

    private init() {
        reset()
    }

    /// Back to the one empty default workspace. Account switch (phone calm
    /// I14b, org and person): the leaving account's session tiles go; the
    /// canvas fills again from the new account's sessions when it appears.
    func reset() {
        workspaces = [
            CodeWorkspace(
                id: "default",
                displayName: "Default Workspace",
                rootPath: nil,
                sessionIds: [],
                layoutMode: .canvas,
                tiles: []
            )
        ]
        activeWorkspaceId = workspaces[0].id
    }

    var activeWorkspace: CodeWorkspace? {
        workspaces.first { $0.id == activeWorkspaceId }
    }

    /// Replaces the default workspace's tiles with one session tile per code
    /// session. Called whenever the canvas view appears.
    func syncTiles(from sessions: [AgentSession]) {
        guard let index = workspaces.firstIndex(where: { $0.id == activeWorkspaceId }) else { return }
        let tiles: [CodeCanvasTile] = sessions.enumerated().map { offset, session in
            CodeCanvasTile(
                id: "tile-\(session.id)",
                type: .session,
                sessionId: session.id,
                x: Double(16 + (offset % 2) * 172),
                y: Double(16 + (offset / 2) * 148),
                width: 160,
                height: 132,
                zIndex: offset + 1,
                label: session.displayTitle
            )
        }
        workspaces[index].tiles = tiles
        workspaces[index].sessionIds = sessions.map(\.id)
    }
}
