import Foundation

/// Skills and plugins installed on the person's computer, read the way the
/// web's capability scanner does (allternit-ai src/plugins/fileSystem.ts
/// `CapabilityScanner.scanSkills` / `scanPlugins`) over allternit-api's
/// files routes (cmd/allternit-api/src/file_routes.rs):
///
/// - `GET    /api/v1/files/list?path=` → `{ path, entries: [{ name, path, type, size, modified_at }] }`
///   (a missing folder answers 200 with no entries)
/// - `GET    /api/v1/files/read?path=` → `{ content }` (404 when missing)
/// - `POST   /api/v1/files/write` `{ path, content }`
/// - `DELETE /api/v1/files/delete?path=` (removes a folder recursively)
///
/// `~` resolves to the computer's home folder. The files routes live on the
/// computer, and cloud-api has its own `/api/v1/files/:id` upload routes that
/// would shadow them, so every call goes through
/// `InstanceConnection.runtimeAPIData` (straight to the active computer's
/// relay) instead of the cloud data-plane fallback.
final class CapabilitiesClient: @unchecked Sendable {
    static let shared = CapabilitiesClient()

    enum Kind: String, Sendable {
        case skill, plugin
    }

    /// Same folders the web scans, under the computer's home.
    static let skillRoots = ["~/.allternit/skills", "~/.agents/skills", "~/.codex/skills"]
    static let pluginRoots = ["~/.allternit/plugins", "~/.agents/plugins", "~/.codex/plugins"]

    private let client: APIClient

    init(client: APIClient = .shared) {
        self.client = client
    }

    // MARK: - Files routes

    struct FileEntry: Decodable, Sendable {
        let name: String
        let path: String
        let type: String
        let size: Int?
        let modifiedAt: String?

        var isDirectory: Bool { type == "directory" }

        enum CodingKeys: String, CodingKey {
            case name, path, type, size
            case modifiedAt = "modified_at"
        }
    }

    private struct ListResponse: Decodable {
        let entries: [FileEntry]?
    }

    private struct ReadResponse: Decodable {
        let content: String?
    }

    private struct WriteBody: Encodable {
        let path: String
        let content: String
    }

    func list(_ path: String) async throws -> [FileEntry] {
        let request = try await client.authorizedRequest(path: "files/list?path=\(settingsQueryValue(path))&details=true")
        let data = try await send(request)
        do {
            return try JSONDecoder().decode(ListResponse.self, from: data).entries ?? []
        } catch {
            throw APIError.decoding(error)
        }
    }

    /// nil when the file doesn't exist.
    func read(_ path: String) async throws -> String? {
        let request = try await client.authorizedRequest(path: "files/read?path=\(settingsQueryValue(path))")
        do {
            let data = try await send(request)
            return try? JSONDecoder().decode(ReadResponse.self, from: data).content
        } catch let APIError.httpError(status, _) where status == 404 {
            return nil
        }
    }

    func write(_ path: String, content: String) async throws {
        var request = try await client.authorizedRequest(path: "files/write", method: "POST")
        request.httpBody = try JSONEncoder().encode(WriteBody(path: path, content: content))
        _ = try await send(request)
    }

    func delete(_ path: String) async throws {
        let request = try await client.authorizedRequest(path: "files/delete?path=\(settingsQueryValue(path))", method: "DELETE")
        _ = try await send(request)
    }

    private func send(_ request: URLRequest) async throws -> Data {
        let (data, response) = try await InstanceConnection.runtimeAPIData(for: request, session: client.session)
        try client.validate(response, data: data)
        return data
    }

    // MARK: - Scanning

    /// Skills: every folder holding a `SKILL.md` under the skill roots
    /// (breadth-first, a skill's own subfolders aren't searched), named from
    /// `config.json` or the SKILL.md frontmatter. First copy of a folder name wins.
    func scanSkills() async throws -> [InstalledCapability] {
        var found: [(dir: String, name: String, modified: String?, hasConfig: Bool)] = []
        var anySucceeded = false
        var lastError: Error?
        for root in Self.skillRoots {
            var queue: [(path: String, modified: String?, depth: Int)] = [(root, nil, 0)]
            while !queue.isEmpty {
                let current = queue.removeFirst()
                let entries: [FileEntry]
                do {
                    entries = try await list(current.path)
                    anySucceeded = true
                } catch {
                    if current.depth == 0 { lastError = error }
                    continue
                }
                if entries.contains(where: { !$0.isDirectory && $0.name.lowercased() == "skill.md" }) {
                    let hasConfig = entries.contains { !$0.isDirectory && $0.name == "config.json" }
                    found.append((current.path, (current.path as NSString).lastPathComponent, current.modified, hasConfig))
                    continue
                }
                guard current.depth < 4 else { continue }
                for entry in entries where entry.isDirectory && !entry.name.hasPrefix(".") && entry.name != "node_modules" {
                    queue.append((entry.path, entry.modifiedAt, current.depth + 1))
                }
            }
        }
        if !anySucceeded, let lastError { throw lastError }

        let loaded = await withTaskGroup(of: (Int, InstalledCapability).self) { group in
            for (index, item) in found.enumerated() {
                group.addTask { [self] in
                    let config = item.hasConfig ? Self.jsonObject(try? await read("\(item.dir)/config.json")) : [:]
                    let skillMD = (try? await read("\(item.dir)/SKILL.md")) ?? ""
                    let front = Self.frontmatter(skillMD)
                    return (index, InstalledCapability(
                        kind: .skill,
                        dirPath: item.dir,
                        name: Self.string(config["name"]) ?? front.name ?? item.name,
                        description: Self.string(config["description"]) ?? front.description ?? "Skill: \(item.name)",
                        author: Self.string(config["author"]) ?? "System",
                        version: Self.string(config["version"]),
                        updatedAt: Self.string(config["updatedAt"]) ?? item.modified,
                        enabled: (config["enabled"] as? Bool) ?? true,
                        configPath: "\(item.dir)/config.json"
                    ))
                }
            }
            var out: [(Int, InstalledCapability)] = []
            for await pair in group { out.append(pair) }
            return out.sorted { $0.0 < $1.0 }.map(\.1)
        }
        var seen = Set<String>()
        return loaded.filter { seen.insert($0.dirPath).inserted }
            .sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
    }

    /// Plugins: each folder in a plugin root with a manifest
    /// (`.claude-plugin/plugin.json`, `plugin.json` or `config.json`, first
    /// non-empty wins). Folders without one (e.g. `~/.codex/plugins/cache`) are skipped.
    func scanPlugins() async throws -> [InstalledCapability] {
        var dirs: [FileEntry] = []
        var anySucceeded = false
        var lastError: Error?
        for root in Self.pluginRoots {
            do {
                dirs += try await list(root).filter(\.isDirectory)
                anySucceeded = true
            } catch {
                lastError = error
            }
        }
        if !anySucceeded, let lastError { throw lastError }

        let loaded = await withTaskGroup(of: (Int, InstalledCapability?).self) { group in
            for (index, dir) in dirs.enumerated() {
                group.addTask { [self] in
                    for candidate in ["\(dir.path)/.claude-plugin/plugin.json", "\(dir.path)/plugin.json", "\(dir.path)/config.json"] {
                        let config = Self.jsonObject(try? await read(candidate))
                        guard !config.isEmpty else { continue }
                        let author: String
                        if let text = config["author"] as? String { author = text }
                        else if let object = config["author"] as? [String: Any], let name = object["name"] as? String { author = name }
                        else { author = "System" }
                        return (index, InstalledCapability(
                            kind: .plugin,
                            dirPath: dir.path,
                            name: Self.string(config["name"]) ?? dir.name,
                            description: Self.string(config["description"]) ?? "Plugin: \(dir.name)",
                            author: author,
                            version: Self.string(config["version"]),
                            updatedAt: Self.string(config["updatedAt"]) ?? dir.modifiedAt,
                            enabled: (config["enabled"] as? Bool) ?? true,
                            configPath: candidate
                        ))
                    }
                    return (index, nil)
                }
            }
            var out: [(Int, InstalledCapability?)] = []
            for await pair in group { out.append(pair) }
            return out.sorted { $0.0 < $1.0 }.compactMap(\.1)
        }
        var seen = Set<String>()
        return loaded.filter { seen.insert($0.dirPath).inserted }
            .sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
    }

    /// The web's `toggleCapabilityEnabled`: rewrite the manifest/config with
    /// `enabled` and `updatedAt`, keeping every other field.
    func setEnabled(_ capability: InstalledCapability, enabled: Bool) async throws {
        var config = Self.jsonObject(try await read(capability.configPath))
        config["enabled"] = enabled
        config["updatedAt"] = ISO8601DateFormatter().string(from: Date())
        let data = try JSONSerialization.data(withJSONObject: config, options: [.prettyPrinted, .sortedKeys])
        try await write(capability.configPath, content: String(decoding: data, as: UTF8.self))
    }

    /// The web's `deleteCapability` / `uninstallMarketplacePlugin`: removes the folder.
    func remove(_ capability: InstalledCapability) async throws {
        try await delete(capability.dirPath)
    }

    // MARK: - Parsing

    private static func jsonObject(_ text: String?) -> [String: Any] {
        guard let text, let data = text.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return [:] }
        return object
    }

    private static func string(_ value: Any?) -> String? {
        guard let text = (value as? String)?.trimmingCharacters(in: .whitespacesAndNewlines), !text.isEmpty else { return nil }
        return text
    }

    /// `name` / `description` from a SKILL.md YAML frontmatter block
    /// (single-line values), as `parseSkillFrontmatter` does.
    static func frontmatter(_ content: String) -> (name: String?, description: String?) {
        let lines = content.replacingOccurrences(of: "\r\n", with: "\n").components(separatedBy: "\n")
        guard lines.first == "---" else { return (nil, nil) }
        var name: String?
        var description: String?
        for line in lines.dropFirst() {
            if line == "---" { break }
            for key in ["name", "description"] where line.hasPrefix("\(key):") {
                var value = String(line.dropFirst(key.count + 1)).trimmingCharacters(in: .whitespaces)
                if value.count >= 2, let first = value.first, (first == "\"" || first == "'"), value.last == first {
                    value = String(value.dropFirst().dropLast())
                }
                guard !value.isEmpty, value != "|", value != ">" else { continue }
                if key == "name" { name = value } else { description = value }
            }
        }
        return (name, description)
    }
}

/// One installed skill or plugin folder on the computer.
struct InstalledCapability: Identifiable, Equatable, Sendable {
    let kind: CapabilitiesClient.Kind
    let dirPath: String
    let name: String
    let description: String
    let author: String
    let version: String?
    let updatedAt: String?
    var enabled: Bool
    /// The file `enabled` is saved in.
    let configPath: String

    var id: String { dirPath }
}
