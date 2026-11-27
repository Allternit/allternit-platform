import Foundation

/// Client for the memory API (`cmd/allternit-api/src/memory_routes.rs`):
/// stats, health, and the document list; plus Allternit Sleep
/// (`cmd/allternit-api/src/sleep/routes.rs`): status, a cycle's report, undo,
/// run, settings and memory conflicts.
final class MemoryClient: @unchecked Sendable {
    private let client: APIClient

    init(client: APIClient = .shared) {
        self.client = client
    }

    /// `GET /api/v1/memory/stats` → `{ connections, insights, memories: { total }, vectors }`.
    func stats() async throws -> MemoryStats {
        try await client.get(path: "memory/stats")
    }

    /// `GET /api/v1/memory/health` → `{ status, memory: {...} }`.
    func health() async throws -> MemoryHealth {
        try await client.get(path: "memory/health")
    }

    /// `GET /api/v1/memory/documents` → `[MemoryDoc]` (a bare array, newest
    /// `updated_at` first).
    func listDocuments() async throws -> [MemoryDocument] {
        try await client.get(path: "memory/documents")
    }

    /// `POST /api/v1/memory/query` — semantic search. The settings UI
    /// searches the already-fetched document list client-side instead; this
    /// is kept for callers that want the server-side semantic query.
    func query(_ text: String) async throws -> [MemoryDocument] {
        try await client.post(path: "memory/query", body: QueryBody(query: text))
    }

    private struct QueryBody: Encodable {
        let query: String
    }

    // MARK: - Knowledge review (`knowledge_routes.rs`)

    /// `GET /api/v1/knowledge/flags?status=open` -> `{ flags: [] }`.
    func knowledgeFlags(status: String = "open") async throws -> [KnowledgeFlag] {
        let r: KnowledgeFlagsResponse = try await client.get(path: "knowledge/flags?status=\(status)")
        return r.flags.elements
    }

    /// `GET /api/v1/knowledge/proposals?status=proposed` -> `{ proposals: [] }`.
    func knowledgeProposals(status: String = "proposed") async throws -> [KnowledgeProposal] {
        let r: KnowledgeProposalsResponse = try await client.get(path: "knowledge/proposals?status=\(status)")
        return r.proposals.elements
    }

    /// `POST /api/v1/knowledge/proposals/{id}/approve`. 409 = no longer proposed.
    func approveProposal(id: String) async throws {
        try await client.post(path: "knowledge/proposals/\(settingsPathSegment(id))/approve")
    }

    /// `POST /api/v1/knowledge/proposals/{id}/reject`. 409 = no longer proposed.
    func rejectProposal(id: String) async throws {
        try await client.post(path: "knowledge/proposals/\(settingsPathSegment(id))/reject")
    }

    /// `POST /api/v1/knowledge/flags/{id}/dismiss`. 409 = already resolved.
    func dismissFlag(id: String) async throws {
        try await client.post(path: "knowledge/flags/\(settingsPathSegment(id))/dismiss")
    }

    // MARK: - Allternit Sleep (cmd/allternit-api/src/sleep/routes.rs)

    /// `GET /api/v1/memory/sleep/status`. A runtime from before Sleep has no
    /// such route: that is a 404 (`.notFound`), which the section shows as
    /// "too old for Sleep".
    func sleepStatus() async throws -> SleepStatus {
        try await sleepCall("memory/sleep/status", method: "GET")
    }

    /// `GET /api/v1/memory/sleep/cycles/:id` → `{ cycle, report }`.
    func sleepCycle(id: String) async throws -> SleepCycleDetail {
        try await sleepCall("memory/sleep/cycles/\(settingsPathSegment(id))", method: "GET")
    }

    /// `POST /api/v1/memory/sleep/cycles/:id/undo`. 409 `conflicts` lists the
    /// files later edits touched (nothing was changed then).
    func undoSleepCycle(id: String) async throws {
        let _: SleepIgnored = try await sleepCall("memory/sleep/cycles/\(settingsPathSegment(id))/undo", method: "POST")
    }

    /// `POST /api/v1/memory/sleep/run` with `{ kind }` (a nap by default).
    func runSleep(kind: String = "nap") async throws {
        let _: SleepIgnored = try await sleepCall("memory/sleep/run", method: "POST", body: ["kind": kind])
    }

    /// `PUT /api/v1/memory/sleep/settings`. Answers the settings as saved.
    func updateSleepSettings(_ update: SleepSettingsUpdate) async throws -> SleepSettings {
        try await sleepCall("memory/sleep/settings", method: "PUT", body: update)
    }

    /// `GET /api/v1/memory/disputes[?state=…]` → `{ disputes, count }`.
    func memoryDisputes(state: String? = nil) async throws -> [MemoryDispute] {
        let query = state.map { "?state=\(settingsQueryValue($0))" } ?? ""
        let body: DisputesBody = try await sleepCall("memory/disputes\(query)", method: "GET")
        return body.disputes ?? []
    }

    /// `POST /api/v1/memory/disputes/:id/resolve` with `{ choice }`.
    func resolveMemoryDispute(id: String, choice: MemoryDisputeChoice) async throws {
        let _: SleepIgnored = try await sleepCall(
            "memory/disputes/\(settingsPathSegment(id))/resolve",
            method: "POST",
            body: ["choice": choice.rawValue]
        )
    }

    private struct DisputesBody: Decodable { let disputes: [MemoryDispute]? }
    /// For answers the UI doesn't read (it reloads status after an action).
    private struct SleepIgnored: Decodable {}
    private struct NoBody: Encodable {}

    private func sleepCall<T: Decodable>(_ path: String, method: String) async throws -> T {
        try await sleepCall(path, method: method, body: Optional<NoBody>.none)
    }

    /// Same base URL, auth and routing (direct or the cloud relay) as every
    /// other call, through `APIClient.send`. Unlike `validate` alone, a
    /// failure keeps the parts of the body Sleep explains (`accepted`,
    /// `reason`, `stage`, `conflicts`) as a `MemorySleepError`.
    private func sleepCall<T: Decodable, B: Encodable>(_ path: String, method: String, body: B?) async throws -> T {
        var request = try await client.authorizedRequest(path: path, method: method)
        if let body { request.httpBody = try JSONEncoder().encode(body) }
        let data: Data
        let response: URLResponse
        do {
            (data, response) = try await client.send(request)
        } catch {
            throw try MemorySleepError.from(transport: error)
        }
        guard let http = response as? HTTPURLResponse else { throw APIError.invalidResponse }
        guard (200...299).contains(http.statusCode) else {
            // validate logs the failure and notes a compute-gate refusal for
            // the app's no-computer chip; Sleep throws its own error instead.
            do { try client.validate(response, data: data, method: method) } catch {}
            throw MemorySleepError.from(status: http.statusCode, data: data)
        }
        if T.self == SleepIgnored.self, let empty = SleepIgnored() as? T { return empty }
        do {
            return try JSONDecoder().decode(T.self, from: data)
        } catch {
            throw APIError.decoding(error)
        }
    }
}

// MARK: - Sleep errors

/// A failed Sleep call, sorted the way web `memory-sleep-client.ts` sorts it
/// (`kindFor`), with the same sentences. `message` is always one a person
/// can read: our own words for the states the section explains, the
/// server's calm sentence otherwise.
struct MemorySleepError: Error, LocalizedError, Equatable, Sendable {
    enum Kind: Equatable, Sendable {
        case offline
        case badRequest
        case signedOut
        case forbidden
        /// 404: no such cycle or dispute, or (on status) a runtime without Sleep.
        case notFound
        /// 409 with `accepted: false`: the stage it needs is off or not built.
        case refused
        /// 409 on undo or resolve: already done, or later edits are in the way.
        case conflict
        /// No computer paired: memory still works, Sleep can't.
        case noComputer
        /// A paired computer that did not answer.
        case computerOffline
        case server
    }

    let kind: Kind
    let status: Int
    let message: String
    /// 409 refusals: why (`sleep_disabled`, `stage_not_built`,
    /// `already_settled`, `already_undone`).
    var reason: String? = nil
    /// 409 refusals: the stage that is off.
    var stage: String? = nil
    /// 409 on undo: files later edits touched.
    var conflicts: [String] = []

    var errorDescription: String? { message }

    static let fallback: [Kind: String] = [
        .offline: "Couldn't reach Allternit. Check your connection and try again.",
        .badRequest: "That change wasn't accepted.",
        .signedOut: "Sign in to use Sleep.",
        .forbidden: "You don't have access to Sleep on this computer.",
        .notFound: "That sleep wasn't found.",
        .refused: "Sleep is off. Turn it on above.",
        .conflict: "This changed since you opened it.",
        .noComputer: "Memory and Sleep run on your computer. Open Allternit Desktop or pair a computer to use them.",
        .computerOffline: "Your computer isn't responding. Open Allternit Desktop or wake your cloud computer, then try again.",
        .server: "Sleep is unavailable right now. Try again in a moment.",
    ]

    /// Our own words for every refusal, never the raw code.
    static let refusalMessage: [String: String] = [
        "sleep_disabled": "That part of Sleep is off. Turn it on under \"What Sleep may do\".",
        "stage_not_built": "This computer's Allternit can't do that yet. Update it.",
        "already_settled": "This conflict was already settled.",
        "already_undone": "This sleep was already undone.",
    ]

    init(kind: Kind, status: Int, message: String? = nil, reason: String? = nil, stage: String? = nil, conflicts: [String] = []) {
        self.kind = kind
        self.status = status
        self.reason = reason
        self.stage = stage
        self.conflicts = conflicts
        if let reason, let ours = Self.refusalMessage[reason] {
            self.message = ours
        } else {
            self.message = message ?? Self.fallback[kind] ?? CalmCopy.fallback
        }
    }

    /// A non-2xx answer from the runtime (or the relay in front of it).
    static func from(status: Int, data: Data?) -> MemorySleepError {
        let object = data.flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] } ?? [:]
        let parsed = APIErrorBody.parse(data)
        let code = parsed.code?.lowercased()
        let accepted = object["accepted"] as? Bool
        let rawError = object["error"] as? String

        let kind: Kind
        switch status {
        case 400, 422: kind = .badRequest
        case 401: kind = .signedOut
        case 403: kind = .forbidden
        case 404: kind = .notFound
        // The compute gate: the account has no computer at all.
        case 409 where parsed.isComputeGateCode, 410 where parsed.isComputeGateCode, 428: kind = .noComputer
        case 409: kind = accepted == false ? .refused : .conflict
        case 503 where code == "runtime_unavailable": kind = .noComputer
        case 503 where code == "runtime_offline" || code == "runtime_warming": kind = .computerOffline
        default: kind = .server
        }

        let reason = (object["reason"] as? String) ?? (rawError == "already undone" ? "already_undone" : nil)
        let conflicts = (object["conflicts"] as? [Any])?.compactMap { $0 as? String } ?? []
        let serverMessage = calmMessageText(parsed.message)
        let ours: Bool
        switch kind {
        case .signedOut, .noComputer, .computerOffline, .refused: ours = true
        case .server: ours = serverMessage == nil
        default: ours = false
        }
        return MemorySleepError(
            kind: kind,
            status: status,
            message: ours ? nil : serverMessage,
            reason: reason,
            stage: object["stage"] as? String,
            conflicts: conflicts
        )
    }

    /// A call that never got an answer. Rethrows cancellation as-is.
    static func from(transport error: Error) throws -> MemorySleepError {
        if error is CancellationError { throw error }
        if let url = error as? URLError {
            if url.code == .cancelled { throw CancellationError() }
            return MemorySleepError(kind: .offline, status: 0)
        }
        if let relay = error as? RuntimeRelayError {
            switch relay {
            case .noComputer: return MemorySleepError(kind: .noComputer, status: 0)
            case .unreachable: return MemorySleepError(kind: .computerOffline, status: 0)
            case .socketUnavailable: return MemorySleepError(kind: .server, status: 0)
            }
        }
        if let api = error as? APIError {
            switch api {
            // `APIClient.send` throws a 503 itself when no runtime is paired
            // (EnvironmentStore.apiRoute → `.unavailable`).
            case .httpError(let status, _) where status == 503:
                return MemorySleepError(kind: .noComputer, status: status)
            case .httpError(let status, _) where api.isComputeGateRefusal:
                return MemorySleepError(kind: .noComputer, status: status)
            case .transport(let inner):
                return try from(transport: inner)
            default:
                return MemorySleepError(kind: .server, status: 0)
            }
        }
        return MemorySleepError(kind: .server, status: 0)
    }
}

// MARK: - Knowledge models

private struct KnowledgeFlagsResponse: Decodable { let flags: LossyArray<KnowledgeFlag> }
private struct KnowledgeProposalsResponse: Decodable { let proposals: LossyArray<KnowledgeProposal> }

/// What a flag or proposal is about: `skill`, `domain` (business
/// knowledge) or `data_access`.
enum KnowledgeTargetKind {
    static func label(_ raw: String) -> String {
        switch raw {
        case "skill": return "Skill"
        case "domain": return "Business knowledge"
        case "data_access": return "Data access"
        default: return "Knowledge"
        }
    }
}

/// An agent's note that knowledge was missing, wrong or stale.
struct KnowledgeFlag: Decodable, Identifiable, Hashable, Sendable {
    let id: String
    let kind: String          // missing | wrong | stale
    let targetKind: String
    let targetRef: String?
    let evidence: String
    let suggestedFix: String?
    let status: String

    var kindLabel: String {
        switch kind {
        case "missing": return "Missing"
        case "wrong": return "Wrong"
        case "stale": return "Out of date"
        default: return "Flagged"
        }
    }

    private enum CodingKeys: String, CodingKey {
        case id, kind, evidence, status
        case targetKind = "target_kind"
        case targetRef = "target_ref"
        case suggestedFix = "suggested_fix"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        kind = (try? c.decode(String.self, forKey: .kind)) ?? ""
        targetKind = (try? c.decode(String.self, forKey: .targetKind)) ?? ""
        targetRef = (try? c.decodeIfPresent(String.self, forKey: .targetRef)) ?? nil
        evidence = (try? c.decode(String.self, forKey: .evidence)) ?? ""
        suggestedFix = (try? c.decodeIfPresent(String.self, forKey: .suggestedFix)) ?? nil
        status = (try? c.decode(String.self, forKey: .status)) ?? "open"
    }
}

/// One citation on a proposal: a flag or a past-chat observation.
struct KnowledgeEvidence: Decodable, Hashable, Sendable {
    let flagId: String?
    let observationId: String?
    let text: String?

    private enum CodingKeys: String, CodingKey {
        case flagId = "flag_id", observationId = "observation_id", text, excerpt, evidence
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        flagId = (try? c.decodeIfPresent(String.self, forKey: .flagId)) ?? nil
        observationId = (try? c.decodeIfPresent(String.self, forKey: .observationId)) ?? nil
        let textValue: String? = (try? c.decodeIfPresent(String.self, forKey: .text)) ?? nil
        let excerptValue: String? = (try? c.decodeIfPresent(String.self, forKey: .excerpt)) ?? nil
        let evidenceValue: String? = (try? c.decodeIfPresent(String.self, forKey: .evidence)) ?? nil
        text = textValue ?? excerptValue ?? evidenceValue
    }
}

/// A change the curator or an agent wants to make to the knowledge files.
struct KnowledgeProposal: Decodable, Identifiable, Hashable, Sendable {
    let id: String
    let targetKind: String
    let targetRef: String
    let title: String
    let body: String
    let base: String?
    let evidence: [KnowledgeEvidence]
    let origin: String
    let status: String

    private enum CodingKeys: String, CodingKey {
        case id, title, body, base, evidence, origin, status
        case targetKind = "target_kind"
        case targetRef = "target_ref"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        targetKind = (try? c.decode(String.self, forKey: .targetKind)) ?? ""
        targetRef = (try? c.decode(String.self, forKey: .targetRef)) ?? ""
        title = (try? c.decode(String.self, forKey: .title)) ?? "Untitled"
        body = (try? c.decode(String.self, forKey: .body)) ?? ""
        base = (try? c.decodeIfPresent(String.self, forKey: .base)) ?? nil
        evidence = (try? c.decodeIfPresent(LossyArray<KnowledgeEvidence>.self, forKey: .evidence))??.elements ?? []
        origin = (try? c.decode(String.self, forKey: .origin)) ?? ""
        status = (try? c.decode(String.self, forKey: .status)) ?? "proposed"
    }
}
