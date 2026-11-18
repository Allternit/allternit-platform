import Combine
import Foundation

// The project pad (project folders plan, "Pad" contract): the free grid of
// tiles a project opens on, plus its own notes, links, checklist and quick
// actions. One record per project scope (`project:<mode>:<id>`), stored on the
// runtime at `GET/PUT /api/v1/project-pad/:scope`.
//
// Every type and rule here mirrors `allternit-ai/src/lib/projects/project-pad.ts`
// and `useProjectPad.ts`, and every limit matches the runtime's
// `validate_project_pad`, so web and iOS read and write the same records.

// MARK: - Limits and ids

enum PadLimits {
    static let tiles = 24
    static let notes = 4000
    static let links = 20
    static let linkTitle = 80
    static let url = 2048
    static let checklist = 50
    static let checkId = 40
    static let checkText = 200
    static let actions = 12
    static let actionId = 40
    static let actionLabel = 40
    static let actionValue = 2048
    static let template = 40
    static let templates = 24
    static let templateName = 40
}

/// The 8 Style tiles plus the pad's own 5.
enum PadTileID: String, CaseIterable, Identifiable, Codable, Sendable {
    case decision, countdown, progress, team, chart, schedule, files, activity
    case actions, links, notes, checklist, threads

    var id: String { rawValue }
    static let newTiles: [PadTileID] = [.actions, .links, .notes, .checklist, .threads]

    init(_ tile: ProjectTile) {
        // Every ProjectTile raw value is a PadTileID raw value.
        self = PadTileID(rawValue: tile.rawValue) ?? .decision
    }

    /// The Style tile this is, or nil for the pad's own tiles.
    var projectTile: ProjectTile? { ProjectTile(rawValue: rawValue) }

    var name: String {
        if let projectTile { return projectTile.name }
        switch self {
        case .actions: return "Quick actions"
        case .links: return "Links"
        case .notes: return "Notes"
        case .checklist: return "Checklist"
        default: return "Threads"
        }
    }

    /// The size a tile gets when added, or when an old look is converted.
    var defaultSize: PadSize {
        switch self {
        case .countdown, .progress, .chart: return .S
        case .activity, .checklist: return .L
        default: return .M
        }
    }
}

enum PadSize: String, CaseIterable, Codable, Sendable {
    case S, M, L, W
    /// Grid span on the 4-column grid (phones use 2 columns).
    var span: (columns: Int, rows: Int) {
        switch self {
        case .S: return (1, 1)
        case .M: return (2, 1)
        case .L: return (2, 2)
        case .W: return (4, 1)
        }
    }
    var next: PadSize {
        switch self {
        case .S: return .M
        case .M: return .L
        case .L: return .W
        case .W: return .S
        }
    }
}

/// The four modes a pad template is picked by.
enum PadMode: String, CaseIterable, Codable, Sendable {
    case chat, code, design, bot

    /// web `padModeOf`: code, design and bot keep their own; everything else
    /// (chat, cowork, task) picks templates as chat.
    static func of(_ mode: String) -> PadMode {
        switch mode {
        case "code": return .code
        case "design": return .design
        case "bot": return .bot
        default: return .chat
        }
    }

    var defaultTemplateID: String {
        switch self {
        case .chat: return "research"
        case .code: return "build"
        case .design: return "client"
        case .bot: return "launchpad"
        }
    }
}

/// A Style (one of the 10 layouts) or `pad`, the free grid.
enum PadLayout: String, CaseIterable, Codable, Sendable {
    case pulse, bento, mission, score, cover, runway, board, crew, orbit, gallery, pad

    init(_ kind: ProjectLayoutKind) {
        self = PadLayout(rawValue: kind.rawValue) ?? .pad
    }

    /// The Style to draw, or nil for the free grid.
    var style: ProjectLayoutKind? { self == .pad ? nil : ProjectLayoutKind(rawValue: rawValue) }
}

enum PadView: String, Codable, Sendable { case pad, list }

// MARK: - Records

struct PadTile: Codable, Equatable, Hashable, Sendable {
    var id: PadTileID
    var size: PadSize
}

struct PadLink: Codable, Equatable, Sendable {
    var title: String
    var url: String
}

struct PadCheckItem: Codable, Equatable, Identifiable, Sendable {
    var id: String
    var text: String
    var done: Bool
}

struct PadAction: Codable, Equatable, Identifiable, Sendable {
    enum Kind: String, Codable, Sendable { case thread, link }
    var id: String
    var kind: Kind
    var label: String
    /// A thread id for `thread`, an http(s) URL for `link`.
    var value: String
}

struct PadData: Codable, Equatable, Sendable {
    var notes: String = ""
    var links: [PadLink] = []
    var checklist: [PadCheckItem] = []
    var actions: [PadAction] = []

    static let empty = PadData()
}

/// One project's pad. Encodes to exactly the web's `ProjectPad` JSON.
struct ProjectPad: Codable, Equatable, Sendable {
    var v: Int = 1
    var view: PadView = .pad
    var layout: PadLayout = .pad
    /// A `ProjectColorSet` id.
    var colors: String = "quiet"
    /// Style tiles switched off (same meaning as ProjectLook.hidden).
    var hidden: [PadTileID] = []
    /// The template the tiles came from, or nil once edited by hand / converted.
    var template: String?
    var tiles: [PadTile] = []
    var data: PadData = .empty
    /// Milliseconds since 1970 (JavaScript `Date.now()`).
    var updatedAt: Int64 = 0

    private enum CodingKeys: String, CodingKey { case v, view, layout, colors, hidden, template, tiles, data, updatedAt }

    init(view: PadView = .pad, layout: PadLayout = .pad, colors: String = "quiet", hidden: [PadTileID] = [],
         template: String? = nil, tiles: [PadTile] = [], data: PadData = .empty, updatedAt: Int64 = 0) {
        self.view = view; self.layout = layout; self.colors = colors; self.hidden = hidden
        self.template = template; self.tiles = tiles; self.data = data; self.updatedAt = updatedAt
    }

    /// Lenient: whatever arrives goes through `normalize`, like the web.
    init(from decoder: Decoder) throws {
        let value = try decoder.singleValueContainer().decode(PadJSONValue.self)
        self = ProjectPad.normalize(value.object, mode: .chat)
    }

    /// `template` is written as `null` when unset, as the web does.
    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(1, forKey: .v)
        try c.encode(view, forKey: .view)
        try c.encode(layout, forKey: .layout)
        try c.encode(colors, forKey: .colors)
        try c.encode(hidden, forKey: .hidden)
        try c.encode(template, forKey: .template)
        try c.encode(tiles, forKey: .tiles)
        try c.encode(data, forKey: .data)
        try c.encode(updatedAt, forKey: .updatedAt)
    }

    static func nowMillis() -> Int64 { Int64((Date().timeIntervalSince1970 * 1000).rounded()) }
}

/// A starter template (built in) or one the person saved to their account.
struct PadTemplate: Codable, Equatable, Identifiable, Sendable {
    /// Built-in id, or `u_<uuid>` for an account template.
    var id: String
    var name: String
    var blurb: String?
    var builtIn: Bool = false
    var tiles: [PadTile]
    var actions: [PadAction] = []
    var createdAt: Int64?

    private enum CodingKeys: String, CodingKey { case id, name, tiles, actions, createdAt }

    init(id: String, name: String, blurb: String? = nil, builtIn: Bool = false, tiles: [PadTile],
         actions: [PadAction] = [], createdAt: Int64? = nil) {
        self.id = id; self.name = name; self.blurb = blurb; self.builtIn = builtIn
        self.tiles = tiles; self.actions = actions; self.createdAt = createdAt
    }

    /// Lenient, like the web's `normalizePadTemplate`; an unusable template fails to decode.
    init(from decoder: Decoder) throws {
        let value = try decoder.singleValueContainer().decode(PadJSONValue.self)
        guard let template = PadTemplate.normalize(value.object) else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "Unusable pad template"))
        }
        self = template
    }

    /// The account document's shape: id, name, tiles, actions, createdAt.
    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(id, forKey: .id)
        try c.encode(name, forKey: .name)
        try c.encode(tiles, forKey: .tiles)
        try c.encode(actions, forKey: .actions)
        try c.encode(createdAt ?? 0, forKey: .createdAt)
    }

    private static func t(_ id: PadTileID, _ size: PadSize) -> PadTile { PadTile(id: id, size: size) }

    /// web `BUILT_IN_TEMPLATES`, in the same order.
    static let builtIns: [PadTemplate] = [
        PadTemplate(id: "launch", name: "Launch", blurb: "Decisions, countdown and the launch list", builtIn: true,
                    tiles: [t(.decision, .M), t(.countdown, .S), t(.progress, .S), t(.checklist, .L), t(.schedule, .M), t(.actions, .M)]),
        PadTemplate(id: "client", name: "Client work", blurb: "What the client needs, files and notes", builtIn: true,
                    tiles: [t(.decision, .M), t(.progress, .S), t(.schedule, .S), t(.files, .M), t(.notes, .M), t(.links, .W)]),
        // The mockup's "Number" tile is out of scope; Progress (S) takes its place.
        PadTemplate(id: "build", name: "Build", blurb: "Agents, changes and run buttons", builtIn: true,
                    tiles: [t(.decision, .M), t(.actions, .M), t(.activity, .L), t(.team, .M), t(.progress, .S), t(.chart, .S)]),
        PadTemplate(id: "research", name: "Research", blurb: "Notes, sources and threads", builtIn: true,
                    tiles: [t(.notes, .L), t(.threads, .M), t(.links, .M), t(.files, .W)]),
        PadTemplate(id: "launchpad", name: "Launchpad", blurb: "Your buttons and links, nothing else", builtIn: true,
                    tiles: [t(.actions, .W), t(.links, .M), t(.notes, .M), t(.threads, .W)]),
    ]

    static func builtIn(id: String) -> PadTemplate? { builtIns.first { $0.id == id } }
    static var research: PadTemplate { builtIn(id: "research")! }

    static func isBuiltInID(_ id: String) -> Bool { builtIn(id: id) != nil }

    /// `u_` plus a UUID, the form cloud-api accepts.
    static func isUserID(_ id: String) -> Bool {
        id.range(of: "^u_[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$",
                 options: .regularExpression) != nil
    }

    static func newUserID() -> String { "u_" + UUID().uuidString.lowercased() }
}

// MARK: - JSON helpers

/// Any JSON value, kept as Foundation objects so `normalize` can read it the
/// way the web reads `unknown`.
struct PadJSONValue: Decodable {
    let object: Any?
    init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() { object = nil }
        else if let b = try? c.decode(Bool.self) { object = NSNumber(value: b) }
        else if let n = try? c.decode(Double.self) { object = NSNumber(value: n) }
        else if let s = try? c.decode(String.self) { object = s }
        else if let a = try? c.decode([PadJSONValue].self) { object = a.map { $0.object ?? NSNull() } }
        else if let d = try? c.decode([String: PadJSONValue].self) { object = d.mapValues { $0.object ?? NSNull() } }
        else { object = nil }
    }
}

enum PadJSON {
    /// Cut to `max` characters counted as the servers count them (Unicode scalars).
    static func clip(_ value: Any?, _ max: Int) -> String {
        guard let s = value as? String else { return "" }
        return clip(s, max)
    }

    static func clip(_ s: String, _ max: Int) -> String {
        var scalars = String.UnicodeScalarView()
        scalars.append(contentsOf: s.unicodeScalars.prefix(max))
        return String(scalars)
    }

    static func length(_ s: String) -> Int { s.unicodeScalars.count }

    static func trimmed(_ value: Any?) -> String {
        (value as? String)?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    }

    static func isBool(_ value: Any?) -> Bool {
        guard let n = value as? NSNumber else { return false }
        return CFGetTypeID(n) == CFBooleanGetTypeID()
    }

    /// `=== true`: a JSON true, never a number.
    static func isTrue(_ value: Any?) -> Bool {
        guard isBool(value) else { return false }
        return (value as? NSNumber)?.boolValue ?? false
    }

    /// A finite JSON number that is not a boolean.
    static func number(_ value: Any?) -> Double? {
        guard !isBool(value), let n = value as? NSNumber else { return nil }
        let d = n.doubleValue
        return d.isFinite ? d : nil
    }

    static func millis(_ value: Any?) -> Int64 {
        guard let d = number(value), d >= 0, d < 9.0e18 else { return 0 }
        return Int64(d)
    }

    /// An absolute http(s) URL with a host and no spaces; anything else
    /// (javascript:, data:) is nil. web `safeHttpUrl`.
    static func safeHttpURL(_ value: Any?) -> String? {
        guard let s = value as? String, !s.isEmpty, length(s) <= PadLimits.url else { return nil }
        if s.unicodeScalars.contains(where: { $0.properties.isWhitespace || $0.properties.generalCategory == .control || $0 == "\u{FEFF}" }) {
            return nil
        }
        let lower = s.lowercased()
        guard lower.hasPrefix("http://") || lower.hasPrefix("https://"),
              let url = URL(string: s), let scheme = url.scheme?.lowercased(), scheme == "http" || scheme == "https",
              let host = url.host, !host.isEmpty else { return nil }
        return s
    }

    /// A short local id for checklist items and actions (web `padItemId`).
    static func itemID(_ prefix: String = "i") -> String {
        counter.increment()
        let random = UUID().uuidString.lowercased().prefix(8)
        return prefix + String(random) + String(counter.value % 1_000_000, radix: 36)
    }

    private final class Counter: @unchecked Sendable {
        private let lock = NSLock()
        private var n = 0
        var value: Int { lock.lock(); defer { lock.unlock() }; return n }
        func increment() { lock.lock(); n += 1; lock.unlock() }
    }
    private static let counter = Counter()

    static func object<T: Encodable>(_ value: T) -> Any? {
        guard let data = try? JSONEncoder().encode(value) else { return nil }
        return try? JSONSerialization.jsonObject(with: data, options: [.fragmentsAllowed])
    }
}

// MARK: - Normalizing (web normalizePad and friends)

extension PadTile {
    static func normalize(_ raw: Any?) -> [PadTile] {
        guard let items = raw as? [Any] else { return [] }
        var seen = Set<PadTileID>()
        var out: [PadTile] = []
        for item in items {
            guard let r = item as? [String: Any], let id = (r["id"] as? String).flatMap(PadTileID.init(rawValue:)),
                  !seen.contains(id) else { continue }
            seen.insert(id)
            out.append(PadTile(id: id, size: (r["size"] as? String).flatMap(PadSize.init(rawValue:)) ?? id.defaultSize))
            if out.count >= PadLimits.tiles { break }
        }
        return out
    }
}

extension PadAction {
    static func normalize(_ raw: Any?) -> [PadAction] {
        guard let items = raw as? [Any] else { return [] }
        var out: [PadAction] = []
        var ids = Set<String>()
        for item in items {
            guard let r = item as? [String: Any], let kind = (r["kind"] as? String).flatMap(Kind.init(rawValue:)) else { continue }
            let label = PadJSON.clip(PadJSON.trimmed(r["label"]), PadLimits.actionLabel)
            guard !label.isEmpty else { continue }
            let value: String?
            switch kind {
            case .link: value = PadJSON.safeHttpURL(r["value"])
            case .thread: value = PadJSON.trimmed(r["value"]).isEmpty ? nil : PadJSON.clip(r["value"], PadLimits.actionValue)
            }
            guard let value else { continue }
            var id = (r["id"] as? String).flatMap { !$0.isEmpty && PadJSON.length($0) <= PadLimits.actionId ? $0 : nil } ?? PadJSON.itemID("a")
            if ids.contains(id) { id = PadJSON.itemID("a") }
            ids.insert(id)
            out.append(PadAction(id: id, kind: kind, label: label, value: value))
            if out.count >= PadLimits.actions { break }
        }
        return out
    }
}

extension PadData {
    static func normalize(_ raw: Any?) -> PadData {
        let r = raw as? [String: Any] ?? [:]
        var links: [PadLink] = []
        for item in r["links"] as? [Any] ?? [] {
            let l = item as? [String: Any]
            guard let url = PadJSON.safeHttpURL(l?["url"]) else { continue }
            var title = PadJSON.clip(PadJSON.trimmed(l?["title"]), PadLimits.linkTitle)
            if title.isEmpty { title = PadJSON.clip(URL(string: url)?.host ?? url, PadLimits.linkTitle) }
            links.append(PadLink(title: title, url: url))
            if links.count >= PadLimits.links { break }
        }
        var checklist: [PadCheckItem] = []
        var ids = Set<String>()
        for item in r["checklist"] as? [Any] ?? [] {
            guard let c = item as? [String: Any] else { continue }
            var id = (c["id"] as? String).flatMap { !$0.isEmpty && PadJSON.length($0) <= PadLimits.checkId ? $0 : nil } ?? PadJSON.itemID("c")
            if ids.contains(id) { id = PadJSON.itemID("c") }
            ids.insert(id)
            checklist.append(PadCheckItem(id: id, text: PadJSON.clip(c["text"], PadLimits.checkText), done: PadJSON.isTrue(c["done"])))
            if checklist.count >= PadLimits.checklist { break }
        }
        return PadData(notes: PadJSON.clip(r["notes"], PadLimits.notes), links: links, checklist: checklist,
                       actions: PadAction.normalize(r["actions"]))
    }
}

extension ProjectPad {
    static let colorIDs: Set<String> = Set(ProjectColorSet.all.map(\.id))

    /// Whatever was stored or received, as a pad that is safe to show and to save.
    static func normalize(_ raw: Any?, mode: PadMode) -> ProjectPad {
        guard let r = raw as? [String: Any] else { return defaultPad(mode: mode) }
        var hidden: [PadTileID] = []
        for id in (r["hidden"] as? [Any] ?? []).compactMap({ ($0 as? String).flatMap(PadTileID.init(rawValue:)) })
            where !hidden.contains(id) { hidden.append(id) }
        let layoutRaw = r["layout"] as? String
        // An old "Make one" layout has no tiles of its own here, so it is the free grid.
        let layout: PadLayout = layoutRaw == "custom" ? .pad : layoutRaw.flatMap(PadLayout.init(rawValue:)) ?? .pad
        let colors = (r["colors"] as? String).flatMap { ProjectColorSet.isValid($0) ? $0 : nil } ?? "quiet"
        let template = (r["template"] as? String).flatMap { !$0.isEmpty && PadJSON.length($0) <= PadLimits.template ? $0 : nil }
        return ProjectPad(view: (r["view"] as? String) == "list" ? .list : .pad, layout: layout, colors: colors,
                          hidden: hidden, template: template, tiles: PadTile.normalize(r["tiles"]),
                          data: PadData.normalize(r["data"]), updatedAt: PadJSON.millis(r["updatedAt"]))
    }

    /// Stored JSON (local copy) as a pad, or nil when it is not a JSON object.
    static func decodeStored(_ data: Data, mode: PadMode) -> ProjectPad? {
        guard let raw = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return nil }
        return normalize(raw, mode: mode)
    }

    /// The runtime's `{ pad: record | null }`: nil for null or a missing pad.
    /// Throws when the body is not a JSON object.
    static func decodeResponse(_ data: Data, mode: PadMode) throws -> ProjectPad? {
        guard let body = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw APIError.invalidResponse
        }
        guard let pad = body["pad"], !(pad is NSNull) else { return nil }
        return normalize(pad, mode: mode)
    }

    /// This pad with every limit applied (same rules as `normalize`).
    func normalized(mode: PadMode) -> ProjectPad {
        ProjectPad.normalize(PadJSON.object(self), mode: mode)
    }

    /// A pad built from a template. Keeps `base`'s notes, links, checklist,
    /// colors and view when given (web `padFromTemplate`).
    static func fromTemplate(_ template: PadTemplate, base: ProjectPad? = nil, now: Int64 = ProjectPad.nowMillis()) -> ProjectPad {
        var data = base?.data ?? .empty
        if !template.actions.isEmpty { data.actions = template.actions }
        let id = PadJSON.clip(template.id, PadLimits.template)
        return ProjectPad(view: base?.view ?? .pad, layout: .pad, colors: base?.colors ?? "quiet", hidden: [],
                          template: id.isEmpty ? nil : id, tiles: template.tiles, data: data, updatedAt: now)
            .normalized(mode: .chat)
    }

    /// The pad a project with nothing saved opens on: the account's default
    /// template for its mode when known, else the mode's built-in default.
    static func defaultPad(mode: PadMode, template: PadTemplate? = nil) -> ProjectPad {
        fromTemplate(template ?? PadTemplate.builtIn(id: mode.defaultTemplateID) ?? .research, now: 0)
    }

    /// A look saved before pads, as a pad, so nothing a person chose is lost:
    /// a Style stays the layout with its hidden tiles; "Make one" (`custom`)
    /// becomes the free grid with its tiles at default sizes (web `lookToPad`).
    static func fromLook(_ look: ProjectLook, now: Int64 = ProjectPad.nowMillis()) -> ProjectPad {
        let at = { (ids: [ProjectTile]) in ids.map { PadTile(id: PadTileID($0), size: PadTileID($0).defaultSize) } }
        if look.layout == .custom {
            let ids = look.customTiles ?? ProjectTile.allCases.filter { !look.hidden.contains($0) }
            return ProjectPad(view: .pad, layout: .pad, colors: look.colors, hidden: [], template: nil,
                              tiles: at(ids), data: .empty, updatedAt: now).normalized(mode: .chat)
        }
        return ProjectPad(view: .pad, layout: PadLayout(look.layout), colors: look.colors,
                          hidden: look.hidden.map { PadTileID($0) }, template: nil,
                          tiles: at(look.layout.tiles.filter { !look.hidden.contains($0) }),
                          data: .empty, updatedAt: now).normalized(mode: .chat)
    }
}

extension PadTemplate {
    /// A template from storage or the network, or nil when it cannot be used
    /// (web `normalizePadTemplate`: account templates only).
    static func normalize(_ raw: Any?) -> PadTemplate? {
        guard let r = raw as? [String: Any], let id = r["id"] as? String, isUserID(id) else { return nil }
        let name = PadJSON.clip(PadJSON.trimmed(r["name"]), PadLimits.templateName)
        guard !name.isEmpty else { return nil }
        return PadTemplate(id: id, name: name, tiles: PadTile.normalize(r["tiles"]),
                           actions: PadAction.normalize(r["actions"]), createdAt: PadJSON.millis(r["createdAt"]))
    }
}

// MARK: - Runtime transport

/// What one runtime call came back with.
enum PadOutcome: Equatable, Sendable {
    /// 2xx with the raw body.
    case ok(Data)
    /// 503 `runtime_unavailable`, `.unavailable` routing, or no network.
    case local
    /// 404: a runtime without the pad route.
    case oldRuntime
    /// 401 / 403.
    case signedOut
    /// Any other failure.
    case failed
}

protocol ProjectPadTransport: Sendable {
    func fetch(scope: String) async -> PadOutcome
    /// `nil` deletes the record.
    func put(scope: String, pad: ProjectPad?) async -> PadOutcome
}

/// `GET/PUT /project-pad/:scope` through `APIClient` (`.direct`, `.relay`, `.unavailable`).
struct RuntimeProjectPadTransport: ProjectPadTransport {
    let client: APIClient

    init(client: APIClient = .shared) { self.client = client }

    /// `project-pad/<encodeURIComponent(scope)>`, relative to the API base.
    static func path(scope: String) -> String {
        var allowed = CharacterSet(charactersIn: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789")
        allowed.insert(charactersIn: "-_.!~*'()")
        return "project-pad/" + (scope.addingPercentEncoding(withAllowedCharacters: allowed) ?? scope)
    }

    func fetch(scope: String) async -> PadOutcome { await call(scope: scope, method: "GET", body: nil) }

    func put(scope: String, pad: ProjectPad?) async -> PadOutcome {
        /// `{"pad": …}` with an explicit null for a delete.
        struct Body: Encodable {
            let pad: ProjectPad?
            func encode(to encoder: Encoder) throws {
                var c = encoder.container(keyedBy: CodingKeys.self)
                try c.encode(pad, forKey: .pad)
            }
            enum CodingKeys: String, CodingKey { case pad }
        }
        guard let body = try? JSONEncoder().encode(Body(pad: pad)) else { return .failed }
        return await call(scope: scope, method: "PUT", body: body)
    }

    private func call(scope: String, method: String, body: Data?) async -> PadOutcome {
        var request: URLRequest
        do {
            // Built here, not via authorizedRequest(path:), because
            // appendingPathComponent would encode the "%" a second time.
            guard var components = URLComponents(url: client.baseURL, resolvingAgainstBaseURL: false) else { return .failed }
            let base = components.percentEncodedPath.hasSuffix("/")
                ? String(components.percentEncodedPath.dropLast()) : components.percentEncodedPath
            components.percentEncodedPath = base + "/" + Self.path(scope: scope)
            guard let url = components.url else { return .failed }
            request = try await client.authorizedRequest(url: url, method: method)
        } catch {
            return .local
        }
        request.httpBody = body
        do {
            let (data, response) = try await client.send(request)
            guard let status = (response as? HTTPURLResponse)?.statusCode else { return .failed }
            return Self.outcome(status: status, data: data)
        } catch APIError.httpError(let status, _) {
            return Self.outcome(status: status, data: Data())
        } catch {
            // No runtime reachable at all: the same as a runtime that is asleep.
            return .local
        }
    }

    static func outcome(status: Int, data: Data) -> PadOutcome {
        switch status {
        case 200...299: return .ok(data)
        case 503: return .local
        case 404: return .oldRuntime
        case 401, 403: return .signedOut
        default: return .failed
        }
    }
}

// MARK: - Store

/// A project's pad, synced to the connected runtime with a copy on this
/// device (web `useProjectPad`). Same fallback on every client:
///
/// - runtime unavailable (503, `.unavailable`, no network) → this device only,
///   `status == .local`;
/// - 404 (an older runtime) → this device only, `status == .oldRuntime`;
/// - the first good GET answers null → upload this device's pad, or the old
///   saved look (`project.look.for.<scope>`) converted with `fromLook`. The old
///   key stays in place for older builds;
/// - signed out → read-only defaults, `status == .signedOut`.
///
/// Saves are optimistic, debounced (~400 ms) and last-write-wins. Call
/// `load()` when the view appears and `flush()` when it goes away.
@MainActor
final class ProjectPadStore: ObservableObject {
    enum SyncStatus: String, Sendable { case loading, synced, local, oldRuntime, signedOut }

    /// The one inline line each non-synced status shows.
    static let statusMessages: [SyncStatus: String] = [
        .local: "Saved on this device only. Connect a computer to sync.",
        .oldRuntime: "Update your computer to sync pads.",
        .signedOut: "Sign in to customize",
    ]
    static let localPrefix = "allternit.pad.v1."
    static let changedNotification = Notification.Name("allternit.projectPadChanged")

    /// The pad to draw: this project's own, or the default template's.
    @Published private(set) var pad: ProjectPad
    @Published private(set) var status: SyncStatus = .loading
    /// A load or sync failure worth showing (the pad still shows).
    @Published private(set) var error: String?

    let scope: String
    let mode: PadMode

    /// The status line to show under the pad, if any.
    var statusMessage: String? { Self.statusMessages[status] }
    /// Whether the project has a pad of its own (not just the default template).
    var hasOwnPad: Bool { stored != nil }

    private let legacyLookScopes: [String]
    private var defaultTemplate: PadTemplate?
    private let transport: ProjectPadTransport
    private let defaults: UserDefaults
    private let isSignedIn: @MainActor () -> Bool
    private let saveDelay: UInt64
    private let source = UUID().uuidString
    private var stored: ProjectPad?
    private var edits = 0
    private var generation = 0
    private var saveTask: Task<Void, Never>?
    private var loadTask: Task<Void, Never>?
    private var writeChain: Task<Void, Never>?
    private var observer: AnyCancellable?

    /// - Parameters:
    ///   - scope: `project:<mode>:<id>` with the web's mode (`ProjectPadStore.scope(mode:id:)`).
    ///   - mode: the mode templates are picked by (`PadMode.of(_:)`).
    ///   - legacyLookScopes: other scopes an older iOS build saved this
    ///     project's look under (chat projects used `project:chat:<id>`).
    ///     The pad's own scope is always checked first.
    init(scope: String,
         mode: PadMode,
         legacyLookScopes: [String] = [],
         defaultTemplate: PadTemplate? = nil,
         transport: ProjectPadTransport = RuntimeProjectPadTransport(),
         defaults: UserDefaults = .standard,
         saveDelayMilliseconds: UInt64 = 400,
         isSignedIn: @escaping @MainActor () -> Bool = { AuthManager.shared.isSignedIn || AuthManager.shared.isSkippingAuth }) {
        self.scope = scope
        self.mode = mode
        self.legacyLookScopes = legacyLookScopes.filter { $0 != scope }
        self.defaultTemplate = defaultTemplate
        self.transport = transport
        self.defaults = defaults
        self.isSignedIn = isSignedIn
        self.saveDelay = saveDelayMilliseconds * 1_000_000
        self.pad = ProjectPad.defaultPad(mode: mode, template: defaultTemplate)
        observer = NotificationCenter.default.publisher(for: Self.changedNotification)
            .receive(on: RunLoop.main)
            .sink { [weak self] note in
                MainActor.assumeIsolated {
                    guard let self,
                          note.userInfo?["scope"] as? String == self.scope,
                          note.userInfo?["source"] as? String != self.source else { return }
                    self.show(self.readLocal())
                }
            }
    }

    /// The pad scope for a project, as the web builds it from the appearance
    /// key (`projectBackdropScope`): `project:<mode>:<id>`. Cowork (task)
    /// projects use mode `cowork`; bot projects `bot`.
    nonisolated static func scope(mode: String, id: String) -> String {
        AppearanceClient.projectKey(mode: mode, id: id).joined(separator: ":")
    }

    /// The scope form the runtime accepts.
    nonisolated static func isValidScope(_ scope: String) -> Bool {
        scope.range(of: "^project:[a-z]{2,16}:[A-Za-z0-9_.:-]{1,128}$", options: .regularExpression) != nil
    }

    private var fallback: ProjectPad { ProjectPad.defaultPad(mode: mode, template: defaultTemplate) }

    /// The account's default template can arrive after the first paint. While
    /// this project has no pad of its own, it follows it.
    func setDefaultTemplate(_ template: PadTemplate?) {
        defaultTemplate = template
        if stored == nil { pad = fallback }
    }

    /// Reads this device's copy, then the runtime. Returns the load task so
    /// callers (and tests) can wait for it.
    @discardableResult
    func load() -> Task<Void, Never> {
        loadTask?.cancel()
        generation += 1
        let forGeneration = generation
        let startEdits = edits
        show(readLocal())
        error = nil
        guard Self.isValidScope(scope) else {
            status = .local
            return Task {}
        }
        guard isSignedIn() else {
            show(nil)
            status = .signedOut
            return Task {}
        }
        status = .loading
        let transport = transport
        let scope = scope
        let task = Task { [weak self] in
            let outcome = await transport.fetch(scope: scope)
            guard let self, self.generation == forGeneration, !Task.isCancelled else { return }
            await self.finishLoad(outcome, startEdits: startEdits)
        }
        loadTask = task
        return task
    }

    private func finishLoad(_ outcome: PadOutcome, startEdits: Int) async {
        // A save made while loading wins over what the server had.
        let edited = edits != startEdits
        switch outcome {
        case .ok(let body):
            let remote: ProjectPad?
            do {
                remote = try ProjectPad.decodeResponse(body, mode: mode)
            } catch {
                showConvertedLookIfEmpty(edited: edited)
                status = .local
                self.error = "The runtime sent a pad this app could not read."
                return
            }
            if let remote {
                if !edited {
                    show(remote)
                    writeLocal(remote)
                }
                status = .synced
                return
            }
            // Nothing on the runtime yet: upload this device's pad, or the converted old look.
            guard let candidate = edited ? stored : (readLocal() ?? convertedLook()) else {
                status = .synced
                return
            }
            if !edited {
                show(candidate)
                writeLocal(candidate)
            }
            await push(candidate).value
        case .signedOut:
            show(nil)
            status = .signedOut
        case .local, .oldRuntime, .failed:
            showConvertedLookIfEmpty(edited: edited)
            status = outcome == .oldRuntime ? .oldRuntime : .local
            if outcome == .failed { error = "The pad could not be loaded. It shows what this device has." }
        }
    }

    private func showConvertedLookIfEmpty(edited: Bool) {
        if !edited, stored == nil, let old = convertedLook() { show(old) }
    }

    /// Change the pad. Shows at once, saves on this device, and syncs after a
    /// short pause; the last save wins.
    func save(_ change: (inout ProjectPad) -> Void) {
        if status == .signedOut {
            error = Self.statusMessages[.signedOut]
            return
        }
        var next = stored ?? fallback
        change(&next)
        next.updatedAt = ProjectPad.nowMillis()
        next = next.normalized(mode: mode)
        edits += 1
        show(next)
        writeLocal(next)
        guard Self.isValidScope(scope) else { return }
        saveTask?.cancel()
        let delay = saveDelay
        saveTask = Task { [weak self] in
            if delay > 0 { try? await Task.sleep(nanoseconds: delay) }
            guard !Task.isCancelled, let self else { return }
            self.saveTask = nil
            if let pad = self.stored { self.push(pad) }
        }
    }

    /// Sends a save still waiting on its pause now, and waits for every write.
    func flush() async {
        if let pending = saveTask {
            pending.cancel()
            saveTask = nil
            if let pad = stored, Self.isValidScope(scope) { push(pad) }
        }
        await writeChain?.value
    }

    /// Writes go out one at a time so the last save lands last.
    @discardableResult
    private func push(_ next: ProjectPad) -> Task<Void, Never> {
        let previous = writeChain
        let transport = transport
        let scope = scope
        let task = Task { [weak self] in
            await previous?.value
            let outcome = await transport.put(scope: scope, pad: next)
            guard let self else { return }
            switch outcome {
            case .ok: self.status = .synced
            case .oldRuntime: self.status = .oldRuntime
            case .signedOut: self.status = .signedOut
            case .local, .failed: self.status = .local
            }
            switch outcome {
            case .failed: self.error = "The pad could not be synced. It is saved on this device."
            case .signedOut: self.error = Self.statusMessages[.signedOut]
            default: self.error = nil
            }
        }
        writeChain = task
        return task
    }

    private func show(_ next: ProjectPad?) {
        stored = next
        pad = next ?? fallback
    }

    // MARK: Local copy

    private var localKey: String { Self.localPrefix + scope }

    private func readLocal() -> ProjectPad? {
        guard let data = defaults.data(forKey: localKey) else { return nil }
        return ProjectPad.decodeStored(data, mode: mode)
    }

    private func writeLocal(_ pad: ProjectPad) {
        if let data = try? JSONEncoder().encode(pad) { defaults.set(data, forKey: localKey) }
        NotificationCenter.default.post(name: Self.changedNotification, object: nil,
                                        userInfo: ["scope": scope, "source": source])
    }

    /// The look saved before pads, converted. Read only; the old key stays.
    private func convertedLook() -> ProjectPad? {
        for lookScope in [scope] + legacyLookScopes {
            if let data = defaults.data(forKey: ProjectLook.storageKey(scope: lookScope)),
               let look = ProjectLook.decodeSaved(data) {
                return ProjectPad.fromLook(look)
            }
        }
        return nil
    }
}
