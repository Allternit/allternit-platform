import Foundation

// Runtime registry (Harness & Bots 2.0, CONTRACT-harness-bots-2.md, plan D2).
//
// The registry is the `runtimes[]` array of gizzi's cli-catalog.json (v3),
// served by cloud-api `GET /api/v1/runtimes` (public, ETag,
// `Cache-Control: public, max-age=3600`). The phone asks cloud-api, never a
// computer, so the list works with no computer and signed out.
//
// The last good body and its ETag are cached in Application Support, so a
// launch offline still shows the list and a revalidation is a cheap 304.
// Until the registry loads (first run offline) a small built-in list made
// from the legacy native harness ids keeps labels and picks working; the
// store's `status` says which one the app is reading.
//
// `fits` and `resolveRuntime` mirror gizzi `runtime-registry.ts` (the
// reference) and the web `src/lib/runtimes/compat.ts` rule for rule.

// MARK: - Wire types (same shapes as gizzi runtime-registry.ts)

/// `brains: "any" | [providerId]`.
enum RuntimeBrains: Decodable, Equatable, Sendable {
    case any
    case list([String])

    init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if let list = try? c.decode([String].self) { self = .list(list) } else { self = .any }
    }
}

/// `access: [plan|key|local|credits]`, also `botBrain.access` on a bot.
enum BrainAccess: String, Codable, CaseIterable, Sendable {
    case plan, key, local, credits

    var label: String {
        switch self {
        case .plan: return "Plan"
        case .key: return "API key"
        case .local: return "Local"
        case .credits: return "Credits"
        }
    }
}

struct RuntimeInstallState: Decodable, Equatable, Sendable {
    var installed: Bool
    var signedIn: Bool
    var version: String?

    private enum CodingKeys: String, CodingKey { case installed, signedIn, version }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        installed = ((try? c.decodeIfPresent(Bool.self, forKey: .installed)) ?? nil) ?? false
        signedIn = ((try? c.decodeIfPresent(Bool.self, forKey: .signedIn)) ?? nil) ?? false
        version = try? c.decodeIfPresent(String.self, forKey: .version)
    }
}

/// One `runtimes[]` entry. Everything but `id` is lenient so a newer server can
/// add fields or values without breaking older builds. Look tokens stay loose
/// JSON (`RuntimeLookView` reads them; unknown fields are ignored).
struct RuntimeEntry: Decodable, Identifiable, Equatable, Sendable {
    struct Adapter: Decodable, Equatable, Sendable {
        var kind: String
        var argv: [String]?
    }
    struct Status: Decodable, Equatable, Sendable {
        var label: String
        var tone: String?
    }
    struct Legacy: Decodable, Equatable, Sendable {
        var harness: String?
        var uhp: [String]?
    }

    var id: String
    var name: String
    var vendor: String
    /// native | coding | personal | general | experimental
    var category: String
    var tool: String?
    var aliases: [String]?
    /// Aliases of the runtime's CLI tool, when the body carries them.
    var toolAliases: [String]?
    var adapter: Adapter
    var brains: RuntimeBrains
    var access: [BrainAccess]
    var more: Bool
    var status: Status?
    var systemLayerVia: String?
    var look: JSONValue?
    var liveUi: JSONValue?
    var legacy: Legacy?
    var docs: String?
    /// Subscription plans this runtime may use (contract `runtimes[].plans`:
    /// claude, chatgpt, kimi, gemini, copilot). nil on an older server; read
    /// it through `RuntimeRegistry.plans(of:)`, which has the fallback table.
    var plans: [String]?

    private enum CodingKeys: String, CodingKey {
        case id, name, vendor, category, tool, aliases, toolAliases, adapter, brains, access, more, status
        case systemLayer, look, liveUi, legacy, docs, plans
    }
    private struct SystemLayer: Decodable { var via: String? }

    init(id: String, name: String, vendor: String, category: String, tool: String?, aliases: [String]? = nil,
         adapterKind: String, brains: RuntimeBrains, access: [BrainAccess] = [], more: Bool = false) {
        self.id = id
        self.name = name
        self.vendor = vendor
        self.category = category
        self.tool = tool
        self.aliases = aliases
        self.adapter = Adapter(kind: adapterKind, argv: nil)
        self.brains = brains
        self.access = access
        self.more = more
        self.systemLayerVia = "none"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        name = ((try? c.decodeIfPresent(String.self, forKey: .name)) ?? nil) ?? id
        vendor = ((try? c.decodeIfPresent(String.self, forKey: .vendor)) ?? nil) ?? name
        category = ((try? c.decodeIfPresent(String.self, forKey: .category)) ?? nil) ?? "coding"
        tool = try? c.decodeIfPresent(String.self, forKey: .tool)
        aliases = try? c.decodeIfPresent([String].self, forKey: .aliases)
        toolAliases = try? c.decodeIfPresent([String].self, forKey: .toolAliases)
        adapter = ((try? c.decodeIfPresent(Adapter.self, forKey: .adapter)) ?? nil) ?? Adapter(kind: "text", argv: nil)
        brains = ((try? c.decodeIfPresent(RuntimeBrains.self, forKey: .brains)) ?? nil) ?? .any
        let rawAccess = ((try? c.decodeIfPresent([String].self, forKey: .access)) ?? nil) ?? []
        access = rawAccess.compactMap(BrainAccess.init(rawValue:))
        more = ((try? c.decodeIfPresent(Bool.self, forKey: .more)) ?? nil) ?? false
        status = try? c.decodeIfPresent(Status.self, forKey: .status)
        systemLayerVia = ((try? c.decodeIfPresent(SystemLayer.self, forKey: .systemLayer)) ?? nil)?.via
        look = try? c.decodeIfPresent(JSONValue.self, forKey: .look)
        liveUi = try? c.decodeIfPresent(JSONValue.self, forKey: .liveUi)
        legacy = try? c.decodeIfPresent(Legacy.self, forKey: .legacy)
        docs = try? c.decodeIfPresent(String.self, forKey: .docs)
        plans = try? c.decodeIfPresent([String].self, forKey: .plans)
    }

    /// `look.default`: terminal | desktop.
    var defaultLook: String {
        if case .object(let o)? = look, case .string(let d)? = o["default"] { return d }
        return "terminal"
    }

    var hasLiveApp: Bool { liveUi != nil }

    /// "Any brain" or "Anthropic models".
    var brainsLabel: String {
        switch brains {
        case .any: return "Any brain"
        case .list(let ids):
            var seen: [String] = []
            for id in ids {
                let name = ModelProviderNames.name(id)
                if !seen.contains(name) { seen.append(name) }
            }
            return "\(seen.joined(separator: ", ")) models"
        }
    }

    var categoryLabel: String { RuntimeRegistry.categoryLabel(category) }
}

struct RuntimesBody: Decodable, Sendable {
    var version: Int
    var runtimes: [RuntimeEntry]
    /// Local gateway with owner auth only; cloud-api never sends it.
    var installed: [String: RuntimeInstallState]?

    private enum CodingKeys: String, CodingKey { case version, runtimes, installed }

    init(version: Int, runtimes: [RuntimeEntry]) {
        self.version = version
        self.runtimes = runtimes
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        version = try c.decode(Int.self, forKey: .version)
        runtimes = try c.decode([RuntimeEntry].self, forKey: .runtimes)
        installed = try? c.decodeIfPresent([String: RuntimeInstallState].self, forKey: .installed)
    }
}

// MARK: - Rules (gizzi runtime-registry.ts is the reference)

enum RuntimeRegistry {
    static func categoryLabel(_ category: String) -> String {
        switch category {
        case "native": return "Allternit native"
        case "coding": return "Coding"
        case "personal": return "Personal assistant"
        case "general": return "General agent"
        case "experimental": return "Experimental"
        default: return category.capitalized
        }
    }

    /// `tools[].aliases` from cli-catalog.json v3, used when the body has no
    /// `toolAliases` (the registry body carries `runtimes[]` only).
    static let catalogToolAliases: [String: [String]] = [
        "claude-cli": ["claude"], "codex-cli": ["codex"], "qwen-cli": ["qwen"], "kimi-cli": ["kimi"],
        "antigravity": ["agy"], "gemini-cli": ["gemini"], "copilot": ["copilot-cli"], "kilo": ["kilocode"],
        "continue-cli": ["cn"], "warp-oz": ["oz"],
    ]

    /// ACP CLI agents gizzi's local driver speaks; with codex, claude and kimi
    /// they are the legacy NATIVE_HARNESS_IDS the built-in list is made from.
    static let fallback: [RuntimeEntry] = {
        var out = [RuntimeEntry(id: "allternit", name: "Allternit", vendor: "Allternit", category: "native", tool: nil,
                                adapterKind: "native", brains: .any, access: [.plan, .key, .local, .credits])]
        out.append(RuntimeEntry(id: "codex", name: "Codex", vendor: "OpenAI", category: "coding", tool: "codex-cli",
                                aliases: ["codex-cli"], adapterKind: "codex-app-server", brains: .list(["openai", "chatgpt"]), access: [.plan, .key]))
        out.append(RuntimeEntry(id: "claude", name: "Claude Code", vendor: "Anthropic", category: "coding", tool: "claude-cli",
                                aliases: ["claude-cli"], adapterKind: "stream-json", brains: .list(["anthropic", "claude"]), access: [.plan, .key]))
        out.append(RuntimeEntry(id: "kimi", name: "Kimi Code", vendor: "Moonshot AI", category: "coding", tool: "kimi-cli",
                                aliases: ["kimi-cli"], adapterKind: "acp", brains: .any, access: [.plan, .key]))
        let acp: [(String, String)] = [
            ("grok", "Grok"), ("hermes", "Hermes"), ("kiro-cli", "Kiro"), ("mcode", "MiniMax Code"), ("qodercli", "Qoder"),
            ("qoderclicn", "Qoder CN"), ("qwenpaw", "QwenPaw"), ("reasonix", "Reasonix"), ("traecli", "Trae"),
        ]
        for (id, name) in acp {
            out.append(RuntimeEntry(id: id, name: name, vendor: name, category: id == "hermes" ? "personal" : "coding",
                                    tool: id, adapterKind: "acp", brains: .any, access: [.plan, .key], more: id == "qoderclicn"))
        }
        return out
    }()

    /// A runtime by id or alias (case-insensitive).
    static func get(_ idOrAlias: String?, in runtimes: [RuntimeEntry]) -> RuntimeEntry? {
        guard let idOrAlias, !idOrAlias.isEmpty else { return nil }
        let key = idOrAlias.lowercased()
        return runtimes.first { $0.id.lowercased() == key }
            ?? runtimes.first { ($0.aliases ?? []).contains { $0.lowercased() == key } }
    }

    /// A runtime by the old native harness id (`legacy.harness`, else its own id).
    private static func byHarness(_ harness: String, in runtimes: [RuntimeEntry]) -> RuntimeEntry? {
        runtimes.first { $0.legacy?.harness == harness } ?? get(harness, in: runtimes)
    }

    static func toolAliases(of runtime: RuntimeEntry) -> [String] {
        guard let tool = runtime.tool else { return [] }
        return runtime.toolAliases ?? catalogToolAliases[tool] ?? []
    }

    /// Does a brain from `providerId` fit this runtime? `providerId` may be a
    /// vendor id (`anthropic`), a subscription id (`subs-anthropic`, prefix
    /// stripped) or a CLI provider id (`claude-cli`): a runtime always runs
    /// its own CLI's models. Unknown runtime ids never fit.
    static func fits(_ runtime: RuntimeEntry?, providerId: String) -> Bool {
        guard let r = runtime else { return false }
        guard case .list(let ids) = r.brains else { return true }
        let p = providerId.hasPrefix("subs-") ? String(providerId.dropFirst("subs-".count)) : providerId
        if ids.contains(p) { return true }
        guard let tool = r.tool else { return false }
        if p == tool || p == r.id || (r.aliases ?? []).contains(p) { return true }
        return toolAliases(of: r).contains(p)
    }

    /// Normalizer rule (contract), on a whole `botBrain` object so every key
    /// this app doesn't model is kept. With no `runtime`, derive it:
    /// allternit_cloud → allternit; native_harness + h → the runtime whose id
    /// or legacy.harness is h (else custom); uhp_harness → the legacy.uhp
    /// match, else custom. With a `runtime`, recompute mode/harness from it:
    /// `custom` keeps the stored mode/harness, and an id this build does not
    /// know is a native harness of that id (a newer client may know more).
    /// PLAN_NAMES (ALLOWED-RULE.md).
    static let planNames: [String: String] = [
        "claude": "Claude", "chatgpt": "ChatGPT", "kimi": "Kimi", "gemini": "Gemini", "copilot": "Copilot",
    ]

    /// The built-in plans table for a server body without `plans`.
    private static let fallbackPlans: [String: [String]] = [
        "allternit": ["claude", "chatgpt", "kimi", "gemini", "copilot"],
        "claude": ["claude"], "codex": ["chatgpt"], "kimi": ["kimi"],
        "antigravity": ["gemini"], "copilot": ["copilot"],
    ]

    /// The plans a runtime may use: the registry's list, else the fallback table.
    static func plans(of runtime: RuntimeEntry) -> [String] {
        runtime.plans ?? fallbackPlans[runtime.id] ?? []
    }

    /// "Claude, ChatGPT" or nil when the runtime takes no plan.
    static func plansLabel(_ runtime: RuntimeEntry) -> String? {
        let names = plans(of: runtime).map { planNames[$0] ?? $0 }
        return names.isEmpty ? nil : names.joined(separator: ", ")
    }

    static func resolveRuntime(_ brain: [String: JSONValue], in runtimes: [RuntimeEntry]) -> [String: JSONValue] {
        var out = brain
        let mode = brain["mode"]?.jString
        if let runtimeId = brain["runtime"]?.jString, !runtimeId.isEmpty {
            guard let r = get(runtimeId, in: runtimes) else {
                if runtimeId == "custom" { return out }
                out["mode"] = .string("native_harness")
                out["harness"] = .string(runtimeId)
                return out
            }
            out["runtime"] = .string(r.id)
            if r.id == "allternit" {
                out.removeValue(forKey: "harness")
                out.removeValue(forKey: "uhpHarnessId")
                out["mode"] = .string("allternit_cloud")
            } else if r.adapter.kind == "unsupported" {
                out["mode"] = .string("uhp_harness")
                out["uhpHarnessId"] = .string(r.legacy?.uhp?.first ?? r.id)
            } else {
                out["mode"] = .string("native_harness")
                out["harness"] = .string(r.legacy?.harness ?? r.id)
            }
            return out
        }
        if mode == "native_harness", let h = brain["harness"]?.jString, !h.isEmpty {
            out["runtime"] = .string(byHarness(h, in: runtimes)?.id ?? "custom")
        } else if mode == "uhp_harness" {
            let u = brain["uhpHarnessId"]?.jString ?? ""
            let r = u.isEmpty ? nil : runtimes.first { $0.legacy?.uhp?.contains(u) == true }
            out["runtime"] = .string(r?.id ?? "custom")
        } else {
            out["runtime"] = .string("allternit")
        }
        return out
    }

    /// The one-line note after an automatic brain switch (web `switchNote`).
    static func switchNote(runtime: RuntimeEntry, provider: String, brainName: String) -> String {
        "\(runtime.name) runs \(ModelProviderNames.name(provider)) models only, so the brain changed to \(brainName)."
    }

    /// How a brain from this provider is paid for (web `accessOfProvider`),
    /// from the provider id alone: never invented per account.
    static func accessOf(providerId pid: String) -> BrainAccess {
        if pid == "allternit" { return .credits }
        if ["ollama", "omlx", "allternit-sidecar", "local-brain", "lmstudio", "local"].contains(pid) { return .local }
        if pid.hasPrefix("subs-") || pid.hasSuffix("-cli") || pid.hasPrefix("plan-") || pid == "claude-org" { return .plan }
        return .key
    }

    /// "on your plan" / "with your API key" (web `accessPhrase`).
    static func accessPhrase(_ access: BrainAccess?, providerId: String?) -> String {
        switch access {
        case .plan:
            guard let pid = providerId else { return "on your plan" }
            if pid == "claude-org" { return "billed to your Claude organization" }
            return "with your \(ModelProviderNames.name(pid)) sign-in"
        case .key: return "with your API key"
        case .local: return "on your computer"
        case .credits: return "with Allternit credits"
        case nil: return "with its own default model"
        }
    }
}

// MARK: - Store (fetch, ETag, disk cache)

enum RuntimesStatus: Equatable, Sendable {
    /// No copy yet; the first fetch is running.
    case loading
    /// The latest copy (fetched now or revalidated with a 304).
    case ready
    /// No copy at all and the fetch failed: the built-in list is shown.
    case error(String)
    /// A cached copy is shown because the fetch failed (offline).
    case stale(String)
}

@MainActor
final class RuntimeRegistryStore: ObservableObject {
    static let shared = RuntimeRegistryStore()

    @Published private(set) var body: RuntimesBody?
    @Published private(set) var status: RuntimesStatus = .loading
    private var etag: String?
    private var fetchTask: Task<Void, Never>?
    private var lastFetch: Date?

    /// The registry, or the built-in list until one has loaded.
    var runtimes: [RuntimeEntry] { body?.runtimes ?? RuntimeRegistry.fallback }
    var installed: [String: RuntimeInstallState]? { body?.installed }
    var isLoaded: Bool { body != nil }

    /// True when the list shown is not the latest (built-in or offline copy).
    var hasIssue: Bool {
        switch status {
        case .error, .stale: return true
        default: return false
        }
    }

    init() {
        if let cached = Self.readCache() {
            body = cached.body
            etag = cached.etag
            status = .ready
        }
    }

    func runtime(_ id: String?) -> RuntimeEntry? { RuntimeRegistry.get(id, in: runtimes) }

    /// Revalidates at most once an hour (the route's max-age) unless forced.
    func refreshIfNeeded(force: Bool = false) {
        if fetchTask != nil { return }
        if !force, let lastFetch, Date().timeIntervalSince(lastFetch) < 3600, status == .ready { return }
        if body == nil { status = .loading }
        fetchTask = Task { [weak self] in
            await self?.fetch()
            self?.fetchTask = nil
        }
    }

    private func fetch() async {
        let url = AppConfig.cloudAPIBaseURL.appendingPathComponent("api/v1/runtimes")
        var request = URLRequest(url: url)
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.timeoutInterval = 15
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        if body != nil, let etag { request.setValue(etag, forHTTPHeaderField: "If-None-Match") }
        do {
            let (data, response) = try await APIClient.shared.session.data(for: request)
            guard let http = response as? HTTPURLResponse else { throw URLError(.badServerResponse) }
            if http.statusCode == 304, body != nil {
                lastFetch = Date()
                status = .ready
                return
            }
            guard (200...299).contains(http.statusCode) else { throw APIError.httpError(statusCode: http.statusCode, message: nil) }
            let decoded = try JSONDecoder().decode(RuntimesBody.self, from: data)
            body = decoded
            etag = http.value(forHTTPHeaderField: "ETag")
            lastFetch = Date()
            status = .ready
            Self.writeCache(data: data, etag: etag)
        } catch {
            let message = Self.describe(error)
            status = body == nil ? .error(message) : .stale(message)
        }
    }

    private static func describe(_ error: Error) -> String {
        if error is URLError { return "You're offline." }
        if error is DecodingError { return "The runtime list didn't read." }
        return "The runtime list didn't load."
    }

    // MARK: Disk cache (Application Support/Allternit/runtimes-v1.json)

    private struct CacheFile: Codable {
        var etag: String?
        var savedAt: Date
        var body: Data
    }

    private static var cacheURL: URL? {
        guard let support = try? FileManager.default.url(for: .applicationSupportDirectory, in: .userDomainMask,
                                                         appropriateFor: nil, create: true) else { return nil }
        let dir = support.appendingPathComponent("Allternit", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.appendingPathComponent("runtimes-v1.json")
    }

    private static func readCache() -> (body: RuntimesBody, etag: String?)? {
        guard let url = cacheURL, let data = try? Data(contentsOf: url),
              let file = try? JSONDecoder().decode(CacheFile.self, from: data),
              var body = try? JSONDecoder().decode(RuntimesBody.self, from: file.body) else { return nil }
        body.installed = nil
        return (body, file.etag)
    }

    private static func writeCache(data: Data, etag: String?) {
        guard let url = cacheURL,
              let encoded = try? JSONEncoder().encode(CacheFile(etag: etag, savedAt: Date(), body: data)) else { return }
        try? encoded.write(to: url, options: .atomic)
    }
}
