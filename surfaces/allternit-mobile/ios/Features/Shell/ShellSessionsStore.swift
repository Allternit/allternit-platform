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
    /// Sessions with a reply streaming right now (the web store's
    /// `streamingBySession`): rows show "Working" instead of the time.
    @Published private(set) var streaming: Set<String> = []
    /// The chat on screen; it never shows as unread.
    @Published private(set) var activeId: String? = nil
    /// When each session was last seen, kept across launches. A session with
    /// activity after it (and not on screen) is unread: the web's
    /// `unreadCounts`, which iOS can't fill from a live sync of every chat.
    @Published private var seen: [String: Date] = [:]

    /// Sessions running on a connected cloud computer (Teleport), by session id → computer name.
    /// Everything not listed runs on this Mac.
    @Published private(set) var cloudPlaces: [String: String] = [:]

    private static let seenKey = "allternit-phone-session-seen"

    typealias SessionsFetch = @Sendable () async throws -> [AgentSession]
    typealias PlanThreadsFetch = @Sendable () async throws -> [AgentSession]
    typealias PlacesFetch = @Sendable () async -> [String: String]?

    private let fetchComputerSessions: SessionsFetch
    private let fetchPlanThreads: PlanThreadsFetch
    private let fetchPlaces: PlacesFetch

    /// The defaults read the live backend; tests pass stubs for the three fetches.
    init(
        fetchComputerSessions: @escaping SessionsFetch = {
            // GET /api/v1/agent-sessions → { sessions: [...], count }
            let envelope: AgentSessionListResponse = try await APIClient.shared.get(path: "agent-sessions")
            return envelope.sessions
        },
        fetchPlanThreads: @escaping PlanThreadsFetch = {
            try await PlanThreadClient.shared.list().compactMap(\.asSession)
        },
        fetchPlaces: @escaping PlacesFetch = {
            struct Place: Decodable { let label: String }
            struct Reply: Decodable { let placements: [String: Place] }
            let reply: Reply? = try? await APIClient.shared.get(path: "teleports/placements")
            return reply?.placements.mapValues { $0.label }
        }
    ) {
        self.fetchComputerSessions = fetchComputerSessions
        self.fetchPlanThreads = fetchPlanThreads
        self.fetchPlaces = fetchPlaces
        if let raw = UserDefaults.standard.dictionary(forKey: Self.seenKey) as? [String: Double] {
            seen = raw.mapValues { Date(timeIntervalSince1970: $0) }
        }
    }

    /// True when the failure only means "this account has no computer":
    /// cloud-api answers 428 (Precondition Required) to a relayed call with
    /// no computer to relay to, and the compute gate answers 409
    /// `compute_required`. Plan chats still work then (contract C5).
    static func isNoComputer(_ error: Error) -> Bool {
        isComputeGateRefusal(error)
    }

    /// Computer sessions and cloud plan chats load independently: a computer
    /// that is missing or failing never hides plan chats (the "brain runner"
    /// path has no computer at all).
    func load() async {
        if !hasLoaded { isLoading = true }
        loadError = nil
        async let computerResult: Result<[AgentSession], Error> = {
            do { return .success(try await fetchComputerSessions()) } catch { return .failure(error) }
        }()
        async let planResult: Result<[AgentSession], Error> = {
            do { return .success(try await fetchPlanThreads()) } catch { return .failure(error) }
        }()
        let computer = await computerResult
        let plan = await planResult

        if Task.isCancelled || isCancellation(computer) || isCancellation(plan) {
            // View disappeared mid-flight: keep the current state.
            isLoading = false
            return
        }

        var merged: [AgentSession] = []
        var computerError: Error?
        var planError: Error?
        switch computer {
        case .success(let list): merged += list
        case .failure(let error): computerError = error
        }
        switch plan {
        case .success(let list): merged += list
        case .failure(let error): planError = error
        }

        // No computer is not an error: its list is just empty. Any other
        // computer failure only shows the error card when no plan chat loaded
        // to fill the list; both failing always does.
        let computerBroken = computerError.map { !Self.isNoComputer($0) } ?? false
        let planFailed = planError != nil
        let showError: Bool
        if computerError != nil && planFailed { showError = true }
        else if computerBroken && merged.isEmpty { showError = true }
        else { showError = false }

        // The no-computer chip reads the refusal; a computer that answered
        // clears a stale one.
        if let computerError, Self.isNoComputer(computerError) {
            ComputeReadState.shared.noteRefused()
        } else if computerError == nil {
            ComputeReadState.shared.clear()
        }

        if showError {
            let shown = (computerError.flatMap { Self.isNoComputer($0) ? nil : $0 }) ?? planError ?? computerError
            loadError = plainErrorText(shown)
        } else {
            sessions = merged.sorted { Self.timestamp(of: $0) > Self.timestamp(of: $1) }
            // First sighting is the baseline: only activity after it counts
            // as unread. The chat on screen is always read.
            for session in sessions where seen[session.id] == nil || session.id == activeId {
                seen[session.id] = Self.timestamp(of: session)
            }
            saveSeen()
            hasLoaded = true
            if computerError == nil { await loadPlaces() } else { cloudPlaces = [:] }
        }
        isLoading = false
    }

    private func isCancellation(_ result: Result<[AgentSession], Error>) -> Bool {
        if case .failure(let error) = result { return error is CancellationError }
        return false
    }

    /// `GET /api/v1/teleports/placements` → { placements: { sessionId: { targetId, label } } }.
    /// An older backend has no route; every session then reads as local.
    func loadPlaces() async {
        if let places = await fetchPlaces() { cloudPlaces = places }
    }

    /// Move a Code session between this Mac and the connected cloud computer
    /// (`POST /api/v1/native-sessions/teleport`, the same call the web header makes).
    func teleport(id: String, toCloud: Bool) async throws {
        struct Body: Encodable { let sessionId: String; let to: String }
        try await APIClient.shared.post(path: "native-sessions/teleport", body: Body(sessionId: id, to: toCloud ? "cloud" : "local"))
        await loadPlaces()
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

    // MARK: - Working / unread

    func isStreaming(_ id: String) -> Bool { streaming.contains(id) }

    /// The web rail's unread dot (and bold title): new activity since the
    /// session was last seen, not while it streams or is on screen.
    func isUnread(_ session: AgentSession) -> Bool {
        guard session.id != activeId, !streaming.contains(session.id) else { return false }
        guard let last = seen[session.id] else { return false }
        return Self.timestamp(of: session) > last
    }

    /// The chat view reports its session and whether a reply is streaming.
    /// `nil` (the chat left the screen) keeps a running reply's "Working".
    func setActive(_ id: String?, streaming isStreaming: Bool) {
        if let previous = activeId, previous != id {
            markSeen(previous)
            // The chat switched sessions, which ends the old session's stream here.
            if id != nil { streaming.remove(previous) }
        }
        activeId = id
        PetOfferCenter.shared.visibleSessionId = id
        guard let id else { return }
        if isStreaming { streaming.insert(id) } else { streaming.remove(id) }
        markSeen(id)
    }

    /// `setActiveSession` / `markSessionRead`: clears the unread state.
    func markSeen(_ id: String) {
        let at = session(id: id).map(Self.timestamp(of:)) ?? .distantPast
        seen[id] = max(at, seen[id] ?? .distantPast)
        saveSeen()
    }

    private func saveSeen() {
        UserDefaults.standard.set(seen.mapValues { $0.timeIntervalSince1970 }, forKey: Self.seenKey)
    }

    // MARK: - Overview actions (useModeOverview rename / archive)

    /// Rename from a card's ⋯ menu: `PATCH /api/v1/agent-sessions/:id`
    /// `{ name }` (renameOverviewItem). Optimistic; restored on failure.
    func rename(id: String, to name: String) async throws {
        let before = sessions
        sessions = sessions.map { $0.id == id ? $0.renamed(name) : $0 }
        struct Body: Encodable { let name: String }
        do {
            try await APIClient.shared.patch(path: "agent-sessions/\(Self.escape(id))", body: Body(name: name))
        } catch {
            sessions = before
            throw error
        }
    }

    /// Close a card (X, swipe up, or ⋯ → Archive): archives the session
    /// (archiveOverviewItem → `metadata.archived`, plus `active: false` as
    /// Code's setCodeSessionArchived sends). It leaves the open lists and
    /// stays under the drawer's Archived filter.
    func archive(id: String) async throws {
        let before = remove(id: id)
        struct Body: Encodable {
            struct Metadata: Encodable { let archived: Bool }
            let active: Bool
            let metadata: Metadata
        }
        do {
            try await APIClient.shared.patch(
                path: "agent-sessions/\(Self.escape(id))",
                body: Body(active: false, metadata: .init(archived: true))
            )
            await load()
        } catch {
            restore(before)
            throw error
        }
    }

    private static func escape(_ id: String) -> String {
        id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id
    }

    /// Open (not archived) sessions of a mode: the header count and the overview.
    func openSessions(for mode: ShellMode) -> [AgentSession] {
        sessions.filter { $0.active && !$0.archived && Self.belongs($0, to: mode) }
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
