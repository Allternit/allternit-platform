import Foundation
import SwiftUI

/// The account's sessions as the phone shell reads them: the drawer's
/// Recents, the chat header's count button and the session overview
/// (allternit-ai useRailRecents / useModeOverview). One list, loaded from
/// `GET /api/v1/agent-sessions`, split per mode by `belongs(_:to:)`.
@MainActor
final class ShellSessionsStore: ObservableObject {
    static let shared = ShellSessionsStore()

    @Published private(set) var sessions: [AgentSession] = []
    @Published private(set) var hasLoaded = false
    @Published private(set) var isLoading = false
    @Published private(set) var loadError: String? = nil

    func load() async {
        if !hasLoaded { isLoading = true }
        loadError = nil
        do {
            // GET /api/v1/agent-sessions → { sessions: [...], count }
            let envelope: AgentSessionListResponse = try await APIClient.shared.get(path: "agent-sessions")
            sessions = envelope.sessions.sorted { Self.timestamp(of: $0) > Self.timestamp(of: $1) }
            hasLoaded = true
        } catch is CancellationError {
            // View disappeared mid-flight — keep the current state.
        } catch {
            loadError = error.localizedDescription
        }
        isLoading = false
    }

    func session(id: String?) -> AgentSession? {
        guard let id else { return nil }
        return sessions.first { $0.id == id }
    }

    /// Optimistic local removal; `restore` puts the list back on a failed delete.
    func remove(id: String) -> [AgentSession] {
        let before = sessions
        sessions.removeAll { $0.id == id }
        return before
    }

    func restore(_ list: [AgentSession]) {
        sessions = list
    }

    /// Open (not archived) sessions of a mode: the header count and the overview.
    func openSessions(for mode: ShellMode) -> [AgentSession] {
        sessions.filter { $0.active && Self.belongs($0, to: mode) }
    }

    /// Sessions that belong to a mode's drawer. Bot sessions (with an
    /// `agent_id`) live in Bots; Code and ACI keep their own surfaces;
    /// incognito sessions are excluded server-side.
    static func belongs(_ session: AgentSession, to mode: ShellMode) -> Bool {
        let surface = session.originSurface ?? ""
        switch mode {
        case .bots:
            return session.agentId != nil
        case .home:
            return session.agentId == nil && (surface == "chat" || surface == "cowork" || surface.isEmpty)
        case .code:
            return session.agentId == nil && surface == "code"
        case .aci:
            return session.agentId == nil && surface == "browser"
        }
    }

    static func timestamp(of record: AgentSession) -> Date {
        parseTimestamp(record.updatedAt) ?? parseTimestamp(record.createdAt) ?? .distantPast
    }

    /// Backend timestamps are RFC-3339 (chrono `to_rfc3339`), with or without
    /// fractional seconds.
    private static func parseTimestamp(_ value: String) -> Date? {
        if let date = try? Date(value, strategy: Date.ISO8601FormatStyle(includingFractionalSeconds: true)) {
            return date
        }
        return try? Date(value, strategy: Date.ISO8601FormatStyle())
    }

    /// The rail's relative time (session-summary formatRelativeTime):
    /// "12s", "5m", "3h", "2d".
    static func relativeTime(_ date: Date) -> String {
        guard date != .distantPast else { return "" }
        let seconds = max(0, Int(Date().timeIntervalSince(date)))
        if seconds < 60 { return "\(seconds)s" }
        if seconds < 3600 { return "\(seconds / 60)m" }
        if seconds < 86_400 { return "\(seconds / 3600)h" }
        return "\(seconds / 86_400)d"
    }
}
