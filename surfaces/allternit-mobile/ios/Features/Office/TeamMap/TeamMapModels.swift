import Foundation

/// The `team.map` flag (contract §6, ai `src/lib/feature-flags.ts`): default
/// OFF until the train proves it. Off = the Office floor, project page and
/// approvals screen behave exactly as before. Turn it on for one device with
/// `defaults write com.allternit.app allternit.flag.team.map -bool YES`, or
/// for a simulator run with the launch argument `-allternit.flag.team.map YES`
/// (launch arguments land in UserDefaults' argument domain).
enum TeamMapFlag {
    static let key = "allternit.flag.team.map"
    static var isOn: Bool { UserDefaults.standard.bool(forKey: key) }
}

// MARK: - Wire shapes (contract §2; nullable fields are always present)

struct TeamAppRef: Decodable, Equatable, Hashable, Sendable {
    let kind: String
    let ref: String
    let name: String?
}

struct TeamNode: Decodable, Identifiable, Equatable, Sendable {
    struct Counts: Decodable, Equatable, Sendable {
        let asks: Int
        let jobs: Int
    }

    /// `p:<user_id>` people, `b:<agent_id>` agent bots, `v:<vendor_bot_id>` vendor bots.
    let id: String
    let kind: String
    let name: String
    let role: String?
    let isMain: Bool?
    /// The bot's model ("Claude Sonnet"), when the server sends one (web `node.brain`).
    let brain: String?
    /// idle, working, needs_input, waiting_approval, offline.
    let state: String
    let counts: Counts

    var isBot: Bool { kind == "bot" }

    enum CodingKeys: String, CodingKey {
        case id, kind, name, role, brain, state, counts
        case isMain = "is_main"
    }

    init(id: String, kind: String, name: String, role: String?, isMain: Bool?, brain: String?, state: String, counts: Counts) {
        self.id = id
        self.kind = kind
        self.name = name
        self.role = role
        self.isMain = isMain
        self.brain = brain
        self.state = state
        self.counts = counts
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        kind = try c.decode(String.self, forKey: .kind)
        name = try c.decode(String.self, forKey: .name)
        role = try c.decodeIfPresent(String.self, forKey: .role)
        isMain = try c.decodeIfPresent(Bool.self, forKey: .isMain)
        brain = try c.decodeIfPresent(String.self, forKey: .brain)
        state = (try c.decodeIfPresent(String.self, forKey: .state)) ?? "idle"
        counts = (try c.decodeIfPresent(Counts.self, forKey: .counts)) ?? Counts(asks: 0, jobs: 0)
    }

    /// The second line (web `subtitle`): a person's role, a bot's kind and brain.
    var subtitle: String {
        if !isBot {
            guard let role, !role.isEmpty else { return "Member" }
            return role.prefix(1).uppercased() + role.dropFirst()
        }
        let kindText = isMain == true ? "Main bot" : (id.hasPrefix("v:") ? "Connected bot" : "Bot")
        if let brain, !brain.isEmpty { return "\(kindText) · \(brain)" }
        return kindText
    }

    var initials: String {
        let letters = name.split(separator: " ").prefix(2).compactMap(\.first)
        let text = String(letters).uppercased()
        return text.isEmpty ? "?" : text
    }

    /// `usr_x` for `p:usr_x`, nil for a bot.
    var userId: String? { id.hasPrefix("p:") ? String(id.dropFirst(2)) : nil }
}

struct TeamEdge: Decodable, Equatable, Sendable {
    let from: String
    let to: String
    /// reports_to (person → manager), owns (person → bot), parent (bot → bot).
    let kind: String
}

struct TeamGrant: Decodable, Identifiable, Equatable, Sendable {
    let id: String
    let bot: String
    let app: TeamAppRef
    /// browser, app, full.
    let level: String
    /// Null when the app is not installed for the owner.
    let ceiling: String?
}

/// The short form the map and `GET /team/rules` return:
/// `{id, text, enabled, scope, approver, locked}`.
struct TeamRule: Decodable, Identifiable, Equatable, Sendable {
    struct Scope: Decodable, Equatable, Sendable {
        /// org, person, bot.
        let kind: String
        /// person: user id; bot: `b:<id>` / `v:<id>`; org: nil.
        let ref: String?
    }

    struct Approver: Decodable, Equatable, Sendable {
        /// owner, manager, user.
        let kind: String
        let userId: String?

        enum CodingKeys: String, CodingKey {
            case kind
            case userId = "user_id"
        }
    }

    let id: String
    /// Server-built rule sentence (contract §4). Shown as sent, never reworded.
    let text: String
    let enabled: Bool
    let scope: Scope
    let approver: Approver?
    /// A built-in rule: cannot be turned off or deleted ("Always on").
    let locked: Bool

    enum CodingKeys: String, CodingKey {
        case id, text, enabled, scope, approver, locked
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        text = try c.decode(String.self, forKey: .text)
        enabled = (try c.decodeIfPresent(Bool.self, forKey: .enabled)) ?? true
        scope = (try c.decodeIfPresent(Scope.self, forKey: .scope)) ?? Scope(kind: "org", ref: nil)
        approver = try c.decodeIfPresent(Approver.self, forKey: .approver)
        locked = (try c.decodeIfPresent(Bool.self, forKey: .locked)) ?? false
    }

    /// The scope as a node id: "org", `p:<uid>`, or the bot id.
    var scopeKey: String {
        switch scope.kind {
        case "person": return scope.ref.map { "p:\($0)" } ?? "org"
        case "bot": return scope.ref ?? "org"
        default: return "org"
        }
    }
}

struct TeamAsk: Decodable, Identifiable, Equatable, Sendable {
    /// Which call decides it. Nil for platform asks (shown read-only).
    struct Resolve: Decodable, Equatable, Sendable {
        let method: String
        let path: String
        let body: JSONValue?
    }

    let id: String
    let store: String
    let title: String
    let summary: String?
    let actor: String?
    let approverUserId: String?
    let ruleId: String?
    let app: TeamAppRef?
    let tool: String?
    let risk: String?
    let state: String
    let createdAt: String?
    let jobId: String?
    let resolve: Resolve?

    enum CodingKeys: String, CodingKey {
        case id, store, title, summary, actor, app, tool, risk, state, resolve
        case approverUserId = "approver_user_id"
        case ruleId = "rule_id"
        case createdAt = "created_at"
        case jobId = "job_id"
    }
}

/// `TeamMap.jobs[]`: a bar job, with the node it waits on.
struct TeamMapJob: Decodable, Identifiable, Equatable, Sendable {
    let jobId: String
    let actor: String?
    let waitingOn: String?
    let app: TeamAppRef?
    let tool: String?
    let title: String?
    let state: String

    var id: String { jobId }

    enum CodingKeys: String, CodingKey {
        case actor, app, tool, title, state
        case jobId = "job_id"
        case waitingOn = "waiting_on"
    }
}

struct TeamMap: Decodable, Equatable, Sendable {
    struct Org: Decodable, Equatable, Sendable {
        let id: String
        let name: String
    }

    let org: Org?
    /// `p:<user_id>` of the viewer.
    let me: String
    var nodes: [TeamNode]
    var edges: [TeamEdge]
    var grants: [TeamGrant]
    var rules: [TeamRule]
    var asks: [TeamAsk]
    var jobs: [TeamMapJob]
    let partial: [String]

    enum CodingKeys: String, CodingKey {
        case org, me, nodes, edges, grants, rules, asks, jobs, partial
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        org = try c.decodeIfPresent(Org.self, forKey: .org)
        me = try c.decode(String.self, forKey: .me)
        nodes = try c.decodeIfPresent([TeamNode].self, forKey: .nodes) ?? []
        edges = try c.decodeIfPresent([TeamEdge].self, forKey: .edges) ?? []
        grants = try c.decodeIfPresent([TeamGrant].self, forKey: .grants) ?? []
        rules = try c.decodeIfPresent([TeamRule].self, forKey: .rules) ?? []
        asks = try c.decodeIfPresent([TeamAsk].self, forKey: .asks) ?? []
        jobs = try c.decodeIfPresent([TeamMapJob].self, forKey: .jobs) ?? []
        partial = try c.decodeIfPresent([String].self, forKey: .partial) ?? []
    }

    // MARK: Lookups

    func node(_ id: String?) -> TeamNode? {
        guard let id else { return nil }
        return nodes.first { $0.id == id }
    }

    func name(_ id: String?) -> String? { node(id)?.name }

    var people: [TeamNode] { nodes.filter { !$0.isBot } }
    var bots: [TeamNode] { nodes.filter(\.isBot) }

    /// The person who owns a bot (`owns` edge).
    func owner(of botId: String) -> String? {
        edges.first { $0.kind == "owns" && $0.to == botId }?.from
    }

    func grants(for botId: String) -> [TeamGrant] {
        grants.filter { $0.bot == botId }
    }

    func rule(_ id: String?) -> TeamRule? {
        guard let id else { return nil }
        return rules.first { $0.id == id }
    }

    /// The viewer's own user id (`me` without the `p:`).
    var myUserId: String { me.hasPrefix("p:") ? String(me.dropFirst(2)) : me }

    /// Who asked: the bot's owner for a bot actor, else the person.
    func requesterName(_ ask: TeamAsk) -> String? {
        guard let actor = ask.actor else { return nil }
        if actor.hasPrefix("p:") { return name(actor) }
        return name(owner(of: actor))
    }

    func approverName(_ ask: TeamAsk) -> String? {
        guard let user = ask.approverUserId else { return nil }
        return name("p:\(user)")
    }

    /// "Stripe at App": the acting bot's level on the ask's app.
    func levelLine(_ ask: TeamAsk) -> String? {
        guard let actor = ask.actor, let app = ask.app,
              let grant = grants.first(where: { $0.bot == actor && $0.app.kind == app.kind && $0.app.ref == app.ref })
        else { return nil }
        return "\(grant.app.displayName) at \(TeamLevel.label(grant.level))"
    }
}

extension TeamAppRef {
    var displayName: String { (name?.isEmpty == false ? name : nil) ?? ref }
}

/// Level and state wording (contract §5). Never shows a raw code.
enum TeamLevel {
    static func label(_ level: String) -> String {
        switch level {
        case "browser": return "Browser"
        case "app": return "App"
        case "full": return "Full"
        default: return level.capitalized
        }
    }
}

enum TeamNodeState {
    static func label(_ state: String) -> String {
        switch state {
        case "working": return "Working"
        case "needs_input": return "Needs input"
        case "waiting_approval": return "Waiting on approval"
        case "offline": return "Offline"
        default: return "Idle"
        }
    }
}

/// The partial-list note (contract §8).
enum TeamPartial {
    static func note(_ partial: [String]) -> String? {
        partial.isEmpty ? nil : "Some approvals or apps may be missing."
    }
}

// MARK: - Map helpers (web team-map/model.ts, same rules)

extension TeamMap {
    var viewer: TeamNode? { node(me) }
    var viewerRole: String? { viewer?.role }
    /// Org owner or admin: may edit rules and anyone's reporting line.
    var viewerIsAdmin: Bool { viewerRole == "owner" || viewerRole == "admin" }

    /// The tree parent: a person's manager, a bot's parent bot or owner.
    func parent(of id: String) -> String? {
        guard let n = node(id) else { return nil }
        if !n.isBot {
            return edges.first { $0.kind == "reports_to" && $0.from == id }?.to
        }
        return edges.first { $0.kind == "parent" && $0.to == id }?.from
            ?? edges.first { $0.kind == "owns" && $0.to == id }?.from
    }

    /// Sort: the viewer, people, the main bot, other bots; then by name.
    private func rank(_ n: TeamNode) -> Int {
        if n.id == me { return 0 }
        if !n.isBot { return 1 }
        return n.isMain == true ? 2 : 3
    }

    func ordered(_ list: [TeamNode]) -> [TeamNode] {
        list.sorted { a, b in
            let ra = rank(a), rb = rank(b)
            return ra != rb ? ra < rb : a.name.localizedCaseInsensitiveCompare(b.name) == .orderedAscending
        }
    }

    /// Roots of the tree (no parent on the map), cycles broken by treating
    /// a node already seen as a root.
    var roots: [TeamNode] {
        ordered(nodes.filter { n in
            guard let p = parent(of: n.id), node(p) != nil else { return true }
            return createsCycle(from: n.id)
        })
    }

    private func createsCycle(from id: String) -> Bool {
        var seen: Set<String> = [id]
        var cursor = parent(of: id)
        while let c = cursor, node(c) != nil {
            if seen.contains(c) { return c == id }
            seen.insert(c)
            cursor = parent(of: c)
        }
        return false
    }

    func children(of id: String) -> [TeamNode] {
        ordered(nodes.filter { $0.id != id && parent(of: $0.id) == id && !createsCycle(from: $0.id) })
    }

    /// Who a bot waits on (bar job first, else its pending ask's approver).
    func waitingOn(_ botId: String) -> String? {
        if let job = jobs.first(where: { $0.actor == botId && $0.waitingOn != nil && $0.state == "waiting_approval" }) {
            return job.waitingOn
        }
        guard node(botId)?.state == "waiting_approval",
              let user = asks.first(where: { $0.actor == botId })?.approverUserId else { return nil }
        return "p:\(user)"
    }

    /// Bots waiting on this node.
    func waitingOnMe(_ id: String) -> [TeamNode] {
        bots.filter { waitingOn($0.id) == id }
    }

    /// State wording (contract §5); a bot waiting on a named person says who.
    func stateText(_ n: TeamNode) -> String {
        if let who = waitingOn(n.id) {
            return "Waiting on \(name(who) ?? "approval")"
        }
        return TeamNodeState.label(n.state)
    }

    /// Asks this node raised (bot) or must answer (person).
    func asks(for n: TeamNode) -> [TeamAsk] {
        let uid = n.userId
        return asks.filter { $0.actor == n.id || (uid != nil && $0.approverUserId == uid) }
    }

    /// Rules about this node: scoped to it, naming it as approver, or behind one of its asks.
    func rules(for n: TeamNode) -> [TeamRule] {
        let uid = n.userId
        let askRules = Set(asks(for: n).compactMap(\.ruleId))
        return rules.filter { r in
            askRules.contains(r.id)
                || (r.scope.kind == "bot" && r.scope.ref == n.id)
                || (r.scope.kind == "person" && uid != nil && r.scope.ref == uid)
                || (uid != nil && r.approver?.userId == uid)
        }
    }

    /// Rules in force for a scope ("org" or a node id): org-wide plus its own.
    func rules(inScope key: String) -> [TeamRule] {
        rules.filter { $0.scope.kind == "org" || $0.scopeKey == key }
    }

    /// The viewer may try to decide an ask (approver, or org owner/admin). The server has the last word.
    func canDecide(_ ask: TeamAsk) -> Bool {
        guard ask.resolve != nil else { return false }
        if let user = ask.approverUserId, me == "p:\(user)" { return true }
        return viewerIsAdmin
    }

    /// The viewer may change this bot's levels: its owner, or an org owner/admin.
    func canEditLevels(of botId: String) -> Bool {
        viewerIsAdmin || owner(of: botId) == me
    }

    /// Apps seen on the map, for the rule editor's app blank.
    var apps: [TeamAppRef] {
        var seen: [String: TeamAppRef] = [:]
        var order: [String] = []
        for g in grants {
            let key = "\(g.app.kind):\(g.app.ref)"
            if seen[key] == nil { order.append(key) }
            seen[key] = g.app
        }
        return order.compactMap { seen[$0] }
    }

    /// The currency mark used by the asks ("€"), for the rule editor's "Try it" line.
    var currencyMark: String {
        for a in asks {
            let text = "\(a.title) \(a.summary ?? "")"
            if let mark = text.first(where: { "€$£¥".contains($0) }) { return String(mark) }
        }
        return ""
    }

    var peopleCount: Int { people.count }
    var botCount: Int { bots.count }
    var waitingCount: Int { asks.count }
}

extension TeamGrant {
    /// "agent" for `b:`, "vendor" for `v:` (PUT /team/grants `bot_kind`).
    var botKind: String { bot.hasPrefix("v:") ? "vendor" : "agent" }
    var botId: String { String(bot.dropFirst(2)) }
}

enum TeamPlural {
    static func count(_ n: Int, _ one: String, _ many: String? = nil) -> String {
        "\(n) \(n == 1 ? one : (many ?? one + "s"))"
    }
}
