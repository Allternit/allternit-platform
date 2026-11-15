import Foundation

// -----------------------------------------------------------------------------
// Vendor bots as real group-chat members (allternit-ai
// src/lib/bots/vendor-group-turn.ts). A vendor-bound bot (execution binding
// `vendor`: Grok, OpenMausBot, Poke…) never answers with a native brain. In a
// room it answers through the subscription gateway on one standing thread
// per room, so the vendor keeps a single conversation for the room. Each turn
// carries the room transcript; the vendor's reply comes back into the room
// log like any other member's.
//
//   GET  gateway/execution-bindings            (bulk; per bot as fallback)
//   GET  gateway/bots/:id/execution-binding
//   GET  threads?botId=  GET threads/:id  POST threads
//   POST agent-sessions/:sessionId/messages    { text, metadata: { correlationId } }
//   POST threads/:id/gateway/sync   GET threads/:id/events?after=
// -----------------------------------------------------------------------------

struct BotExecutionBindingRecord: Decodable, Sendable {
    let botId: String
    let type: String
    let vendor: String?
    let externalAgentName: String?

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        botId = try c.decodeIfPresent(String.self, forKey: .botId) ?? ""
        type = try c.decodeIfPresent(String.self, forKey: .type) ?? "allternit"
        vendor = try c.decodeIfPresent(String.self, forKey: .vendor)
        externalAgentName = try c.decodeIfPresent(String.self, forKey: .externalAgentName)
    }

    enum CodingKeys: String, CodingKey { case botId, type, vendor, externalAgentName }

    var isVendor: Bool { type == "vendor" }
}

private struct BindingList: Decodable { let bindings: [BotExecutionBindingRecord]? }
private struct BindingEnvelope: Decodable { let binding: BotExecutionBindingRecord? }

private struct RoomThread: Decodable {
    let id: String
    let title: String?
    let createdBy: String?
    let currentSessionId: String?
}

private struct RoomThreadEnvelope: Decodable { let thread: RoomThread }
private struct RoomThreadList: Decodable { let threads: [RoomThread]? }

private struct CreateRoomThreadBody: Encodable {
    let botId: String
    let title: String
    let kind: String
    let objective: String
    let createdBy: String
}

private struct VendorTurnBody: Encodable {
    struct Metadata: Encodable { let correlationId: String }
    let text: String
    let metadata: Metadata
}

private struct VendorTurnReply: Decodable {
    struct Metadata: Decodable { let pending: Bool? }
    let content: String?
    let metadata: Metadata?
}

private struct ThreadEventsPage: Decodable {
    let events: [JSONValue]?
    let cursor: JSONValue?
}

private struct EmptyJSON: Encodable {}

/// The outcome of one vendor member turn.
struct VendorTurnOutcome: Sendable {
    var text: String? = nil
    /// Plain-language reason when the vendor did not answer.
    var problem: String? = nil
}

enum VendorGroupTurn {
    private static let pendingTimeout: TimeInterval = 150
    private static let pollNanos: UInt64 = 2_000_000_000
    private static let roomThreadsKey = "allternit-group-vendor-threads"

    @MainActor private static var vendorCache: [String: BotExecutionBindingRecord?] = [:]
    @MainActor private static var bulkLoadedAt: Date? = nil

    private static func escape(_ value: String) -> String {
        value.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed.subtracting(CharacterSet(charactersIn: "/"))) ?? value
    }

    /// The bot's execution binding (cached per launch; bindings change rarely).
    @MainActor
    static func binding(for botId: String) async -> BotExecutionBindingRecord? {
        if let cached = vendorCache[botId] { return cached }
        if bulkLoadedAt == nil || Date().timeIntervalSince(bulkLoadedAt ?? .distantPast) > 30 {
            if let list: BindingList = try? await APIClient.shared.get(path: "gateway/execution-bindings") {
                bulkLoadedAt = Date()
                for row in list.bindings ?? [] where !row.botId.isEmpty { vendorCache[row.botId] = row }
                if let hit = vendorCache[botId] { return hit }
                vendorCache[botId] = .some(nil)
                return nil
            }
        }
        let envelope: BindingEnvelope? = try? await APIClient.shared.get(path: "gateway/bots/\(escape(botId))/execution-binding")
        vendorCache[botId] = .some(envelope?.binding)
        return envelope?.binding
    }

    /// Drops cached bindings (a bot was connected to / disconnected from a vendor).
    @MainActor
    static func invalidate() {
        vendorCache = [:]
        bulkLoadedAt = nil
    }

    /// Members whose bot runs on a vendor: added as `source: "vendor"`, or
    /// whose execution binding says so.
    @MainActor
    static func vendorBoundMembers(_ members: [GroupChatMember]) async -> Set<String> {
        var out = Set<String>()
        for member in members {
            if member.source == "vendor" { out.insert(member.botId); continue }
            if await binding(for: member.botId)?.isVendor == true { out.insert(member.botId) }
        }
        return out
    }

    // MARK: Room thread

    private static func rememberedThread(room: String, botId: String) -> String? {
        let map = UserDefaults.standard.dictionary(forKey: roomThreadsKey) as? [String: [String: String]]
        return map?[room]?[botId]
    }

    private static func rememberThread(_ threadId: String, room: String, botId: String) {
        var map = (UserDefaults.standard.dictionary(forKey: roomThreadsKey) as? [String: [String: String]]) ?? [:]
        var forRoom = map[room] ?? [:]
        forRoom[botId] = threadId
        map[room] = forRoom
        UserDefaults.standard.set(map, forKey: roomThreadsKey)
    }

    /// The room's standing thread (and session) for this vendor bot: the one
    /// this device used, else the one another device made (same title, made
    /// by "group"), else a new one.
    private static func roomThreadSession(room: BotGroupChat, member: GroupChatMember) async throws -> (threadId: String, sessionId: String) {
        if let known = rememberedThread(room: room.id, botId: member.botId),
           let envelope: RoomThreadEnvelope = try? await APIClient.shared.get(path: "threads/\(escape(known))"),
           let sessionId = envelope.thread.currentSessionId {
            return (known, sessionId)
        }
        let title = "#\(room.name)"
        let botQuery = member.botId.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? member.botId
        if let list: RoomThreadList = try? await APIClient.shared.get(path: "threads?botId=\(botQuery)"),
           let existing = list.threads?.first(where: { $0.title == title && $0.createdBy == "group" && $0.currentSessionId != nil }),
           let sessionId = existing.currentSessionId {
            rememberThread(existing.id, room: room.id, botId: member.botId)
            return (existing.id, sessionId)
        }
        let created: RoomThreadEnvelope = try await APIClient.shared.post(path: "threads", body: CreateRoomThreadBody(
            botId: member.botId, title: title, kind: "standing",
            objective: "Group chat \"\(room.name)\"", createdBy: "group"
        ))
        guard let sessionId = created.thread.currentSessionId else {
            throw APIError.httpError(statusCode: 500, message: "No session for \(member.displayName)'s room thread")
        }
        rememberThread(created.thread.id, room: room.id, botId: member.botId)
        return (created.thread.id, sessionId)
    }

    // MARK: Events

    private static func string(_ value: JSONValue?) -> String? {
        if case .string(let s) = value { return s }
        return nil
    }

    private static func object(_ value: JSONValue?) -> [String: JSONValue]? {
        if case .object(let o) = value { return o }
        return nil
    }

    private static func correlation(_ event: JSONValue) -> String? {
        guard let o = object(event) else { return nil }
        let inner = object(o["event"]) ?? object(o["payload"]) ?? [:]
        return string(o["correlationId"]) ?? string(o["correlation_id"])
            ?? string(inner["correlationId"]) ?? string(inner["correlation_id"])
    }

    private static func reply(_ event: JSONValue) -> String? {
        guard let o = object(event) else { return nil }
        let inner = object(o["event"]) ?? o
        let type = string(inner["type"]) ?? string(o["type"])
        guard type == "agent.message.completed" else { return nil }
        let payload = object(inner["payload"]) ?? object(o["payload"]) ?? [:]
        return string(payload["reply"]) ?? string(payload["text"]) ?? string(inner["content"])
    }

    private static func cursorString(_ value: JSONValue?) -> String? {
        switch value {
        case .string(let s): return s
        case .number(let n): return String(Int(n))
        default: return nil
        }
    }

    /// A reply the server accepted but hadn't finished: follow the thread's
    /// event log for this turn.
    private static func awaitPendingReply(threadId: String, correlationId: String,
                                          isCancelled: @escaping @Sendable @MainActor () -> Bool) async -> String? {
        let deadline = Date().addingTimeInterval(pendingTimeout)
        var cursor: String? = nil
        while Date() < deadline {
            if await isCancelled() { return nil }
            try? await APIClient.shared.post(path: "threads/\(escape(threadId))/gateway/sync", body: EmptyJSON())
            let after = cursor.map { "?after=\($0.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? $0)" } ?? ""
            if let page: ThreadEventsPage = try? await APIClient.shared.get(path: "threads/\(escape(threadId))/events\(after)") {
                for event in page.events ?? [] where correlation(event) == correlationId {
                    if let text = reply(event) { return text }
                }
                cursor = cursorString(page.cursor) ?? cursor
            }
            try? await Task.sleep(nanoseconds: pollNanos)
        }
        return nil
    }

    // MARK: Turn

    /// One vendor member turn in a room. `prompt` is the room-history prompt.
    static func run(room: BotGroupChat, member: GroupChatMember, prompt: String,
                    isCancelled: @escaping @Sendable @MainActor () -> Bool) async -> VendorTurnOutcome {
        let correlationId = "room-\(room.id)-\(member.botId)-\(Int(Date().timeIntervalSince1970 * 1000))-\(UUID().uuidString.prefix(6).lowercased())"
        do {
            let (threadId, sessionId) = try await roomThreadSession(room: room, member: member)
            let response: VendorTurnReply = try await APIClient.shared.post(
                path: "agent-sessions/\(escape(sessionId))/messages",
                body: VendorTurnBody(text: prompt, metadata: .init(correlationId: correlationId))
            )
            var text = response.metadata?.pending == true ? nil : response.content?.trimmingCharacters(in: .whitespacesAndNewlines)
            if text?.isEmpty ?? true {
                text = await awaitPendingReply(threadId: threadId, correlationId: correlationId, isCancelled: isCancelled)?
                    .trimmingCharacters(in: .whitespacesAndNewlines)
            }
            Task { try? await APIClient.shared.post(path: "threads/\(escape(threadId))/gateway/sync", body: EmptyJSON()) }
            guard let text, !text.isEmpty else {
                if await isCancelled() { return VendorTurnOutcome() }
                return VendorTurnOutcome(problem: "\(member.displayName) didn't answer in time.")
            }
            if ["(pass)", "pass", "pass."].contains(text.lowercased()) { return VendorTurnOutcome() }
            return VendorTurnOutcome(text: text)
        } catch APIError.httpError(let status, let message) where status == 428 || status == 409 {
            return VendorTurnOutcome(problem: status == 428
                ? "\(member.displayName) needs your approval in its chat."
                : "\(member.displayName) needs attention: \(message ?? "its vendor connection isn't ready").")
        } catch {
            return VendorTurnOutcome(problem: "\(member.displayName) couldn't be reached.")
        }
    }
}
