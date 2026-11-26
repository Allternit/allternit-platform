import Foundation

/// A skill an MCP server (or an installed app) serves to the person's chats.
/// `GET /api/v1/mcp/skills` (allternit-api). Decoding is tolerant: an item with no
/// `id` gets `server:name`, one with no `enabled` counts as on.
struct ServerSkill: Identifiable, Hashable, Sendable, Decodable {
    let id: String
    let source: String
    let from: String
    let server: String
    let connectorId: String
    let uri: String
    let name: String
    let description: String
    var enabled: Bool
    let defaultEnabled: Bool

    enum CodingKeys: String, CodingKey {
        case id, source, from, server, connectorId, uri, name, description, enabled, defaultEnabled
    }

    init(id: String, source: String = "server", from: String, server: String = "", connectorId: String = "",
         uri: String = "", name: String, description: String = "", enabled: Bool = true, defaultEnabled: Bool = true) {
        self.id = id; self.source = source; self.from = from; self.server = server
        self.connectorId = connectorId; self.uri = uri; self.name = name
        self.description = description; self.enabled = enabled; self.defaultEnabled = defaultEnabled
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let name = (try? c.decode(String.self, forKey: .name)) ?? ""
        let server = (try? c.decode(String.self, forKey: .server)) ?? ""
        self.name = name
        self.server = server
        self.id = (try? c.decode(String.self, forKey: .id)) ?? "\(server):\(name)"
        self.source = (try? c.decode(String.self, forKey: .source)) ?? "server"
        self.from = (try? c.decode(String.self, forKey: .from)) ?? server
        self.connectorId = (try? c.decode(String.self, forKey: .connectorId)) ?? ""
        self.uri = (try? c.decode(String.self, forKey: .uri)) ?? ""
        self.description = (try? c.decode(String.self, forKey: .description)) ?? ""
        self.enabled = (try? c.decode(Bool.self, forKey: .enabled)) ?? true
        self.defaultEnabled = (try? c.decode(Bool.self, forKey: .defaultEnabled)) ?? true
    }

    /// "From <from>" for a server, "From the <from> app" for an app skill.
    var sourceLine: String { source == "app" ? "From the \(from) app" : "From \(from)" }
}

/// Server skills routes. They live on the computer next to the MCP connectors,
/// so calls go through `InstanceConnection.runtimeAPIData` like CapabilitiesClient.
final class ServerSkillsClient: @unchecked Sendable {
    static let shared = ServerSkillsClient()
    private let client: APIClient
    init(client: APIClient = .shared) { self.client = client }

    private struct ListEnvelope: Decodable { let skills: [ServerSkill]? }
    private struct PrefBody: Encodable { let id: String; let enabled: Bool }

    /// `GET /api/v1/mcp/skills`. A 404 (old server) or an empty answer is `[]`.
    func listServerSkills() async throws -> [ServerSkill] {
        let request = try await client.authorizedRequest(path: "mcp/skills")
        let (data, response) = try await InstanceConnection.runtimeAPIData(for: request, session: client.session)
        if let http = response as? HTTPURLResponse, http.statusCode == 404 { return [] }
        try client.validate(response, data: data)
        return Self.decodeList(data)
    }

    /// `PUT /api/v1/mcp/skills/prefs` `{ id, enabled }`.
    func setSkillEnabled(id: String, enabled: Bool) async throws {
        var request = try await client.authorizedRequest(path: "mcp/skills/prefs", method: "PUT")
        request.httpBody = try JSONEncoder().encode(PrefBody(id: id, enabled: enabled))
        let (data, response) = try await InstanceConnection.runtimeAPIData(for: request, session: client.session)
        try client.validate(response, data: data)
    }

    static func decodeList(_ data: Data) -> [ServerSkill] {
        let decoder = JSONDecoder()
        if let list = try? decoder.decode([ServerSkill].self, from: data) { return list }
        return (try? decoder.decode(ListEnvelope.self, from: data))?.skills ?? []
    }
}

/// Cached list of server skills (5-minute TTL). The composer list, the + sheet
/// row and Customize all read this one store.
@MainActor
final class ServerSkillsStore: ObservableObject {
    static let shared = ServerSkillsStore()
    private static let ttl: TimeInterval = 300

    @Published private(set) var skills: [ServerSkill] = []
    @Published var saveError: String?
    private var fetchedAt: Date?
    private var inflight = false

    var enabledSkills: [ServerSkill] { skills.filter(\.enabled) }
    /// The `/` list, the + row and the Customize group show only with a non-empty
    /// catalog (no `mcp.skills` flag reader exists in the iOS app, so the list is the gate).
    var hasSkills: Bool { !skills.isEmpty }

    func loadIfNeeded(force: Bool = false) {
        if !force, let fetchedAt, Date().timeIntervalSince(fetchedAt) < Self.ttl { return }
        guard !inflight else { return }
        inflight = true
        Task {
            defer { inflight = false }
            // Signed out, offline or no server: no group, no error.
            let list = (try? await ServerSkillsClient.shared.listServerSkills()) ?? []
            skills = list
            SkillNames.set(list)
            fetchedAt = Date()
        }
    }

    /// Flips the switch at once; a failed save puts it back and says so.
    func set(_ skill: ServerSkill, enabled: Bool) {
        guard let i = skills.firstIndex(where: { $0.id == skill.id }) else { return }
        skills[i].enabled = enabled
        saveError = nil
        Task {
            do {
                try await ServerSkillsClient.shared.setSkillEnabled(id: skill.id, enabled: enabled)
            } catch {
                if let j = skills.firstIndex(where: { $0.id == skill.id }) { skills[j].enabled = !enabled }
                saveError = "Couldn't save that switch. Try again."
            }
        }
    }

    /// The enabled skill a message starts with (`/<id>` then space or end), with
    /// the rest as its arguments. The visible text stays as typed.
    func pick(for text: String) -> (id: String, arguments: String?)? {
        guard text.hasPrefix("/") else { return nil }
        let body = text.dropFirst()
        let token = body.prefix { !$0.isWhitespace }
        guard !token.isEmpty, enabledSkills.contains(where: { $0.id == String(token) }) else { return nil }
        let rest = body.dropFirst(token.count).trimmingCharacters(in: .whitespacesAndNewlines)
        return (String(token), rest.isEmpty ? nil : rest)
    }
}
