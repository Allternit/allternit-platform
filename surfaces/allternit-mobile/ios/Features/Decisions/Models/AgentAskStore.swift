import Foundation
import UIKit

/// What the person has answered this app session, and what each chat's
/// agent is still waiting on. Keeps an answered card from coming back when
/// a row re-renders or a poll still lists it (web DecisionsDeck `answered`).
@MainActor
final class AgentAskStore: ObservableObject {
    static let shared = AgentAskStore()

    /// Question / permission ids answered or skipped here.
    @Published private(set) var resolvedIds: Set<String> = []
    /// Pending questions per session, from `GET /questions?sessionId=` —
    /// covers a chat reopened while its agent still waits.
    @Published private(set) var pendingQuestions: [String: [AgentQuestionRequest]] = [:]
    @Published private(set) var pendingPermissions: [String: [AgentPermissionAsk]] = [:]

    private let client: AgentAsksClient
    private var inFlight: Set<String> = []
    /// Clients for chats bound to one computer (Start anywhere), by runtime id.
    private var boundClients: [String: AgentAsksClient] = [:]

    /// A chat bound to a Gizzi session on a picked computer: its asks live on
    /// that computer and show only in that thread, never in the app-wide lists.
    func isBound(_ sessionId: String?) -> Bool {
        boundRuntime(sessionId) != nil
    }

    private func boundRuntime(_ sessionId: String?) -> String? {
        guard let binding = SessionStartStore.shared.binding(for: sessionId), binding.kind == .engine else { return nil }
        return binding.runtimeId
    }

    /// The client for a session: its bound computer's (the same
    /// `/api/v1/questions` / `/permissions` routes, through the relay), else the app's.
    private func client(for sessionId: String?) -> AgentAsksClient {
        guard let runtimeId = boundRuntime(sessionId) else { return client }
        if let cached = boundClients[runtimeId] { return cached }
        let relay = RuntimeRelayClient.relayBaseURL(runtimeId: runtimeId)
        let bound = AgentAsksClient(client: APIClient(baseURL: relay.appendingPathComponent("api/v1"),
                                                      tokenProvider: { try await AuthManager.shared.getToken() }))
        boundClients[runtimeId] = bound
        return bound
    }

    /// Async asks (no waiting turn) per session: kept even when the session
    /// is idle, rebuilt from the pending list on launch and foreground.
    @Published private(set) var asyncAsks: [String: [AgentQuestionRequest]] = [:]
    /// Sessions that started a new turn after an async ask was answered.
    @Published private(set) var workingSessions: Set<String> = []

    init(client: AgentAsksClient = .shared) {
        self.client = client
        NotificationCenter.default.addObserver(
            forName: UIApplication.willEnterForegroundNotification, object: nil, queue: .main
        ) { [weak self] _ in
            Task { @MainActor in await self?.refreshAll() }
        }
    }

    /// Re-fetches every pending question across sessions (launch / foreground),
    /// so async asks that arrived while the app was closed come back.
    func refreshAll() async {
        guard let all = try? await client.listQuestions() else { return }
        var bySession: [String: [AgentQuestionRequest]] = [:]
        // Bound threads own their asks: skip them here, keep what their own refresh read.
        for ask in all where !resolvedIds.contains(ask.id) && !isBound(ask.sessionID) {
            bySession[ask.sessionID, default: []].append(ask)
        }
        asyncAsks = bySession.mapValues { $0.filter { $0.isAsync } }.filter { !$0.value.isEmpty }
        for (session, list) in pendingQuestions where isBound(session) { bySession[session] = list }
        pendingQuestions = bySession
    }

    /// Stream events: `question.asked` adds, `question.replied` / `question.rejected` remove.
    func questionAsked(_ ask: AgentQuestionRequest) {
        guard !resolvedIds.contains(ask.id) else { return }
        var list = pendingQuestions[ask.sessionID] ?? []
        if !list.contains(where: { $0.id == ask.id }) { list.append(ask) }
        pendingQuestions[ask.sessionID] = list
        if ask.isAsync, !isBound(ask.sessionID) {
            var a = asyncAsks[ask.sessionID] ?? []
            if !a.contains(where: { $0.id == ask.id }) { a.append(ask) }
            asyncAsks[ask.sessionID] = a
        }
    }

    func questionClosed(id: String) { markResolved(id) }

    func isWorking(_ sessionId: String) -> Bool { workingSessions.contains(sessionId) }
    func clearWorking(_ sessionId: String) { workingSessions.remove(sessionId) }

    func isResolved(_ id: String) -> Bool { resolvedIds.contains(id) }

    func markResolved(_ id: String) {
        resolvedIds.insert(id)
        for (session, list) in asyncAsks {
            let kept = list.filter { $0.id != id }
            asyncAsks[session] = kept.isEmpty ? nil : kept
        }
        for (session, list) in pendingQuestions {
            pendingQuestions[session] = list.filter { $0.id != id }
        }
        for (session, list) in pendingPermissions {
            pendingPermissions[session] = list.filter { $0.id != id }
        }
    }

    func questions(for sessionId: String?) -> [AgentQuestionRequest] {
        guard let sessionId else { return [] }
        return (pendingQuestions[sessionId] ?? []).filter { !resolvedIds.contains($0.id) }
    }

    func permissions(for sessionId: String?) -> [AgentPermissionAsk] {
        // A bound thread shows its approvals in BoundEngineApprovals.
        guard let sessionId, !isBound(sessionId) else { return [] }
        return (pendingPermissions[sessionId] ?? []).filter { !resolvedIds.contains($0.id) }
    }

    /// Re-reads what the session's agent waits on. Failures keep the last
    /// list (offline computer: the cards that streamed in still show).
    func refresh(sessionId: String) async {
        guard !inFlight.contains(sessionId) else { return }
        inFlight.insert(sessionId)
        defer { inFlight.remove(sessionId) }
        let client = client(for: sessionId)
        if let questions = try? await client.listQuestions(sessionId: sessionId) {
            pendingQuestions[sessionId] = questions
        }
        if let permissions = try? await client.listPermissions(sessionId: sessionId) {
            pendingPermissions[sessionId] = permissions
        }
    }

    // MARK: - Answers

    func answer(_ request: AgentQuestionRequest, answers: [AgentAsksClient.ReplyAnswer]) async throws {
        try await client(for: request.sessionID).replyQuestion(id: request.id, answers: answers)
        if request.isAsync { workingSessions.insert(request.sessionID) }
        markResolved(request.id)
    }

    func skip(_ request: AgentQuestionRequest) async throws {
        try await client(for: request.sessionID).rejectQuestion(id: request.id)
        markResolved(request.id)
    }

    /// `reply`: "once" | "always" | "reject".
    func reply(_ permission: AgentPermissionAsk, reply: String) async throws {
        try await client(for: permission.sessionID).replyPermission(id: permission.id, reply: reply)
        markResolved(permission.id)
    }
}
