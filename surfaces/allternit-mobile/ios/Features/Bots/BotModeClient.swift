import Foundation

// -----------------------------------------------------------------------------
// Bot Mode REST surface — every endpoint the PWA's Bot Mode reads that the
// existing clients (AgentClient, ThreadsClient, BotEventsClient,
// BotDesktopClient, AgentChatClient) don't already cover. Paths mirror the
// web exactly (allternit-ai src/lib/bots/*, src/lib/computers-*.ts,
// src/lib/gateway/api.ts) and go through `APIClient.shared` so the relay /
// direct routing is the same as every other platform client.
// -----------------------------------------------------------------------------

// MARK: - Models

/// `GET /api/v1/bot-projects` row (lib/bots/bot-projects.ts BotProjectSummary).
struct BotProjectSummary: Decodable, Identifiable, Hashable, Sendable {
    struct Counts: Decodable, Hashable, Sendable {
        var waiting: Int = 0
        var working: Int = 0
        var queued: Int = 0
        var idle: Int = 0
        var resolved: Int = 0

        init() {}

        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            waiting = try c.decodeIfPresent(Int.self, forKey: .waiting) ?? 0
            working = try c.decodeIfPresent(Int.self, forKey: .working) ?? 0
            queued = try c.decodeIfPresent(Int.self, forKey: .queued) ?? 0
            idle = try c.decodeIfPresent(Int.self, forKey: .idle) ?? 0
            resolved = try c.decodeIfPresent(Int.self, forKey: .resolved) ?? 0
        }

        enum CodingKeys: String, CodingKey { case waiting, working, queued, idle, resolved }
    }

    let id: String
    let title: String
    let description: String?
    let botIds: [String]
    let counts: Counts
    let lastActivityAt: String?
    let archived: Bool
    let favorite: Bool

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        title = try c.decodeIfPresent(String.self, forKey: .title) ?? "Untitled project"
        description = try c.decodeIfPresent(String.self, forKey: .description)
        botIds = try c.decodeIfPresent([String].self, forKey: .botIds) ?? []
        counts = try c.decodeIfPresent(Counts.self, forKey: .counts) ?? Counts()
        lastActivityAt = try c.decodeIfPresent(String.self, forKey: .lastActivityAt)
        archived = try c.decodeIfPresent(Bool.self, forKey: .archived) ?? false
        favorite = try c.decodeIfPresent(Bool.self, forKey: .favorite) ?? false
    }

    enum CodingKeys: String, CodingKey {
        case id, title, description, botIds, counts, lastActivityAt, archived, favorite
    }
}

private struct BotProjectListResponse: Decodable {
    let projects: [BotProjectSummary]
}

/// One member of a bot group chat (lib/bots/group-chat.types.ts).
struct GroupChatMember: Codable, Hashable, Sendable, Identifiable {
    var botId: String
    var displayName: String
    var handle: String
    var source: String
    var providerId: String?

    var id: String { botId }
}

/// One line of a group chat's log.
struct GroupChatMessage: Codable, Hashable, Sendable, Identifiable {
    var id: String
    /// "user" | "bot"
    var from: String
    var botId: String?
    var displayName: String?
    var text: String
    var timestamp: String
    var threadId: String?
}

/// A group chat room as the app uses it — decoded from the server's
/// `BotGroupRecord` (cmd/allternit-api/src/bot_group_routes.rs), whose
/// members and log ride as JSON strings.
struct BotGroupChat: Identifiable, Hashable, Sendable {
    var id: String
    var name: String
    var image: String?
    var members: [GroupChatMember]
    var log: [GroupChatMessage]
    var createdAt: String
    var updatedAt: String
}

/// Wire shape of `/api/v1/bot-groups` (snake_case keys on the struct itself).
private struct BotGroupRecord: Codable {
    let id: String
    let name: String
    let image: String?
    let members_json: String
    let log_json: String
    let created_at: String
    let updated_at: String
}

/// `GET /api/v1/cowork/memory?bot=` row (lib/bots/bot-memory-server.ts).
struct BotMemoryItem: Decodable, Identifiable, Hashable, Sendable {
    let id: String
    let content: String
    let type: String
    let source: String?
    let createdAt: String

    enum CodingKeys: String, CodingKey {
        case id, content, type, source
        case createdAt = "created_at"
    }
}

private struct BotMemoryListResponse: Decodable {
    let memories: [BotMemoryItem]?
}

/// `GET /api/v1/gateway/bots/:id/channels` row (lib/gateway/api.ts BotChannel).
struct BotChannel: Decodable, Identifiable, Hashable, Sendable {
    let accountId: String
    let provider: String
    let displayName: String?
    let handle: String?
    let state: String
    let enabled: Bool
    let isDefault: Bool

    var id: String { accountId }

    var label: String {
        if let displayName, !displayName.isEmpty { return displayName }
        return provider.prefix(1).uppercased() + provider.dropFirst()
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        accountId = try c.decode(String.self, forKey: .accountId)
        provider = try c.decodeIfPresent(String.self, forKey: .provider) ?? "channel"
        displayName = try c.decodeIfPresent(String.self, forKey: .displayName)
        handle = try c.decodeIfPresent(String.self, forKey: .handle)
        state = try c.decodeIfPresent(String.self, forKey: .state) ?? "unknown"
        enabled = try c.decodeIfPresent(Bool.self, forKey: .enabled) ?? false
        isDefault = try c.decodeIfPresent(Bool.self, forKey: .isDefault) ?? false
    }

    enum CodingKeys: String, CodingKey {
        case accountId, provider, displayName, handle, state, enabled, isDefault
    }
}

private struct BotChannelListResponse: Decodable {
    let channels: [BotChannel]
}

/// `GET /api/v1/computers` row (lib/computers-api.ts Computer), subsetted.
struct ComputerRecord: Decodable, Identifiable, Hashable, Sendable {
    let id: String
    let kind: String
    let provider: String
    let status: String
    let name: String
    let os: String?
    let botId: String?
    let host: String?

    var isRunning: Bool { status == "running" }

    var kindLabel: String {
        switch kind {
        case "local": return "This network"
        case "byo_vps": return "Your server"
        case "managed": return "Cloud"
        case "byoc": return "Your cloud"
        case "cloud_desktop": return "Cloud desktop"
        default: return kind
        }
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        kind = try c.decodeIfPresent(String.self, forKey: .kind) ?? "managed"
        provider = try c.decodeIfPresent(String.self, forKey: .provider) ?? ""
        status = try c.decodeIfPresent(String.self, forKey: .status) ?? "unknown"
        name = try c.decodeIfPresent(String.self, forKey: .name) ?? "Computer"
        os = try c.decodeIfPresent(String.self, forKey: .os)
        botId = try c.decodeIfPresent(String.self, forKey: .botId)
        host = try c.decodeIfPresent(String.self, forKey: .host)
    }

    enum CodingKeys: String, CodingKey {
        case id, kind, provider, status, name, os, host
        case botId = "bot_id"
    }
}

private struct ComputerListResponse: Decodable {
    let computers: [ComputerRecord]
}

/// `GET /api/v1/computers/:id/status` (computer_embed.rs).
struct ComputerViewStatus: Decodable, Sendable {
    struct WS: Decodable, Sendable { let vnc: String }
    let id: String
    let status: String
    let name: String?
    let ws: WS
}

/// Control lease (computer_control_lease.rs; lib/computers-control.ts).
struct ComputerControlHolder: Decodable, Hashable, Sendable {
    let kind: String
    let id: String
    let label: String?
    let deviceId: String?

    enum CodingKeys: String, CodingKey {
        case kind, id, label
        case deviceId = "device_id"
    }

    var displayLabel: String {
        if let label, !label.isEmpty { return label }
        switch kind {
        case "agent": return "An agent"
        case "session": return "A session"
        case "bot": return "A bot"
        default: return "Someone"
        }
    }
}

struct ComputerControlLease: Decodable, Hashable, Sendable {
    let computerId: String
    let holder: ComputerControlHolder
    let expiresAt: String?

    enum CodingKeys: String, CodingKey {
        case holder
        case computerId = "computer_id"
        case expiresAt = "expires_at"
    }
}

struct ComputerControlState: Decodable, Sendable {
    let control: ComputerControlLease?
}

private struct ComputerControlTakeResponse: Decodable {
    let control: ComputerControlLease?
}

private struct ComputerWSTokenResponse: Decodable {
    let token: String
}

// MARK: - Client

final class BotModeClient: @unchecked Sendable {
    static let shared = BotModeClient()

    private let client: APIClient

    init(client: APIClient = .shared) {
        self.client = client
    }

    // MARK: Threads (thread_routes.rs)

    /// Every bot's threads (the Threads page + rail waiting badge).
    func listAllThreads() async throws -> [BotThread] {
        let response: BotThreadListResponse = try await client.get(path: "threads?includeResolved=true")
        return response.threads.filter { !$0.incognito }
    }

    /// A bot project's threads (project home).
    func listProjectThreads(projectId: String) async throws -> [BotThread] {
        let response: BotThreadListResponse = try await client.get(
            path: "threads?projectId=\(Self.query(projectId))&includeResolved=true"
        )
        return response.threads.filter { !$0.incognito }
    }

    func approveThread(id: String) async throws {
        try await client.post(path: "threads/\(Self.escape(id))/approve", body: EmptyBody())
    }

    func declineThread(id: String) async throws {
        try await client.post(path: "threads/\(Self.escape(id))/decline", body: EmptyBody())
    }

    // MARK: Bot projects

    func listBotProjects() async throws -> [BotProjectSummary] {
        let response: BotProjectListResponse = try await client.get(path: "bot-projects")
        return response.projects
    }

    // MARK: Group chats (bot_group_routes.rs)

    func listGroups() async throws -> [BotGroupChat] {
        let rows: [BotGroupRecord] = try await client.get(path: "bot-groups")
        let decoder = JSONDecoder()
        return rows.map { row in
            BotGroupChat(
                id: row.id,
                name: row.name,
                image: row.image,
                members: (try? decoder.decode([GroupChatMember].self, from: Data(row.members_json.utf8))) ?? [],
                log: (try? decoder.decode([GroupChatMessage].self, from: Data(row.log_json.utf8))) ?? [],
                createdAt: row.created_at,
                updatedAt: row.updated_at
            )
        }
    }

    /// Upsert — the server keeps the last 80 log lines the web sends.
    func saveGroup(_ group: BotGroupChat) async throws {
        let encoder = JSONEncoder()
        let members = String(data: try encoder.encode(group.members), encoding: .utf8) ?? "[]"
        let log = String(data: try encoder.encode(Array(group.log.suffix(80))), encoding: .utf8) ?? "[]"
        try await client.put(path: "bot-groups", body: BotGroupRecord(
            id: group.id,
            name: group.name,
            image: group.image,
            members_json: members,
            log_json: log,
            created_at: group.createdAt,
            updated_at: group.updatedAt
        ))
    }

    func deleteGroup(id: String) async throws {
        try await client.delete(path: "bot-groups/\(Self.escape(id))")
    }

    // MARK: Memory (cowork memory, scoped by bot)

    func listMemories(botId: String) async throws -> [BotMemoryItem] {
        let response: BotMemoryListResponse = try await client.get(
            path: "cowork/memory?bot=\(Self.query(botId))&limit=200"
        )
        return response.memories ?? []
    }

    func addMemory(botId: String, content: String) async throws {
        try await client.post(path: "cowork/memory", body: AddMemoryBody(bot: botId, content: content, type: "fact", source: "user"))
    }

    func forgetMemory(id: String) async throws {
        try await client.delete(path: "cowork/memory/\(Self.escape(id))")
    }

    // MARK: Channels (gateway)

    func listChannels(botId: String) async throws -> [BotChannel] {
        let response: BotChannelListResponse = try await client.get(path: "gateway/bots/\(Self.escape(botId))/channels")
        return response.channels
    }

    func setChannel(botId: String, accountId: String, enabled: Bool) async throws {
        try await client.put(
            path: "gateway/bots/\(Self.escape(botId))/channels/\(Self.escape(accountId))",
            body: SetChannelBody(enabled: enabled)
        )
    }

    // MARK: Main chat session

    /// Creates a bot chat session with Bot Mode metadata — the same shape as
    /// the web's `createCanonicalChat` (`botCanonicalFor`) or a bot thread
    /// session. The metadata bag is string-valued on iOS.
    func createBotSession(botId: String, botName: String, name: String,
                          metadata: [String: String]) async throws -> AgentSession {
        var bag = metadata
        bag["isBot"] = "true"
        bag["agentId"] = botId
        bag["botName"] = botName
        let body = CreateAgentSessionRequest(
            name: name,
            originSurface: "chat",
            sessionMode: "agent",
            agentId: botId,
            agentName: botName,
            metadata: bag
        )
        return try await client.post(path: "agent-sessions", body: body)
    }

    /// Stores the bot's greeting as the chat's first message, from the bot,
    /// without a model turn (web bot-greeting.ts `persistBotGreeting`):
    /// `POST /api/v1/agent-sessions/:id/messages` role assistant + noReply,
    /// labelled `source: "bot-greeting"`.
    func storeGreeting(sessionId: String, text: String) async throws {
        struct Body: Encodable {
            let text: String
            let role = "assistant"
            let noReply = true
            let metadata = ["source": "bot-greeting"]
        }
        try await client.post(path: "agent-sessions/\(Self.query(sessionId))/messages", body: Body(text: text))
    }

    // MARK: Computers (computer_routes.rs / computer_embed.rs / computer_ws.rs)

    func listComputers(botId: String? = nil) async throws -> [ComputerRecord] {
        let path = botId.map { "computers?bot_id=\(Self.query($0))" } ?? "computers"
        let response: ComputerListResponse = try await client.get(path: path)
        return response.computers
    }

    func startComputer(id: String) async throws {
        try await client.post(path: "computers/\(Self.escape(id))/start", body: EmptyBody())
    }

    func viewStatus(computerId: String) async throws -> ComputerViewStatus {
        try await client.get(path: "computers/\(Self.escape(computerId))/status")
    }

    func controlState(computerId: String) async throws -> ComputerControlState {
        try await client.get(path: "computers/\(Self.escape(computerId))/control")
    }

    /// Takes the control lease for this device. The device id header is what
    /// the web interceptor adds so control lands on this device
    /// (computer_routes.rs `user_holder`).
    func takeControl(computerId: String) async throws {
        var request = try await client.authorizedRequest(
            path: "computers/\(Self.escape(computerId))/control/take", method: "POST"
        )
        request.httpBody = Data("{}".utf8)
        request.setValue(Self.deviceId, forHTTPHeaderField: "x-allternit-device-id")
                request.setValue("ios", forHTTPHeaderField: "x-allternit-device-platform")
        let (data, response) = try await client.send(request)
        try client.validate(response, data: data)
    }

    func releaseControl(computerId: String) async throws {
        try await client.post(path: "computers/\(Self.escape(computerId))/control/release", body: EmptyBody())
    }

    /// Mints a short-lived VNC token. Full control is only granted to the
    /// lease holder; everyone else watches read-only (server-enforced).
    func vncToken(computerId: String, readOnly: Bool) async throws -> String {
        let response: ComputerWSTokenResponse = try await client.post(
            path: "computers/\(Self.escape(computerId))/ws-token",
            body: WSTokenBody(purpose: "vnc", read_only: readOnly)
        )
        return response.token
    }

    // MARK: Helpers

    /// A stable per-install device id for the control lease holder (kept in
    /// UserDefaults so it is readable off the main actor).
    static var deviceId: String {
        let key = "allternit-bot-mode-device-id"
        if let existing = UserDefaults.standard.string(forKey: key) { return existing }
        let fresh = UUID().uuidString
        UserDefaults.standard.set(fresh, forKey: key)
        return fresh
    }

    private struct EmptyBody: Encodable {}

    private struct AddMemoryBody: Encodable {
        let bot: String
        let content: String
        let type: String
        let source: String
    }

    private struct SetChannelBody: Encodable {
        let enabled: Bool
    }

    private struct WSTokenBody: Encodable {
        let purpose: String
        let read_only: Bool
    }

    static func escape(_ id: String) -> String {
        id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id
    }

    static func query(_ value: String) -> String {
        var allowed = CharacterSet.urlQueryAllowed
        allowed.remove(charactersIn: "&=+?")
        return value.addingPercentEncoding(withAllowedCharacters: allowed) ?? value
    }
}
