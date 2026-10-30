import Foundation

/// The MCP servers on the person's computer, the way the web's Settings ›
/// MCP panel finds and changes them (allternit-ai
/// src/views/settings/McpSettingsPanel.tsx over src/plugins/fileSystem.ts
/// `scanMcps` and src/plugins/capabilityWriter.ts `createMcp` /
/// `updateCapabilityMetadata` / `toggleCapabilityEnabled` / `deleteCapability`):
///
/// - Server files: one JSON file per server in `~/.allternit/mcps`,
///   `~/.agents/mcps`, `~/.codex/mcps`. These can be added, edited, turned
///   on/off and removed.
/// - Runtime config files (`~/.config/gizzi/allternit.json`,
///   `~/.claude/settings.json`, `~/.codex/config.toml`, …): listed read-only,
///   since rewriting another tool's settings file from a phone is unsafe.
///
/// Files go over allternit-api's files routes (`CapabilitiesClient`). Live
/// status and each server's tools come from gizzi-code's MCP routes
/// (cmd/gizzi-code/src/runtime/server/routes/mcp.ts, mounted at `v1/mcp`):
///
/// - `GET  v1/mcp`                     → `{ [name]: { status, error? } }`
/// - `GET  v1/mcp/tools/catalog`       → `{ descriptors: { [qualified]: { serverName, originalName, title? } } }`
/// - `POST v1/mcp/:name/connect` / `POST v1/mcp/:name/disconnect`
final class McpServersClient: @unchecked Sendable {
    static let shared = McpServersClient()

    static let serverFolders = ["~/.allternit/mcps", "~/.agents/mcps", "~/.codex/mcps"]
    /// New servers are written here (capabilityWriter `MCPS_DIR`).
    static let newServerFolder = "~/.allternit/mcps"
    static let configFiles = [
        "~/.config/allternit-shell/allternit.json",
        "~/.config/gizzi/allternit.json",
        "~/.config/gizzi-code/allternit.json",
        "~/.claude/settings.json",
        "~/.codex/config.toml",
        "~/.gemini/mcp.json",
    ]

    private let files: CapabilitiesClient
    private let gizziBaseURL: URL

    init(files: CapabilitiesClient = .shared, gizziBaseURL: URL = AppConfig.gizziCodeBaseURL) {
        self.files = files
        self.gizziBaseURL = gizziBaseURL
    }

    // MARK: - Scanning

    /// Every server, first copy of a name wins (fileSystem.ts `getMcpDedupKey`).
    /// Throws only when no folder or config file could be read at all.
    func scan() async throws -> [McpServer] {
        var servers: [McpServer] = []
        var seen = Set<String>()
        var anySucceeded = false
        var lastError: Error?

        func add(_ server: McpServer) {
            let key = server.name.trimmingCharacters(in: .whitespaces).lowercased()
            guard seen.insert(key).inserted else { return }
            servers.append(server)
        }

        for folder in Self.serverFolders {
            let entries: [CapabilitiesClient.FileEntry]
            do {
                entries = try await files.list(folder)
                anySucceeded = true
            } catch {
                lastError = error
                continue
            }
            for entry in entries where !entry.isDirectory && entry.name.hasSuffix(".json") {
                guard let text = try? await files.read(entry.path) else { continue }
                add(Self.serverFromFile(path: entry.path, fileName: entry.name, text: text, modified: entry.modifiedAt))
            }
        }

        for configPath in Self.configFiles {
            let text: String?
            do {
                text = try await files.read(configPath)
                anySucceeded = true
            } catch {
                lastError = error
                continue
            }
            guard let text else { continue }
            let found = configPath.hasSuffix(".toml")
                ? Self.serversFromToml(text, source: configPath)
                : Self.serversFromJSONConfig(text, source: configPath)
            found.forEach(add)
        }

        if !anySucceeded, let lastError { throw lastError }
        return servers.sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
    }

    // MARK: - Changes (server files only)

    struct Draft: Equatable, Sendable {
        var name = ""
        var description = ""
        var command = ""
        /// One argument per line.
        var argsText = ""
        /// Comma separated.
        var tagsText = ""

        var args: [String] {
            argsText.components(separatedBy: .newlines)
                .map { $0.trimmingCharacters(in: .whitespaces) }
                .filter { !$0.isEmpty }
        }

        var tags: [String] {
            tagsText.split(separator: ",")
                .map { $0.trimmingCharacters(in: .whitespaces) }
                .filter { !$0.isEmpty }
        }

        var isValid: Bool {
            !name.trimmingCharacters(in: .whitespaces).isEmpty
                && !command.trimmingCharacters(in: .whitespaces).isEmpty
        }

        init() {}

        init(_ server: McpServer) {
            name = server.name
            description = server.userDescription ?? ""
            command = server.command ?? ""
            argsText = server.args.joined(separator: "\n")
            tagsText = server.tags.joined(separator: ", ")
        }
    }

    /// capabilityWriter `createMcp`: `~/.allternit/mcps/<slug>.json`.
    /// Refuses to overwrite a server file that's already there.
    func create(_ draft: Draft) async throws -> String {
        let path = "\(Self.newServerFolder)/\(Self.slugify(draft.name)).json"
        if try await files.read(path) != nil {
            throw APIError.httpError(statusCode: 409, message: "A server file named \(Self.slugify(draft.name)).json already exists. Pick another name.")
        }
        let now = ISO8601DateFormatter().string(from: Date())
        let config: [String: Any] = [
            "name": draft.name.trimmingCharacters(in: .whitespaces),
            "description": draft.description.trimmingCharacters(in: .whitespacesAndNewlines),
            "command": draft.command.trimmingCharacters(in: .whitespaces),
            "args": draft.args,
            "tags": draft.tags,
            "enabled": true,
            "version": "1.0.0",
            "author": "User",
            "createdAt": now,
            "updatedAt": now,
        ]
        try await files.write(path, content: Self.jsonText(config))
        return path
    }

    /// `updateCapabilityMetadata`: rewrite the server's own file, keeping
    /// every field this form doesn't edit.
    func update(_ server: McpServer, with draft: Draft) async throws {
        guard server.isEditable else { throw Self.readOnlyError(server) }
        var config = Self.jsonObject(try await files.read(server.sourcePath))
        config["name"] = draft.name.trimmingCharacters(in: .whitespaces)
        config["description"] = draft.description.trimmingCharacters(in: .whitespacesAndNewlines)
        config["command"] = draft.command.trimmingCharacters(in: .whitespaces)
        config["args"] = draft.args
        config["tags"] = draft.tags
        config["updatedAt"] = ISO8601DateFormatter().string(from: Date())
        try await files.write(server.sourcePath, content: Self.jsonText(config))
    }

    /// `toggleCapabilityEnabled`: `enabled` + `updatedAt`, other fields kept.
    func setEnabled(_ server: McpServer, enabled: Bool) async throws {
        guard server.isEditable else { throw Self.readOnlyError(server) }
        var config = Self.jsonObject(try await files.read(server.sourcePath))
        config["enabled"] = enabled
        config["updatedAt"] = ISO8601DateFormatter().string(from: Date())
        try await files.write(server.sourcePath, content: Self.jsonText(config))
    }

    /// `deleteCapability`: removes the server's file.
    func remove(_ server: McpServer) async throws {
        guard server.isEditable else { throw Self.readOnlyError(server) }
        try await files.delete(server.sourcePath)
    }

    private static func readOnlyError(_ server: McpServer) -> Error {
        APIError.httpError(statusCode: 400, message: "\(server.name) is set in \(server.sourcePath). Change it there.")
    }

    // MARK: - Live state (gizzi-code)

    private func gizzi() async throws -> APIClient {
        try await InstanceConnection.gizziClient(for: gizziBaseURL)
    }

    /// The runtime's view of every server it has configured.
    func liveStatus() async throws -> [String: McpLiveStatus] {
        try await gizzi().get(path: "v1/mcp")
    }

    private struct CatalogBody: Decodable {
        let descriptors: [String: McpToolDescriptor]?
    }

    /// Tools by server name (lowercased).
    func tools() async throws -> [String: [McpToolDescriptor]] {
        let body: CatalogBody = try await gizzi().get(path: "v1/mcp/tools/catalog")
        var out: [String: [McpToolDescriptor]] = [:]
        for (qualified, descriptor) in body.descriptors ?? [:] {
            var tool = descriptor
            tool.qualifiedName = qualified
            out[descriptor.serverName.lowercased(), default: []].append(tool)
        }
        for key in out.keys {
            out[key]?.sort { $0.displayName.localizedCaseInsensitiveCompare($1.displayName) == .orderedAscending }
        }
        return out
    }

    func connect(_ runtimeName: String) async throws {
        try await gizzi().post(path: "v1/mcp/\(settingsPathSegment(runtimeName))/connect")
    }

    func disconnect(_ runtimeName: String) async throws {
        try await gizzi().post(path: "v1/mcp/\(settingsPathSegment(runtimeName))/disconnect")
    }

    // MARK: - Parsing

    static func serverFromFile(path: String, fileName: String, text: String, modified: String?) -> McpServer {
        let config = jsonObject(text)
        let base = fileName.hasSuffix(".json") ? String(fileName.dropLast(5)) : fileName
        let (command, args) = commandParts(config)
        let userDescription = string(config["description"])
        return McpServer(
            sourcePath: path,
            key: base,
            name: string(config["name"]) ?? base,
            userDescription: userDescription,
            command: command,
            args: args,
            url: string(config["url"]),
            tags: (config["tags"] as? [Any])?.compactMap { $0 as? String } ?? [],
            enabled: (config["enabled"] as? Bool) ?? true,
            isEditable: true,
            version: string(config["version"]),
            updatedAt: string(config["updatedAt"]) ?? modified
        )
    }

    /// `mcp` / `mcpServers` / `mcp_servers` blocks of a JSON config file.
    static func serversFromJSONConfig(_ text: String, source: String) -> [McpServer] {
        let config = jsonObject(text)
        var out: [McpServer] = []
        for containerKey in ["mcp", "mcpServers", "mcp_servers"] {
            guard let container = config[containerKey] as? [String: Any] else { continue }
            for (key, value) in container.sorted(by: { $0.key < $1.key }) {
                guard let record = value as? [String: Any] else { continue }
                let (command, args) = commandParts(record)
                out.append(McpServer(
                    sourcePath: source,
                    key: key,
                    name: displayLabel(key),
                    userDescription: string(record["description"]),
                    command: command,
                    args: args,
                    url: string(record["url"]),
                    tags: [],
                    enabled: (record["enabled"] as? Bool) ?? true,
                    isEditable: false,
                    version: string(record["version"]),
                    updatedAt: nil
                ))
            }
        }
        return out
    }

    /// `[mcp_servers.<name>]` tables of a TOML file (fileSystem.ts
    /// `parseTomlMcpSections`: single-line strings and string arrays).
    static func serversFromToml(_ text: String, source: String) -> [McpServer] {
        var out: [McpServer] = []
        var current: (key: String, values: [String: Any])?

        func flush() {
            guard let current else { return }
            let command = current.values["command"] as? String
            out.append(McpServer(
                sourcePath: source,
                key: current.key,
                name: displayLabel(current.key),
                userDescription: nil,
                command: command?.isEmpty == false ? command : nil,
                args: current.values["args"] as? [String] ?? [],
                url: current.values["url"] as? String,
                tags: [],
                enabled: true,
                isEditable: false,
                version: nil,
                updatedAt: nil
            ))
        }

        for rawLine in text.replacingOccurrences(of: "\r\n", with: "\n").components(separatedBy: "\n") {
            let line = rawLine.trimmingCharacters(in: .whitespaces)
            if line.hasPrefix("["), line.hasSuffix("]"), !line.hasPrefix("[[") {
                flush()
                let section = String(line.dropFirst().dropLast())
                if section.hasPrefix("mcp_servers.") {
                    let key = String(section.dropFirst("mcp_servers.".count))
                    if key.isEmpty || key.contains(".") {
                        current = nil
                    } else {
                        current = (key: key, values: [String: Any]())
                    }
                } else {
                    current = nil
                }
                continue
            }
            guard current != nil, !line.isEmpty, !line.hasPrefix("#"),
                  let eq = line.firstIndex(of: "=") else { continue }
            let key = line[..<eq].trimmingCharacters(in: .whitespaces)
            let value = line[line.index(after: eq)...].trimmingCharacters(in: .whitespaces)
            if let parsed = tomlValue(value) { current?.values[key] = parsed }
        }
        flush()
        return out
    }

    private static func tomlValue(_ raw: String) -> Any? {
        func unquote(_ s: String) -> String? {
            let t = s.trimmingCharacters(in: .whitespaces)
            guard t.count >= 2, let first = t.first, first == "\"" || first == "'", t.last == first else { return nil }
            return String(t.dropFirst().dropLast())
        }
        if let single = unquote(raw) { return single }
        if raw.hasPrefix("["), raw.hasSuffix("]") {
            return raw.dropFirst().dropLast().split(separator: ",").compactMap { unquote(String($0)) }
        }
        return nil
    }

    /// `command` may be a string (with `args`) or an array (command first).
    private static func commandParts(_ record: [String: Any]) -> (String?, [String]) {
        if let parts = record["command"] as? [Any] {
            let strings = parts.compactMap { $0 as? String }
            return (strings.first, Array(strings.dropFirst()))
        }
        let args = (record["args"] as? [Any])?.compactMap { $0 as? String } ?? []
        return (string(record["command"]), args)
    }

    static func displayLabel(_ value: String) -> String {
        let spaced = value.replacingOccurrences(of: "[_-]+", with: " ", options: .regularExpression)
            .trimmingCharacters(in: .whitespaces)
        guard !spaced.isEmpty else { return "Unnamed" }
        return spaced.split(separator: " ").map { word in
            word.prefix(1).uppercased() + word.dropFirst()
        }.joined(separator: " ")
    }

    /// capabilityWriter `slugify`.
    static func slugify(_ value: String) -> String {
        let slug = value.lowercased()
            .replacingOccurrences(of: "[^a-z0-9]+", with: "-", options: .regularExpression)
            .trimmingCharacters(in: CharacterSet(charactersIn: "-"))
        return slug.isEmpty ? "item" : String(slug.prefix(48))
    }

    private static func jsonObject(_ text: String?) -> [String: Any] {
        guard let text, let data = text.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return [:] }
        return object
    }

    private static func jsonText(_ object: [String: Any]) throws -> String {
        let data = try JSONSerialization.data(withJSONObject: object, options: [.prettyPrinted, .sortedKeys])
        return String(decoding: data, as: UTF8.self)
    }

    private static func string(_ value: Any?) -> String? {
        guard let text = (value as? String)?.trimmingCharacters(in: .whitespacesAndNewlines), !text.isEmpty else { return nil }
        return text
    }
}

/// One MCP server found on the computer.
struct McpServer: Identifiable, Equatable, Sendable {
    /// The server's own file, or the config file that lists it.
    let sourcePath: String
    /// File name without `.json`, or the config key: the runtime's name for it.
    let key: String
    let name: String
    let userDescription: String?
    let command: String?
    let args: [String]
    let url: String?
    let tags: [String]
    var enabled: Bool
    /// Only servers with their own file in an mcps folder can be changed.
    let isEditable: Bool
    let version: String?
    let updatedAt: String?

    var id: String { "\(sourcePath)#\(key)" }

    /// fileSystem.ts' fallback description: the URL or command it runs.
    var summary: String {
        if let userDescription { return userDescription }
        if let url { return url }
        if let command { return ([command] + args).joined(separator: " ") }
        return "MCP server \(key)"
    }

    /// Names the runtime might know this server by.
    var runtimeNames: [String] {
        var names = [key, name, McpServersClient.slugify(name)]
        names = names.map { $0.lowercased() }
        var seen = Set<String>()
        return names.filter { seen.insert($0).inserted }
    }
}

/// gizzi-code `MCP.Status` (runtime/tools/mcp/index.ts).
struct McpLiveStatus: Decodable, Equatable, Sendable {
    let status: String
    let error: String?

    var label: String {
        switch status {
        case "connected": return "Connected"
        case "disabled": return "Not connected"
        case "failed": return "Failed"
        case "needs_auth": return "Needs sign-in"
        case "needs_client_registration": return "Needs setup"
        default: return status.capitalized
        }
    }

    var isConnected: Bool { status == "connected" }
    var isProblem: Bool { status == "failed" || status == "needs_auth" || status == "needs_client_registration" }
}

/// gizzi-code `MCP.ToolDescriptor` (the fields the phone shows).
struct McpToolDescriptor: Decodable, Equatable, Sendable, Identifiable {
    let serverName: String
    let originalName: String
    let title: String?
    let requiresConfirmation: Bool?
    var qualifiedName: String = ""

    var id: String { qualifiedName.isEmpty ? "\(serverName)/\(originalName)" : qualifiedName }
    var displayName: String { title?.isEmpty == false ? title! : originalName }

    enum CodingKeys: String, CodingKey {
        case serverName, originalName, title, requiresConfirmation
    }
}
