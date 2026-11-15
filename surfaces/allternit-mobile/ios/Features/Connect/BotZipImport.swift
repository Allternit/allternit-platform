import Foundation
import Compression

// -----------------------------------------------------------------------------
// Bot .zip packages — the phone side of allternit-ai `src/lib/bots/bot-import.ts`
// (`previewBotImport` / `importBotFromZip`). A package holds a root `bot.json`
// that satisfies the web's `botPackageSchema` (lib/bots/bot-contract.ts) plus
// optional memory/, skills/, tasks/, docs/, identity/, connectors/ and secrets/
// folders and root files, which are copied into the bot's workspace on the
// computer (`agents/<id>/…`) through allternit-api's files routes.
// -----------------------------------------------------------------------------

struct BotZipError: LocalizedError, Equatable {
    let message: String
    init(_ message: String) { self.message = message }
    var errorDescription: String? { message }
}

// MARK: - Zip reader

/// A small zip reader: central directory, stored (0) and deflate (8) entries.
/// No zip64, no encryption. Folders and `__MACOSX/` are skipped.
struct ZipArchive: Sendable {
    struct Entry: Sendable, Equatable {
        let path: String
        let data: Data
    }

    static let maxTotalBytes = 50 * 1024 * 1024

    let entries: [Entry]

    /// A zip starts with a local file header (PK\3\4) or, when empty, the end record (PK\5\6).
    static func looksLikeZip(_ data: Data) -> Bool {
        guard data.count >= 4 else { return false }
        let s = data.startIndex
        return data[s] == 0x50 && data[s + 1] == 0x4B && (data[s + 2] == 0x03 || data[s + 2] == 0x05)
    }

    init(_ data: Data) throws {
        let notZip = BotZipError("This isn’t a zip file. Pick a bot package (.zip) exported from Allternit.")
        let damaged = BotZipError("This zip file is damaged and can’t be read.")
        let tooBig = BotZipError("This bot package is too big. Packages can hold at most 50 MB of files.")
        guard data.count <= Self.maxTotalBytes else { throw tooBig }
        let b = [UInt8](data)
        guard b.count >= 22 else { throw notZip }
        func u16(_ o: Int) -> Int { Int(b[o]) | Int(b[o + 1]) << 8 }
        func u32(_ o: Int) -> Int { Int(b[o]) | Int(b[o + 1]) << 8 | Int(b[o + 2]) << 16 | Int(b[o + 3]) << 24 }

        // End of central directory record: last 22 bytes plus up to a 64 KB comment.
        var eocd = -1
        var i = b.count - 22
        let lowest = max(0, b.count - 22 - 0xFFFF)
        while i >= lowest {
            if u32(i) == 0x0605_4B50 { eocd = i; break }
            i -= 1
        }
        guard eocd >= 0 else { throw notZip }
        let count = u16(eocd + 10)
        let cdSize = u32(eocd + 12)
        let cdOffset = u32(eocd + 16)
        if count == 0xFFFF || cdOffset == 0xFFFF_FFFF || cdSize == 0xFFFF_FFFF { throw tooBig }
        guard cdOffset + cdSize <= b.count else { throw damaged }

        var entries: [Entry] = []
        var total = 0
        var p = cdOffset
        for _ in 0..<count {
            guard p + 46 <= b.count, u32(p) == 0x0201_4B50 else { throw damaged }
            let flags = u16(p + 8)
            let method = u16(p + 10)
            let compSize = u32(p + 20)
            let size = u32(p + 24)
            let nameLen = u16(p + 28)
            let extraLen = u16(p + 30)
            let commentLen = u16(p + 32)
            let localOffset = u32(p + 42)
            guard p + 46 + nameLen <= b.count else { throw damaged }
            let name = String(decoding: b[(p + 46)..<(p + 46 + nameLen)], as: UTF8.self)
                .replacingOccurrences(of: "\\", with: "/")
            p += 46 + nameLen + extraLen + commentLen

            if name.hasSuffix("/") || name == "__MACOSX" || name.hasPrefix("__MACOSX/") { continue }
            if name.hasPrefix("/") || name.split(separator: "/", omittingEmptySubsequences: false).contains("..") {
                throw BotZipError("This zip has a file path that points outside the package (\(name)), so it can’t be imported.")
            }
            if flags & 0x1 != 0 { throw BotZipError("This zip is password-protected. Export the bot again without a password.") }
            total += size
            guard total <= Self.maxTotalBytes else { throw tooBig }

            guard localOffset + 30 <= b.count, u32(localOffset) == 0x0403_4B50 else { throw damaged }
            let start = localOffset + 30 + u16(localOffset + 26) + u16(localOffset + 28)
            guard start + compSize <= b.count else { throw damaged }
            let raw = b[start..<(start + compSize)]
            let content: Data
            switch method {
            case 0:
                guard compSize == size else { throw damaged }
                content = Data(raw)
            case 8:
                content = try Self.inflate(raw, expected: size, damaged: damaged)
            default:
                throw BotZipError("This zip uses a compression type the phone can’t read. Export it again from Allternit.")
            }
            entries.append(Entry(path: name, data: content))
        }
        self.entries = entries
    }

    /// Raw deflate (COMPRESSION_ZLIB is raw RFC 1951 deflate, no zlib header).
    private static func inflate(_ src: ArraySlice<UInt8>, expected: Int, damaged: BotZipError) throws -> Data {
        if expected == 0 { return Data() }
        guard !src.isEmpty else { throw damaged }
        var dst = [UInt8](repeating: 0, count: expected)
        let written = src.withUnsafeBufferPointer { s -> Int in
            dst.withUnsafeMutableBufferPointer { d -> Int in
                guard let sBase = s.baseAddress, let dBase = d.baseAddress else { return 0 }
                return compression_decode_buffer(dBase, expected, sBase, s.count, nil, COMPRESSION_ZLIB)
            }
        }
        guard written == expected else { throw damaged }
        return Data(dst)
    }
}

// MARK: - Bot package

/// A read and validated bot package.
struct BotZipPackage: Sendable, Equatable {
    /// What the web's import preview shows (BotImportPreview).
    struct Preview: Equatable, Sendable {
        var name: String
        var description: String
        var category: String?
        var hasMemory: Bool
        var hasSkills: Bool
        var hasTasks: Bool
        var hasDocs: Bool
        var hasIdentity: Bool
        var hasConnectors: Bool
        var hasSecrets: Bool
        var hasVMOperator: Bool
        var fileCount: Int
        var warnings: [String]

        /// "Memory · Skills · Docs" style list of what the package has.
        var contents: [String] {
            var out: [String] = []
            if hasMemory { out.append("Memory") }
            if hasSkills { out.append("Skills") }
            if hasTasks { out.append("Tasks") }
            if hasDocs { out.append("Docs") }
            if hasIdentity { out.append("Identity") }
            if hasConnectors { out.append("Connectors") }
            if hasSecrets { out.append("Secret slots") }
            if hasVMOperator { out.append("Computer use") }
            return out
        }
    }

    static let knownDirectories = ["memory", "skills", "tasks", "docs", "identity", "connectors", "secrets"]

    let bot: [String: JSONValue]
    /// Every file except bot.json.
    let files: [ZipArchive.Entry]
    let preview: Preview

    /// Reads a zip and checks bot.json against the web's botPackageSchema.
    static func load(_ data: Data) throws -> BotZipPackage {
        let archive = try ZipArchive(data)
        guard let botFile = archive.entries.first(where: { $0.path == "bot.json" }) else {
            throw BotZipError("This zip has no bot.json at the top level. Pick a bot package exported from Allternit.")
        }
        guard let json = try? JSONDecoder().decode(JSONValue.self, from: botFile.data), let bot = json.jObject else {
            throw BotZipError("The bot.json in this zip isn’t valid JSON.")
        }
        let problems = BotPackageValidator.validate(bot)
        if !problems.isEmpty {
            let shown = problems.prefix(4).map { "• \($0)" }.joined(separator: "\n")
            let more = problems.count > 4 ? "\n…and \(problems.count - 4) more" : ""
            throw BotZipError("This isn’t a valid bot package:\n\(shown)\(more)")
        }
        let files = archive.entries.filter { $0.path != "bot.json" }
        func hasDir(_ name: String) -> Bool { files.contains { $0.path.hasPrefix("\(name)/") } }
        func nonEmptyArray(_ key: String) -> Bool { !(bot[key]?.jArray.isEmpty ?? true) }
        var warnings: [String] = []
        if !hasDir("memory") && !hasDir("skills") && !hasDir("docs") {
            warnings.append("This package has no memory, skills or docs folders.")
        }
        let profile = bot["botProfile"]?.jObject ?? [:]
        let identity: Bool = {
            if case .object? = bot["identityChannels"] { return true }
            return false
        }()
        let preview = Preview(
            name: profile["displayName"]?.jString ?? "",
            description: bot["description"]?.jString ?? "",
            category: profile["botCategory"]?.jString,
            hasMemory: hasDir("memory"), hasSkills: hasDir("skills"), hasTasks: hasDir("tasks"), hasDocs: hasDir("docs"),
            hasIdentity: hasDir("identity") || identity,
            hasConnectors: nonEmptyArray("connectorBindings"), hasSecrets: nonEmptyArray("secretRefs"),
            hasVMOperator: bot["vmOperator"]?.at("enabled")?.jBool == true,
            fileCount: files.count, warnings: warnings)
        return BotZipPackage(bot: bot, files: files, preview: preview)
    }

    /// The files that go into the workspace, at the web's relative paths:
    /// the known folders, then root files. Other folders are left out, like the web.
    var workspaceFiles: [ZipArchive.Entry] {
        let inDirs = files.filter { f in Self.knownDirectories.contains { f.path.hasPrefix("\($0)/") && f.path.count > $0.count + 1 } }
        let root = files.filter { !$0.path.contains("/") }
        return inDirs + root
    }
}

/// The required fields and limits of allternit-ai `botPackageSchema`. Nested
/// autonomous primitives (connectors, secrets, messaging, identity, computer
/// use, harness, brain) are checked for shape only; the server keeps them as JSON.
enum BotPackageValidator {
    static let types = ["orchestrator", "sub-agent", "worker", "specialist", "reviewer", "assistant", "bot"]
    static let statuses = ["idle", "running", "paused", "error"]
    static let categories = ["research", "code", "writing", "data", "sales", "design", "ops", "custom"]

    static func validate(_ bot: [String: JSONValue]) -> [String] {
        var errors: [String] = []
        func string(_ obj: [String: JSONValue], _ key: String, path: String, required: Bool, min: Int = 0, max: Int? = nil, trim: Bool = false) {
            guard let value = obj[key], value != .null else {
                if required { errors.append("\(path): Required") }
                return
            }
            guard var s = value.jString else { errors.append("\(path): Expected text"); return }
            if trim { s = s.trimmingCharacters(in: .whitespacesAndNewlines) }
            if s.count < min { errors.append("\(path): Can’t be empty") }
            if let max, s.count > max { errors.append("\(path): At most \(max) characters") }
        }
        func number(_ key: String, required: Bool, int: Bool, range: ClosedRange<Double>) {
            guard let value = bot[key], value != .null else {
                if required { errors.append("\(key): Required") }
                return
            }
            guard case .number(let n) = value else { errors.append("\(key): Expected a number"); return }
            if int && n != n.rounded() { errors.append("\(key): Expected a whole number") }
            if !range.contains(n) { errors.append("\(key): Must be from \(format(range.lowerBound)) to \(format(range.upperBound))") }
        }
        func stringArray(_ obj: [String: JSONValue], _ key: String, path: String, required: Bool) {
            guard let value = obj[key], value != .null else {
                if required { errors.append("\(path): Required") }
                return
            }
            guard case .array(let items) = value else { errors.append("\(path): Expected a list"); return }
            if items.contains(where: { $0.jString == nil }) { errors.append("\(path): Expected a list of text") }
        }
        func oneOf(_ obj: [String: JSONValue], _ key: String, path: String, _ options: [String], required: Bool) {
            guard let value = obj[key], value != .null else {
                if required { errors.append("\(path): Required") }
                return
            }
            guard let s = value.jString, options.contains(s) else {
                errors.append("\(path): Must be one of \(options.joined(separator: ", "))")
                return
            }
        }
        func shape(_ key: String, array: Bool) {
            guard let value = bot[key], value != .null else { return }
            switch (value, array) {
            case (.array, true), (.object, false): break
            default: errors.append("\(key): Expected \(array ? "a list" : "an object")")
            }
        }

        string(bot, "id", path: "id", required: true, min: 1)
        string(bot, "name", path: "name", required: true, min: 1, max: 100)
        string(bot, "description", path: "description", required: true, max: 2000)
        oneOf(bot, "type", path: "type", types, required: true)
        string(bot, "model", path: "model", required: true, min: 1)
        string(bot, "provider", path: "provider", required: true, min: 1, trim: true)
        stringArray(bot, "capabilities", path: "capabilities", required: true)
        string(bot, "systemPrompt", path: "systemPrompt", required: false, max: 50000)
        stringArray(bot, "tools", path: "tools", required: true)
        number("maxIterations", required: true, int: true, range: 1...100)
        number("temperature", required: true, int: false, range: 0...2)
        oneOf(bot, "status", path: "status", statuses, required: true)
        string(bot, "createdAt", path: "createdAt", required: true)
        string(bot, "updatedAt", path: "updatedAt", required: true)
        string(bot, "category", path: "category", required: false)
        stringArray(bot, "tags", path: "tags", required: false)
        shape("config", array: false)
        if bot["isBot"] != .bool(true) { errors.append("isBot: Must be true") }

        if let profile = bot["botProfile"]?.jObject {
            string(profile, "displayName", path: "botProfile.displayName", required: true, min: 1, max: 120)
            string(profile, "tagline", path: "botProfile.tagline", required: false, max: 240)
            string(profile, "welcomeMessage", path: "botProfile.welcomeMessage", required: false, max: 2000)
            if let prompts = profile["starterPrompts"], prompts != .null {
                if case .array(let items) = prompts {
                    if items.count > 12 { errors.append("botProfile.starterPrompts: At most 12") }
                    if items.contains(where: { ($0.jString?.count ?? 0) < 1 || ($0.jString?.count ?? 0) > 500 }) {
                        errors.append("botProfile.starterPrompts: Each prompt must be 1 to 500 characters")
                    }
                } else {
                    errors.append("botProfile.starterPrompts: Expected a list")
                }
            }
            if let color = profile["accentColor"], color != .null {
                let s = color.jString ?? ""
                let ok = s.range(of: #"^(#[0-9A-Fa-f]{3,8}|rgb\(.*\)|hsl\(.*\))$"#, options: .regularExpression) != nil
                if !ok { errors.append("botProfile.accentColor: accentColor must be a hex, rgb, or hsl color string") }
            }
            if let group = profile["groupChatEnabled"], group != .null, group.jBool == nil {
                errors.append("botProfile.groupChatEnabled: Expected true or false")
            }
            string(profile, "defaultPresetId", path: "botProfile.defaultPresetId", required: false)
            oneOf(profile, "botCategory", path: "botProfile.botCategory", categories, required: false)
        } else {
            errors.append("botProfile: Required")
        }

        shape("harness", array: false)
        string(bot, "brainId", path: "brainId", required: false)
        shape("brain", array: false)
        shape("connectorBindings", array: true)
        shape("secretRefs", array: true)
        shape("messagingConfig", array: false)
        shape("identityChannels", array: false)
        shape("vmOperator", array: false)
        return errors
    }

    private static func format(_ n: Double) -> String {
        n == n.rounded() ? String(Int(n)) : String(n)
    }
}

// MARK: - Import

enum BotZipImporter {
    struct Result: Sendable {
        let botId: String
        let warnings: [String]
    }

    /// web importBotFromZip: a fresh id, the web's create body, then the
    /// workspace on the computer (`agents/<id>/.allternit/manifest.json`)
    /// and the package's folders and root files copied under `agents/<id>/`.
    /// Per-file failures become warnings; only the bot create failing fails.
    static func run(_ package: BotZipPackage, makeMain: Bool) async throws -> Result {
        let files = package.workspaceFiles
        // The workspace lives on the computer. Check one is reachable before
        // making the bot, so a missing computer doesn't leave a half-imported bot.
        if !files.isEmpty {
            do {
                _ = try await CapabilitiesClient.shared.list("agents")
            } catch let error as RuntimeRelayError where error == .noComputer || error == .unreachable {
                throw BotZipError("This package has files that go on your computer, and no computer is connected. Open Allternit Desktop or wake your cloud computer, then try again.")
            } catch {
                // Any other answer (an older runtime, a 404) — let the writes report it per file.
            }
        }

        let sourceId = package.bot["id"]?.jString ?? "bot"
        let freshId = "bot_\(sourceId)_\(UUID().uuidString.lowercased().prefix(8))"
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        let now = formatter.string(from: Date())
        let botId = try await MeetAgentsAPI.create(body: createBody(package.bot, id: freshId, now: now, makeMain: makeMain))

        var warnings: [String] = []
        let root = "agents/\(botId)"
        for file in files {
            let target = "\(root)/\(file.path)"
            guard let text = String(data: file.data, encoding: .utf8) else {
                warnings.append("Skipped \(file.path): it isn’t a text file.")
                continue
            }
            do {
                try await CapabilitiesClient.shared.write(target, content: text)
            } catch {
                warnings.append("Couldn’t copy \(file.path): \(FriendlyErrorMessage.from(error.localizedDescription))")
            }
        }

        // Workspace manifest (agent-workspace.service createWorkspace / writeWorkspaceFile).
        if !files.isEmpty {
            let millis = JSONValue.number((Date().timeIntervalSince1970 * 1000).rounded())
            let name = package.bot["botProfile"]?.at("displayName")?.jString ?? sourceId
            let manifest: JSONValue = .object([
                "id": .string(botId), "agentId": .string(botId), "agentName": .string(name),
                "template": .string("allternit-standard"), "version": .string("1.0.0"),
                "createdAt": millis, "lastModified": millis,
                "files": .array(files.map { .string($0.path) }),
            ])
            let encoder = JSONEncoder()
            encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
            if let data = try? encoder.encode(manifest), let text = String(data: data, encoding: .utf8) {
                do {
                    try await CapabilitiesClient.shared.write("\(root)/.allternit/manifest.json", content: text)
                } catch {
                    warnings.append("Couldn’t write the workspace manifest: \(FriendlyErrorMessage.from(error.localizedDescription))")
                }
            }
        }
        return Result(botId: botId, warnings: warnings)
    }

    /// bot-import buildCreateInput → agent.service createAgent's REST body.
    static func createBody(_ bot: [String: JSONValue], id: String, now: String, makeMain: Bool) -> [String: JSONValue] {
        var profile = bot["botProfile"]?.jObject ?? [:]
        if makeMain { profile["chiefOfStaff"] = .bool(true) }
        let displayName = profile["displayName"]?.jString ?? bot["name"]?.jString ?? "Imported bot"

        var config = bot["config"]?.jObject ?? [:]
        config["importedAt"] = .string(now)
        config["importedFrom"] = .string("zip")
        config["agentSource"] = .string("organization")
        config["isBot"] = .bool(true)
        config["botProfile"] = .object(profile)
        let primitives: [(camel: String, snake: String)] = [
            ("connectorBindings", "connector_bindings"), ("secretRefs", "secret_refs"),
            ("messagingConfig", "messaging_config"), ("identityChannels", "identity_channels"),
            ("vmOperator", "vm_operator"),
        ]
        for p in primitives { if let v = bot[p.camel], v != .null { config[p.camel] = v } }

        var body: [String: JSONValue] = [
            "id": .string(id),
            "name": .string(displayName),
            "description": bot["description"] ?? .string(""),
            "type": bot["type"] ?? .string("worker"),
            "model": bot["model"] ?? .string(""),
            "provider": bot["provider"] ?? .string(""),
            "capabilities": bot["capabilities"] ?? .array([]),
            "tools": bot["tools"] ?? .array([]),
            "max_iterations": bot["maxIterations"] ?? .number(10),
            "temperature": bot["temperature"] ?? .number(0.7),
            "config": .object(config),
            "trust_tier": .string("standard"),
            "enabled_modes": .array(["chat", "cowork", "code", "design", "browser"].map { .string($0) }),
            "category": bot["category"] ?? .string("general"),
            "tags": bot["tags"] ?? .array([]),
            "is_bot": .bool(true),
            "bot_profile": .object(profile),
        ]
        if let prompt = bot["systemPrompt"], prompt != .null { body["system_prompt"] = prompt }
        for p in primitives { if let v = bot[p.camel], v != .null { body[p.snake] = v } }
        return body
    }
}
