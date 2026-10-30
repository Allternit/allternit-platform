import Foundation

// MARK: - Code session updates (allternit-api)

/// `PATCH /api/v1/agent-sessions/:id` (agent_session_routes.rs
/// `update_session`): `name` renames, `active: false` archives (gizzi
/// `archived`), and `metadata.permission` applies a Code permission mode's
/// gizzi rule set — the same three writes the web's mode-session-store makes.
struct CodeSessionsClient: Sendable {
    private let client: APIClient

    init(client: APIClient = .shared) {
        self.client = client
    }

    func rename(sessionId: String, name: String) async throws {
        try await client.patch(path: "agent-sessions/\(Self.escape(sessionId))", body: UpdateBody(name: name))
    }

    /// Closing a card in the overview archives the session (useModeOverview
    /// `archiveOverviewItem`); it leaves the open list.
    func archive(sessionId: String) async throws {
        try await client.patch(path: "agent-sessions/\(Self.escape(sessionId))", body: UpdateBody(active: false))
    }

    func applyPermission(sessionId: String, mode: CodePermissionMode) async throws {
        try await client.patch(
            path: "agent-sessions/\(Self.escape(sessionId))",
            body: UpdateBody(metadata: .init(permission: mode.rules))
        )
    }

    private struct UpdateBody: Encodable {
        var name: String? = nil
        var active: Bool? = nil
        var metadata: Metadata? = nil

        struct Metadata: Encodable {
            let permission: [CodePermissionRule]
        }
    }

    private static func escape(_ id: String) -> String {
        id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id
    }
}

/// One gizzi permission rule (`{ permission, pattern, action }`).
struct CodePermissionRule: Encodable, Sendable {
    let permission: String
    let pattern: String
    let action: String
}

/// The permission modes under the Code prompt (CodeBottomStatusBar.tsx
/// `CODE_SESSION_MODE_META`), each mapped to the gizzi rule set the web
/// sends (mode-session-store.ts `codePermissionRules`).
enum CodePermissionMode: String, CaseIterable, Identifiable, Sendable {
    case manual = "DEFAULT"
    case auto = "AUTO"
    case plan = "PLAN"
    case safe = "SAFE"

    var id: String { rawValue }

    var label: String {
        switch self {
        case .manual: return "Manual"
        case .auto: return "Auto-accept edits"
        case .plan: return "Plan mode"
        case .safe: return "Read only"
        }
    }

    var description: String {
        switch self {
        case .manual: return "Ask before running commands and edits"
        case .auto: return "Run commands and apply file edits without asking"
        case .plan: return "Explore the codebase and propose a plan before changes"
        case .safe: return "Inspect files without changing anything"
        }
    }

    /// SF Symbols for the web's HandPointing / HandPalm / Scroll / ShieldCheck.
    var icon: String {
        switch self {
        case .manual: return "hand.point.up.left"
        case .auto: return "hand.raised"
        case .plan: return "scroll"
        case .safe: return "checkmark.shield"
        }
    }

    var rules: [CodePermissionRule] {
        let read = ["read", "glob", "grep", "list", "websearch", "codesearch", "question", "todoread", "lsp"]
        let allow = { (permission: String) in CodePermissionRule(permission: permission, pattern: "*", action: "allow") }
        switch self {
        case .manual:
            return []
        case .auto:
            return read.map(allow) + ["edit", "write", "patch", "multiedit"].map(allow)
        case .plan, .safe:
            return [CodePermissionRule(permission: "*", pattern: "*", action: "deny")] + read.map(allow)
        }
    }

    static let storageKey = "allternit.code.permission-mode"
}

// MARK: - Working folder (gizzi-code)

/// The working folder and branch of the computer's gizzi-code instance:
/// `GET v1/path` (`directory`) and `GET v1/vcs` (`branch`) — what the web's
/// top deck shows as the folder chip and branch.
struct CodeFolderInfo: Equatable, Sendable {
    let directory: String
    let branch: String?

    var name: String {
        let trimmed = directory.hasSuffix("/") ? String(directory.dropLast()) : directory
        return trimmed.split(separator: "/").last.map(String.init) ?? trimmed
    }
}

enum CodeFolderClient {
    private struct PathResponse: Decodable { let directory: String }
    private struct VcsResponse: Decodable { let branch: String? }

    static func load(baseURL: URL) async throws -> CodeFolderInfo {
        let client = try await InstanceConnection.gizziClient(for: baseURL)
        let path: PathResponse = try await client.get(path: "v1/path")
        let vcs: VcsResponse? = try? await client.get(path: "v1/vcs")
        return CodeFolderInfo(directory: path.directory, branch: vcs?.branch)
    }
}

// MARK: - Terminal bots (Factory)

/// A Terminal bot from `GET /api/factory/agents` (the fields the phone
/// terminal's bot strip reads — lib/factory/types.ts `Agent`).
struct TerminalBot: Decodable, Identifiable, Sendable, Equatable {
    struct Binding: Decodable, Sendable, Equatable { let type: String }
    struct Pane: Decodable, Sendable, Equatable { let id: String; let attachable: Bool }
    struct Machine: Decodable, Sendable, Equatable { let id: String; let name: String? }

    let id: String
    let slug: String
    let name: String
    let address: String
    let state: String
    let binding: Binding
    let pane: Pane?
    let machine: Machine?

    /// `isTerminalBot` (useTerminalBots.ts): a terminal binding with a pane.
    var isTerminalBot: Bool { binding.type == "terminal" && pane != nil }

    var initial: String { String((name.isEmpty ? slug : name).trimmingCharacters(in: .whitespaces).first ?? "?").uppercased() }

    /// AGENT_STATE labels (components/factory/bot/labels.ts).
    var stateLabel: String {
        switch state {
        case "idle": return "Idle"
        case "working": return "Working"
        case "blocked": return "Blocked"
        case "needs_you": return "Needs you"
        case "done": return "Done"
        case "failed": return "Failed"
        case "offline": return "Offline"
        default: return state
        }
    }

    /// Where the bot runs when it isn't this computer (BotPaneParts `remoteMachine`).
    var remoteMachine: String? {
        guard let machine, machine.id != "local" else { return nil }
        return machine.name ?? machine.id
    }
}

/// A Factory error in the contract's words (`{ code, fact, action }`), or the
/// engine-missing case when nothing serves `/api/factory` at all.
struct FactoryError: Error, Equatable, Sendable {
    let fact: String
    let action: String?
    let engineMissing: Bool

    static let engineMissingText = "The Factory engine isn't running on this computer"
}

struct TerminalBotsResult: Sendable {
    let bots: [TerminalBot]
    /// `engine.running == false`: the pane engine can't be read.
    let engineError: String?
    let engineRunning: Bool
}

/// An agent tree (lib/factory/types.ts `AgentTree`): a leased git worktree.
struct FactoryTree: Decodable, Sendable, Identifiable, Equatable {
    struct Owner: Decodable, Sendable, Equatable { let kind: String; let id: String; let label: String }
    struct Process: Decodable, Sendable, Equatable { let pid: Int; let command: String; let devServer: Bool }

    let id: String
    let repo: String
    let path: String
    let branch: String?
    let owner: Owner?
    /// active | paused | done
    let state: String
    let behindMain: Int?
    let aheadMain: Int?
    let dirtyFiles: Int?
    let sizeBytes: Double?
    let processes: [Process]?
    let checkpointCount: Int?
}

struct FactoryTreeCheckpoint: Decodable, Sendable, Identifiable, Equatable {
    let n: Int
    let sha: String
    let at: String
    let tool: String?
    let files: Int
    let summary: String

    var id: Int { n }
}

struct FactoryTreesList: Decodable, Sendable {
    struct Machine: Decodable, Sendable { let freeBytes: Double?; let warnings: [String]? }
    struct Overlap: Decodable, Sendable { let a: String; let b: String; let files: [String] }

    let trees: [FactoryTree]
    let machine: Machine?
    let overlaps: [Overlap]?
}

/// The Factory routes the phone terminal uses (lib/factory/panes.ts):
/// the bot list, a bot's pane screen (SSE `screen` events), and keystrokes.
struct FactoryPaneClient: Sendable {
    private let client: APIClient
    private let baseURL: URL

    init(client: APIClient = .shared, baseURL: URL = AppConfig.factoryBaseURL) {
        self.client = client
        self.baseURL = baseURL
    }

    private struct AgentsResponse: Decodable {
        struct Engine: Decodable { let running: Bool; let error: String? }
        let agents: [TerminalBot]?
        let engine: Engine?
    }

    private struct ErrorBody: Decodable {
        let fact: String?
        let action: String?
    }

    func terminalBots() async throws -> TerminalBotsResult {
        let data = try await send(path: "agents")
        let body = try JSONDecoder().decode(AgentsResponse.self, from: data)
        return TerminalBotsResult(
            bots: (body.agents ?? []).filter(\.isTerminalBot),
            engineError: body.engine?.error,
            engineRunning: body.engine?.running ?? true
        )
    }

    /// `POST /agents/:id/input` `{ text?, keys? }` — keystrokes, not a send.
    func sendInput(agentId: String, text: String? = nil, keys: [String]? = nil) async throws {
        struct Input: Encodable { let text: String?; let keys: [String]? }
        _ = try await send(path: "agents/\(Self.escape(agentId))/input", method: "POST", body: Input(text: text, keys: keys))
    }

    /// `GET /agents/:id/stream` (SSE): `screen` events carrying the visible
    /// screen as ANSI, then one `gone` event `{ reason }`.
    func screenStream(agentId: String) -> AsyncThrowingStream<PaneStreamEvent, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    var request = try await client.authorizedRequest(url: baseURL.appendingPathComponent("agents/\(agentId)/stream"))
                    request.setValue("text/event-stream", forHTTPHeaderField: "Accept")
                    request.timeoutInterval = 3600
                    let (bytes, response) = try await InstanceConnection.runtimeAPIBytes(for: request, session: client.session)
                    try client.validate(response)
                    var event = "message"
                    var dataLines: [String] = []
                    for try await line in bytes.lines {
                        if line.isEmpty {
                            Self.dispatch(event: event, data: dataLines.joined(separator: "\n"), into: continuation)
                            event = "message"
                            dataLines = []
                        } else if line.hasPrefix("event:") {
                            // `bytes.lines` drops the blank separator line, so a
                            // new event name also closes the previous event.
                            if !dataLines.isEmpty {
                                Self.dispatch(event: event, data: dataLines.joined(separator: "\n"), into: continuation)
                                dataLines = []
                            }
                            event = line.dropFirst(6).trimmingCharacters(in: .whitespaces)
                        } else if line.hasPrefix("data:") {
                            dataLines.append(String(line.dropFirst(5)).trimmingCharacters(in: .whitespaces))
                            // Each pane event is one JSON line: deliver it now.
                            Self.dispatch(event: event, data: dataLines.joined(separator: "\n"), into: continuation)
                            dataLines = []
                        }
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: Self.factoryError(error))
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    enum PaneStreamEvent: Sendable {
        case screen(String)
        case gone(String?)
    }

    private static func dispatch(event: String, data: String, into continuation: AsyncThrowingStream<PaneStreamEvent, Error>.Continuation) {
        guard let json = data.data(using: .utf8) else { return }
        if event == "gone" {
            struct Gone: Decodable { let reason: String? }
            continuation.yield(.gone((try? JSONDecoder().decode(Gone.self, from: json))?.reason))
        } else {
            struct Screen: Decodable { let ansi: String }
            if let screen = try? JSONDecoder().decode(Screen.self, from: json) {
                continuation.yield(.screen(screen.ansi))
            }
        }
    }

    /// `GET /trees?path=` — the agent trees (leased worktrees) under a folder,
    /// with the machine's disk warnings and trees that changed the same files.
    func trees(path: String?) async throws -> FactoryTreesList {
        var url = baseURL.appendingPathComponent("trees")
        if let path, var components = URLComponents(url: url, resolvingAgainstBaseURL: false) {
            components.queryItems = [URLQueryItem(name: "path", value: path)]
            url = components.url ?? url
        }
        let data = try await perform(try await client.authorizedRequest(url: url))
        return try JSONDecoder().decode(FactoryTreesList.self, from: data)
    }

    /// `GET /trees/:id/checkpoints`, newest first.
    func treeCheckpoints(id: String) async throws -> [FactoryTreeCheckpoint] {
        struct Body: Decodable { let checkpoints: [FactoryTreeCheckpoint] }
        let data = try await send(path: "trees/\(Self.escape(id))/checkpoints")
        return try JSONDecoder().decode(Body.self, from: data).checkpoints
    }

    private func send<B: Encodable>(path: String, method: String = "GET", body: B) async throws -> Data {
        var request = try await client.authorizedRequest(url: baseURL.appendingPathComponent(path), method: method)
        request.httpBody = try JSONEncoder().encode(body)
        return try await perform(request)
    }

    private func send(path: String) async throws -> Data {
        try await perform(try await client.authorizedRequest(url: baseURL.appendingPathComponent(path)))
    }

    private func perform(_ request: URLRequest) async throws -> Data {
        do {
            let (data, response) = try await InstanceConnection.runtimeAPIData(for: request, session: client.session)
            if let http = response as? HTTPURLResponse, !(200..<300).contains(http.statusCode) {
                let body = try? JSONDecoder().decode(ErrorBody.self, from: data)
                // 502/503/404 with no contract body: nothing serves the Factory.
                let missing = body?.fact == nil && [404, 502, 503, 504].contains(http.statusCode)
                throw FactoryError(
                    fact: body?.fact ?? "Reading the Terminal bots failed.",
                    action: body?.action,
                    engineMissing: missing
                )
            }
            return data
        } catch {
            throw Self.factoryError(error)
        }
    }

    private static func factoryError(_ error: Error) -> Error {
        if error is FactoryError || error is CancellationError { return error }
        if let urlError = error as? URLError, urlError.code == .cancelled { return CancellationError() }
        return FactoryError(fact: error.localizedDescription, action: "Try again.", engineMissing: true)
    }

    private static func escape(_ id: String) -> String {
        id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id
    }

    /// Terminal input bytes → pane input batches, in order (panes.ts
    /// `terminalDataToPaneInput`): printable runs become `text`, control
    /// bytes and escape sequences become named `keys`.
    static func paneInput(from data: String) -> [(text: String?, keys: [String]?)] {
        let named: [String: String] = [
            "\r": "enter", "\n": "enter", "\t": "tab", "\u{7f}": "backspace", "\u{8}": "backspace",
            "\u{1b}": "esc", "\u{1b}[A": "up", "\u{1b}[B": "down", "\u{1b}[C": "right", "\u{1b}[D": "left",
            "\u{1b}OA": "up", "\u{1b}OB": "down", "\u{1b}OC": "right", "\u{1b}OD": "left", "\u{1b}[Z": "shift+tab",
        ]
        var out: [(text: String?, keys: [String]?)] = []
        var text = ""
        func flush() {
            if !text.isEmpty { out.append((text, nil)) }
            text = ""
        }
        func push(_ key: String) {
            flush()
            if let last = out.last, last.text == nil, let keys = last.keys {
                out[out.count - 1] = (nil, keys + [key])
            } else {
                out.append((nil, [key]))
            }
        }
        let scalars = Array(data.unicodeScalars)
        var i = 0
        while i < scalars.count {
            let ch = scalars[i]
            if ch == "\u{1b}" {
                if i + 2 < scalars.count {
                    let three = String(String.UnicodeScalarView(scalars[i...(i + 2)]))
                    if let key = named[three] {
                        push(key)
                        i += 3
                        continue
                    }
                }
                // CSI with parameters (ESC [ 1 ; 5 A): skip the whole sequence.
                if i + 1 < scalars.count, scalars[i + 1] == "[" {
                    var j = i + 2
                    while j < scalars.count, "0123456789;".unicodeScalars.contains(scalars[j]) { j += 1 }
                    if j < scalars.count, CharacterSet.letters.contains(scalars[j]) || scalars[j] == "~" {
                        flush()
                        i = j + 1
                        continue
                    }
                }
                if i + 1 < scalars.count, scalars[i + 1].value >= 32, scalars[i + 1] != "\u{7f}" {
                    push("alt+\(Character(scalars[i + 1]))")
                    i += 2
                    continue
                }
                push("esc")
                i += 1
                continue
            }
            if let key = named[String(Character(ch))] {
                push(key)
                i += 1
                continue
            }
            if ch.value < 32 {
                push("ctrl+\(Character(UnicodeScalar(ch.value + 96)!))")
                i += 1
                continue
            }
            text.unicodeScalars.append(ch)
            i += 1
        }
        flush()
        return out
    }
}

// MARK: - What's new (gizzi-code changelog)

struct GizziRelease: Codable, Sendable, Equatable, Identifiable {
    struct Section: Codable, Sendable, Equatable {
        var kind: String
        var items: [String]
    }

    var version: String
    var date: String?
    var summary: String?
    var sections: [Section]

    var id: String { version }
}

/// The gizzi-code releases for the What's new sheet (views/code/code-whats-new.ts):
/// read live from the gizzi-code CHANGELOG on GitHub, cached with a 30 min TTL.
enum GizziChangelog {
    static let rawURL = URL(string: "https://raw.githubusercontent.com/Allternit/allternit-platform/main/cmd/gizzi-code/CHANGELOG.md")!
    static let pageURL = URL(string: "https://github.com/Allternit/allternit-platform/blob/main/cmd/gizzi-code/CHANGELOG.md")!
    static let seenKey = "allternit:code-whats-new:seen"
    private static let cacheKey = "allternit:gizzi-code-changelog:v1"
    private static let cacheTimeKey = "allternit:gizzi-code-changelog:v1:at"
    private static let ttl: TimeInterval = 30 * 60
    private static let maxReleases = 10

    static func cached() -> [GizziRelease]? {
        guard let data = UserDefaults.standard.data(forKey: cacheKey) else { return nil }
        return try? JSONDecoder().decode([GizziRelease].self, from: data)
    }

    /// Cached copy while fresh, else the live file; the cache on failure.
    static func load(force: Bool = false) async throws -> [GizziRelease] {
        let at = UserDefaults.standard.double(forKey: cacheTimeKey)
        if !force, let cached = cached(), Date().timeIntervalSince1970 - at < ttl {
            return cached
        }
        do {
            let (data, response) = try await URLSession.shared.data(from: rawURL)
            guard (response as? HTTPURLResponse)?.statusCode == 200 else { throw APIError.invalidResponse }
            let releases = parse(String(decoding: data, as: UTF8.self))
            if let encoded = try? JSONEncoder().encode(releases) {
                UserDefaults.standard.set(encoded, forKey: cacheKey)
                UserDefaults.standard.set(Date().timeIntervalSince1970, forKey: cacheTimeKey)
            }
            return releases
        } catch {
            if let cached = cached() { return cached }
            throw error
        }
    }

    /// Latest released (non-Unreleased) version — what the unread dot keys on.
    static func latestVersion(_ releases: [GizziRelease]) -> String? {
        releases.first { $0.version != "Unreleased" }?.version
    }

    /// `parseGizziChangelog`: Keep a Changelog headings (`## 2.2.3 (2026-10-07)`,
    /// `## 2.0.7 — 2026-09-06`, `## Unreleased`), `###` sections, bullets that
    /// may wrap, prose under a version heading as its summary.
    static func parse(_ markdown: String) -> [GizziRelease] {
        var releases: [GizziRelease] = []
        var current: GizziRelease? = nil
        var sectionIndex: Int? = nil
        var item: [String]? = nil
        var summary: [String]? = nil

        func flushItem() {
            if let lines = item, let index = sectionIndex, current != nil {
                let text = clean(lines.joined(separator: " "))
                if !text.isEmpty { current!.sections[index].items.append(text) }
            }
            item = nil
        }
        func flushSummary() {
            if let lines = summary, current != nil, current!.summary == nil {
                let text = clean(lines.joined(separator: " "))
                if !text.isEmpty { current!.summary = text }
            }
            summary = nil
        }
        func flushRelease() {
            flushItem()
            flushSummary()
            if var release = current {
                release.sections = release.sections.filter { !$0.items.isEmpty }
                if !release.sections.isEmpty || release.summary != nil { releases.append(release) }
            }
            current = nil
            sectionIndex = nil
        }

        let heading = try? NSRegularExpression(pattern: #"^##\s+(Unreleased|v?(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?))\b(.*)$"#, options: [.caseInsensitive])
        let dateRegex = try? NSRegularExpression(pattern: #"(\d{4}-\d{2}-\d{2})"#)

        for raw in markdown.components(separatedBy: .newlines) {
            let line = raw.replacingOccurrences(of: #"\s+$"#, with: "", options: .regularExpression)
            if line.hasPrefix("## ") || line == "##" {
                flushRelease()
                if releases.filter({ $0.version != "Unreleased" }).count >= maxReleases { break }
                let ns = line as NSString
                if let match = heading?.firstMatch(in: line, range: NSRange(location: 0, length: ns.length)) {
                    let isUnreleased = ns.substring(with: match.range(at: 1)).lowercased() == "unreleased"
                    let version = isUnreleased ? "Unreleased" : ns.substring(with: match.range(at: 2))
                    let rest = match.range(at: 3).location != NSNotFound ? ns.substring(with: match.range(at: 3)) : ""
                    let restNS = rest as NSString
                    let date = dateRegex?.firstMatch(in: rest, range: NSRange(location: 0, length: restNS.length))
                        .map { restNS.substring(with: $0.range(at: 1)) }
                    current = GizziRelease(version: version, date: date, summary: nil, sections: [])
                }
                continue
            }
            guard current != nil else { continue }
            if line.hasPrefix("###") {
                flushItem()
                flushSummary()
                let kind = line.drop(while: { $0 == "#" }).trimmingCharacters(in: .whitespaces)
                current!.sections.append(.init(kind: clean(kind), items: []))
                sectionIndex = current!.sections.count - 1
                continue
            }
            if line.trimmingCharacters(in: .whitespaces).isEmpty {
                flushItem()
                flushSummary()
                continue
            }
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if let first = trimmed.first, "-*+".contains(first), trimmed.dropFirst().first == " " {
                flushItem()
                flushSummary()
                if sectionIndex == nil {
                    current!.sections.append(.init(kind: "", items: []))
                    sectionIndex = current!.sections.count - 1
                }
                item = [String(trimmed.dropFirst(2))]
                continue
            }
            if item != nil {
                item!.append(trimmed)
            } else if sectionIndex == nil {
                summary = (summary ?? []) + [trimmed]
            }
        }
        flushRelease()
        return releases
    }

    /// `[text](url)` → text, `**bold**` → bold, whitespace collapsed.
    private static func clean(_ text: String) -> String {
        text
            .replacingOccurrences(of: #"\[([^\]]+)\]\([^)]+\)"#, with: "$1", options: .regularExpression)
            .replacingOccurrences(of: #"\*\*([^*]+)\*\*"#, with: "$1", options: .regularExpression)
            .replacingOccurrences(of: #"\s+"#, with: " ", options: .regularExpression)
            .trimmingCharacters(in: .whitespaces)
    }
}
