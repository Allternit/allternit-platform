import Foundation

/// Connected view — what is ACTUALLY connected to this account/device
/// (allternit-ai `setup/connectedInventory.ts` + `inventorySources.ts`).
///
/// `load` reads the real sources (each independently; one failing source
/// never blanks the others) and `buildGraph` turns the result plus this
/// setup session into graph nodes. Nothing here invents a connection: a node
/// is "conn" only when its source says so, "new" when this session set it
/// up, and "avail" otherwise (with the reason in its detail).
enum ConnectedInventory {
    enum SourceState: Sendable { case ok, error, unavailable }

    struct Source<T: Sendable>: Sendable {
        var state: SourceState
        var items: [T]
        var note: String?
    }

    struct Runtime: Sendable { let id: String; let name: String; let status: String; let host: String? }
    struct SubLogin: Sendable { let provider: String; let ready: Bool }
    struct Named: Sendable { let id: String; let name: String; let connected: Bool; var detail: String?; var isMain = false }

    struct Inventory: Sendable {
        var providers: Source<SetupProviderAuth>
        var cloudModels: Source<SetupModelRow>
        var runtimes: Source<Runtime>
        var cloudComputers: Source<SetupCloudComputer>
        var bots: Source<Named>
        var channels: Source<Named>
        var memory: Source<Named>
        var projects: Source<Named>
        var connectors: Source<Named>
        var subscriptions: Source<SubLogin>
    }

    static let runtimeNote = "Needs a connected computer or Allternit Cloud computer"

    private static func note(for error: Error) -> String {
        let msg: String = {
            if let e = error as? ConnectError { return "\(e.status.map(String.init) ?? "") \(e.code ?? "") \(e.message)" }
            return error.localizedDescription
        }()
        if msg.range(of: "runtime_unavailable|No paired Allternit runtime|503", options: [.regularExpression, .caseInsensitive]) != nil { return runtimeNote }
        if msg.range(of: "401|403|unauthor|Sign in again", options: [.regularExpression, .caseInsensitive]) != nil { return "Sign in to see this" }
        return "Couldn't load right now"
    }

    private static func settle<T: Sendable, R: Sendable>(
        runtime: Bool, reachable: Bool,
        _ fn: @Sendable () async throws -> T, _ map: (T) -> [R]
    ) async -> Source<R> {
        if runtime && !reachable { return Source(state: .unavailable, items: [], note: runtimeNote) }
        do {
            return Source(state: .ok, items: map(try await fn()))
        } catch {
            let n = note(for: error)
            return Source(state: n == runtimeNote ? .unavailable : .error, items: [], note: n)
        }
    }

    private static let connectedChannelStates: Set<String> = ["CONNECTED", "DEGRADED"]
    private static let channelNames: [String: String] = [
        "slack": "Slack", "teams": "Microsoft Teams", "discord": "Discord", "telegram": "Telegram",
        "whatsapp": "WhatsApp", "whatsapp-personal": "WhatsApp", "email": "Email", "sms": "SMS",
    ]
    private static func channelName(_ vendor: String) -> String {
        channelNames[vendor] ?? vendor.prefix(1).uppercased() + vendor.dropFirst()
    }

    /// Runtime-backed sources need a reachable computer (one online); cloud ones don't.
    static func load(runtimes: [RelayRuntimeDevice], reachable: Bool) async -> Inventory {
        let rts = runtimes.map { Runtime(id: $0.id, name: $0.displayName, status: $0.status, host: $0.hostname) }
        async let providers = settle(runtime: true, reachable: reachable, { try await SetupAPI.providerAuthStatus() }) { $0 }
        async let models = settle(runtime: false, reachable: true, { await SetupAPI.cloudModelCatalog() }) { $0 }
        async let clouds = settle(runtime: false, reachable: true, { try await SetupAPI.cloudComputers() }) { $0 }
        async let bots = settle(runtime: true, reachable: reachable, { try await MeetAgentsAPI.roster() }) { (rows: [MeetRosterBot]) -> [Named] in
            let main = rows.first(where: \.isMain)?.id
            return rows.map { Named(id: $0.id, name: $0.displayName, connected: true, isMain: $0.id == main) }
        }
        async let channels = settle(runtime: true, reachable: reachable, { try await SetupAPI.channelAccounts() }) { rows in
            rows.filter { $0.authType == "channel_oauth" }.map {
                Named(id: $0.id, name: channelName($0.vendor), connected: connectedChannelStates.contains($0.state), detail: $0.displayName)
            }
        }
        async let memory = settle(runtime: true, reachable: reachable, { try await MemoryDriveAPI.mounts() }) { rows in
            rows.map { Named(id: $0.ref, name: $0.name, connected: $0.revision != nil) }
        }
        async let projects = settle(runtime: true, reachable: reachable, { try await SetupAPI.botProjects() }) { rows in
            rows.map { Named(id: $0.id, name: $0.name, connected: true) }
        }
        async let connectors = settle(runtime: true, reachable: reachable, { try await SetupAPI.mcpConnectors() }) { rows in
            rows.map { Named(id: $0.id, name: $0.name, connected: $0.connected) }
        }
        async let subs = settle(runtime: true, reachable: reachable, { try await SetupAPI.fabricAccounts() }) { rows in
            rows.map { SubLogin(provider: $0.provider, ready: $0.isReady) }
        }
        return await Inventory(
            providers: providers, cloudModels: models,
            runtimes: Source(state: .ok, items: rts),
            cloudComputers: clouds, bots: bots, channels: channels, memory: memory,
            projects: projects, connectors: connectors, subscriptions: subs)
    }

    // MARK: - Graph

    enum NodeStatus: Sendable { case new, conn, inc, avail }
    enum NodeGroup: String, CaseIterable, Sendable { case brains = "Brains", computers = "Computers", bots = "Bots", channels = "Channels", workspace = "Workspace" }

    struct Node: Identifiable, Equatable, Sendable {
        let id: String
        let group: NodeGroup
        let title: String
        /// Short title for the phone graph.
        let short: String
        /// Second line under brain nodes.
        var sub: String?
        let desc: String
        let status: NodeStatus
        /// Status sentence for the detail card.
        let statusText: String
        let brain: Bool
    }

    /// What this setup session did (from the setup state).
    struct SessionFacts: Equatable, Sendable {
        var where_: WhereChoice?
        var wherePaired: Bool
        var whereLabel: String?
        var brain: BrainChoice?
        var brainConnected: Bool
        var brainLabel: String?
        var mainBotId: String?

        init(_ s: SetupState) {
            where_ = s.choices.where
            wherePaired = s.outcomes[.where]?.status == .paired
            whereLabel = s.outcomes[.where]?.label
            brain = s.choices.ai
            brainConnected = s.outcomes[.ai]?.status == .connected
            brainLabel = s.outcomes[.ai]?.label
            mainBotId = s.mainBotId
        }
    }

    private static func statusText(_ s: NodeStatus) -> String {
        switch s {
        case .new: return "Connected · set up just now"
        case .conn: return "Connected"
        case .inc: return "Included in your workspace"
        case .avail: return "Available · add it any time"
        }
    }

    private static func node(_ id: String, _ group: NodeGroup, _ title: String, short: String? = nil, sub: String? = nil,
                             desc: String, status: NodeStatus, brain: Bool = false, statusText text: String? = nil) -> Node {
        Node(id: id, group: group, title: title, short: short ?? title, sub: sub, desc: desc, status: status,
             statusText: text ?? statusText(status), brain: brain)
    }

    /// Max nodes per side group so the graph stays legible.
    static let groupCap = 4

    private static func capped(_ nodes: [Node], _ group: NodeGroup, _ moreDesc: String, cap: Int = groupCap) -> [Node] {
        guard nodes.count > cap else { return nodes }
        let shown = Array(nodes.prefix(cap - 1))
        let rest = nodes.count - shown.count
        return shown + [node("\(group.rawValue)-more", group, "+\(rest) more", desc: moreDesc, status: .conn, statusText: "\(rest) more connected")]
    }

    private static let providersFor: [BrainChoice: [String]] = [
        .claude: ["anthropic", "claude-cli"], .chatgpt: ["openai", "codex-cli"],
        .gemini: ["google", "antigravity"], .local: ["ollama", "lmstudio", "sidecar"],
    ]

    /// The graph on the phone (`onPhone` facts: this device is a phone).
    static func buildGraph(_ inv: Inventory, _ s: SessionFacts) -> [Node] {
        var out: [Node] = []
        let authed = Set(inv.providers.items.filter { $0.authenticated }.map { $0.id })

        // Brains — the five rows from setup, each with its real status.
        let rows: [(BrainChoice, String, String)] = [
            (.allternit, "Hosted models", inv.cloudModels.items.isEmpty
                ? "Allternit Cloud models, billed to your Allternit plan."
                : "Allternit Cloud models (\(inv.cloudModels.items.count) in the catalog right now), billed to your Allternit plan."),
            (.claude, "Your account", "Your own Anthropic account — an API key or your Claude subscription."),
            (.chatgpt, "Your account", "Your own OpenAI account — an API key or your ChatGPT sign-in."),
            (.gemini, "Your account", "Your own Google account — an API key or a Google sign-in."),
            (.local, "On your computer", "A model that runs on your computer. No network needed once downloaded."),
        ]
        for (id, sub, desc) in rows {
            let isSession = s.brainConnected && s.brain == id
            let before = id == .allternit ? false : (providersFor[id] ?? []).contains(where: authed.contains)
            let title = id == .allternit ? "Allternit Cloud" : id.label
            let subLine = isSession && id == .allternit ? s.brainLabel?.replacingOccurrences(of: "Allternit Cloud · ", with: "") ?? sub : sub
            out.append(node("brain-\(id.rawValue)", .brains, title, short: id == .local ? "Local" : title, sub: subLine, desc: desc,
                            status: isSession ? .new : before ? .conn : .avail, brain: true,
                            statusText: isSession ? "Default brain · set up just now" : nil))
        }
        // Subscription brains — only those with a login on the Sessions computer, or set up now.
        for sb in BrainChoice.subscriptions {
            let provider = sb.subscriptionProvider ?? ""
            let logins = inv.subscriptions.items.filter { $0.provider == provider }
            let isSession = s.brainConnected && s.brain == sb
            guard !logins.isEmpty || isSession else { continue }
            let ready = logins.contains { $0.ready }
            out.append(node("brain-\(sb.rawValue)", .brains, "\(sb.shortName) sub", sub: "Your subscription",
                            desc: "Your \(sb.shortName) subscription, signed in on your Sessions computer.",
                            status: isSession ? .new : ready ? .conn : .avail, brain: true,
                            statusText: isSession ? "Default brain · set up just now" : ready ? "Connected" : "Signed out · sign in again in Settings → Connections"))
        }
        // A brain connected this session that isn't one of the five rows.
        if s.brainConnected, s.brain == .other, let label = s.brainLabel {
            out.append(node("brain-other", .brains, label, sub: "Your account", desc: "Connected during setup.", status: .new, brain: true,
                            statusText: "Default brain · set up just now"))
        }

        // Computers — real runtimes + cloud computers, then the open placeholders.
        var comps: [Node] = []
        let sessionComputer = s.wherePaired ? s.where_ : nil
        for r in inv.runtimes.items {
            let isNew = sessionComputer == .mac && s.whereLabel == r.name
            comps.append(node("rt-\(r.id)", .computers, r.name, desc: r.host.map { "\($0)." } ?? "A computer paired to your account.",
                              status: isNew ? .new : r.status == "offline" ? .avail : .conn,
                              statusText: isNew ? nil : r.status == "offline" ? "Paired · offline right now" : r.status == "busy" ? "Connected · busy" : nil))
        }
        for cc in inv.cloudComputers.items {
            let spec = cc.specs.map { " \($0)." } ?? ""
            comps.append(node("cc-\(cc.id)", .computers, "Allternit Cloud computer", short: "Cloud",
                              desc: "An always-on computer in Allternit Cloud.\(spec)",
                              status: sessionComputer == .cloud ? .new : cc.isRunning ? .conn : .avail,
                              statusText: sessionComputer == .cloud ? nil : cc.isRunning ? "Connected" : "Status: \(cc.status)"))
        }
        if inv.cloudComputers.items.isEmpty {
            comps.append(node("comp-cloud", .computers, "Allternit Cloud computer", short: "Cloud",
                              desc: "An always-on computer in Allternit Cloud. Comes with a paid plan.", status: .avail,
                              statusText: s.where_ == .cloud ? "Chosen · finish in Settings → Billing" : nil))
        }
        if sessionComputer == .server {
            comps.append(node("comp-server", .computers, s.whereLabel ?? "Your server", short: "Server",
                              desc: "A machine you own, connected during setup.", status: .new))
        } else if comps.count < groupCap {
            comps.append(node("comp-server", .computers, "Your server", short: "Server",
                              desc: "A machine you own, paired over SSH or a backend URL.", status: .avail))
        }
        out.append(contentsOf: capped(comps, .computers, "More computers are paired to this account. See Settings → Computers.", cap: groupCap - 1))
        out.append(node("phone", .computers, "This phone", short: "Phone",
                        desc: "You are setting up from this phone. Chats, bots and approvals work here.", status: .conn))

        // Bots — the main bot first (its own node, always present), then the rest.
        if let main = inv.bots.items.first(where: \.isMain) {
            let isNew = s.mainBotId == main.id
            out.append(node("bot-main", .bots, main.name, desc: "Your main bot — the one pinned at the top of Bots that answers first.",
                            status: isNew ? .new : .conn, statusText: isNew ? "Main bot · set up just now" : "Main bot"))
        } else {
            let ok = inv.bots.state == .ok
            out.append(node("bot-main", .bots, "Main bot", desc: ok ? "No main bot yet. Pick or set one up in Bots." : inv.bots.note ?? "Pick or set one up in Bots.",
                            status: .avail, statusText: ok ? nil : inv.bots.note))
        }
        let bots = inv.bots.items.filter { !$0.isMain }.map {
            node("bot-\($0.id)", .bots, $0.name, desc: "A bot on your account.", status: .conn)
        }
        out.append(contentsOf: capped(bots, .bots, "More bots on your account. See Bots.", cap: groupCap - 1))

        // Channels — real accounts, else the channels you can connect.
        var channels = inv.channels.items.map { c in
            node("ch-\(c.id)", .channels, c.name, desc: c.detail.map { "\(c.name) · \($0)" } ?? "Talk to your bots from \(c.name).",
                 status: c.connected ? .conn : .avail, statusText: c.connected ? nil : "Needs attention in Settings → Connections")
        }
        let names = Set(channels.map(\.title))
        for name in ["Slack", "Email", "SMS", "Telegram"] where channels.count < groupCap && !names.contains(name) {
            channels.append(node("ch-\(name.lowercased())", .channels, name,
                                 desc: inv.channels.state == .ok ? "Talk to your bots from \(name)." : inv.channels.note ?? "Talk to your bots from \(name).",
                                 status: .avail))
        }
        out.append(contentsOf: capped(channels, .channels, "More channels are connected. See Settings → Connections."))

        // Workspace
        let memCreated = inv.memory.items.contains(where: \.connected)
        out.append(node("memory", .workspace, "Memory Drive", desc: "What Gizzi remembers about your work. You can see and edit it.",
                        status: memCreated ? .conn : inv.memory.state == .ok ? .inc : .avail,
                        statusText: memCreated ? "Connected" : inv.memory.state == .ok ? "Included · created the first time you open it" : inv.memory.note))
        let projectCount = inv.projects.items.count
        out.append(node("projects", .workspace, "Projects", desc: "Chats, tasks, code and canvases grouped by project.",
                        status: projectCount > 0 ? .conn : .inc,
                        statusText: projectCount > 0 ? "\(projectCount) project\(projectCount == 1 ? "" : "s")" : inv.projects.state == .ok ? "Included · none yet" : "Included"))
        let conns = inv.connectors.items.filter(\.connected).count
        out.append(node("connectors", .workspace, "Connectors", desc: "MCP apps and connectors that let agents use your tools.",
                        status: conns > 0 ? .conn : .avail,
                        statusText: conns > 0 ? "\(conns) connected" : inv.connectors.state == .ok ? nil : inv.connectors.note))
        return out
    }
}
