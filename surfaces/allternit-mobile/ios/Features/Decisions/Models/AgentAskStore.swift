import Foundation

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

    init(client: AgentAsksClient = .shared) {
        self.client = client
    }

    func isResolved(_ id: String) -> Bool { resolvedIds.contains(id) }

    func markResolved(_ id: String) {
        resolvedIds.insert(id)
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
        guard let sessionId else { return [] }
        return (pendingPermissions[sessionId] ?? []).filter { !resolvedIds.contains($0.id) }
    }

    /// Re-reads what the session's agent waits on. Failures keep the last
    /// list (offline computer: the cards that streamed in still show).
    func refresh(sessionId: String) async {
        guard !inFlight.contains(sessionId) else { return }
        inFlight.insert(sessionId)
        defer { inFlight.remove(sessionId) }
        if let questions = try? await client.listQuestions(sessionId: sessionId) {
            pendingQuestions[sessionId] = questions
        }
        if let permissions = try? await client.listPermissions(sessionId: sessionId) {
            pendingPermissions[sessionId] = permissions
        }
    }

    // MARK: - Answers

    func answer(_ request: AgentQuestionRequest, answers: [AgentAsksClient.ReplyAnswer]) async throws {
        try await client.replyQuestion(id: request.id, answers: answers)
        markResolved(request.id)
    }

    func skip(_ request: AgentQuestionRequest) async throws {
        try await client.rejectQuestion(id: request.id)
        markResolved(request.id)
    }

    /// `reply`: "once" | "always" | "reject".
    func reply(_ permission: AgentPermissionAsk, reply: String) async throws {
        try await client.replyPermission(id: permission.id, reply: reply)
        markResolved(permission.id)
    }
}
