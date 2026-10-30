import Foundation

/// Answers the app's API calls from `SampleWorkspace` — the port of the web's
/// src/embed/preview-api.ts plus the phone-only reads the native app makes
/// (cowork projects, cron jobs, the computer list). Reads come back in the
/// exact wire shape each Swift model decodes; unknown reads answer 404 so a
/// screen shows its own empty/error state instead of reaching the network.
/// The Delegate board's writes run on an in-memory copy (as on the web);
/// every other write answers a benign success.
enum SampleWorkspaceAPI {
    struct Answer {
        let status: Int
        let body: Data
    }

    /// Where a request path lives once the host's prefix is stripped.
    private enum Namespace {
        /// allternit-api / cloud-api `/api/v1/*`.
        case v1
        /// cloud-api `/api/v2/*` (Artifacts library).
        case v2
        /// `/api/factory/*`.
        case factory
        /// gizzi-code's own `/v1/*` (cron, automations, sessions).
        case gizzi
        case other
    }

    static func answer(method rawMethod: String, url: URL, body: Data?) -> Answer {
        let method = rawMethod.uppercased()
        let path = url.path.hasSuffix("/") && url.path.count > 1 ? String(url.path.dropLast()) : url.path

        // Cloud relay: the envelope's inner call IS the request (the relay's
        // response is the runtime's response, fetch-interceptor.ts:441-468).
        if method == "POST", path.hasPrefix("/api/v1/runtime-devices/"), path.hasSuffix("/proxy"),
           let body, let envelope = try? JSONSerialization.jsonObject(with: body) as? [String: Any],
           let innerPath = envelope["path"] as? String,
           let innerURL = URL(string: "allternit-relay://\(SampleWorkspace.runtimeId)\(innerPath.hasPrefix("/") ? "" : "/")\(innerPath)") {
            let innerBody = (envelope["body"] as? String).flatMap { $0.isEmpty ? nil : Data($0.utf8) }
            return answer(method: envelope["method"] as? String ?? "GET", url: innerURL, body: innerBody)
        }

        let (namespace, route) = split(path)
        let parts = route.split(separator: "/").map { String($0).removingPercentEncoding ?? String($0) }
        let query = Dictionary(
            (URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems ?? []).map { ($0.name, $0.value ?? "") },
            uniquingKeysWith: { first, _ in first }
        )

        if method != "GET" {
            return write(method: method, namespace: namespace, parts: parts, body: body)
        }
        guard let value = read(namespace: namespace, parts: parts, query: query) else {
            return json(["error": "not_found"], status: 404)
        }
        return json(value)
    }

    // MARK: - Reads

    private static func read(namespace: Namespace, parts: [String], query: [String: String]) -> Any? {
        let now = Date()
        switch namespace {
        case .v1:
            switch parts {
            case ["agents"]:
                let agents = SampleWorkspace.bots(now: now)
                return ["agents": agents, "total": agents.count]
            case let p where p.count == 2 && p[0] == "agents":
                return SampleWorkspace.bots(now: now).first { $0["id"] as? String == p[1] }.map { ["agent": $0] }
            case ["agent-templates"]:
                return ["templates": [Any]()]
            case ["agent-sessions"]:
                return ["sessions": [Any](), "count": 0]
            case let p where p.count == 3 && p[0] == "agent-sessions" && p[2] == "messages":
                // A chat the sample just made has no history yet.
                return [Any]()
            case ["cowork", "sessions"]:
                return ["sessions": [Any]()]
            case ["bot-projects"]:
                return ["projects": SampleWorkspace.projects(now: now)]
            case ["bot-groups"]:
                return [SampleWorkspace.launchRoom(now: now)]
            case let p where p.count == 3 && p[0] == "agent-sessions" && p[2] == "context":
                // A chat a third full: the composer meter's context reading.
                return ["breakdown": [
                    "contextWindow": 200_000, "usedTokens": 61_400, "utilization": 0.307,
                    "basis": "provider", "source": "gizzi",
                    "categories": [
                        ["id": "messages", "name": "Messages", "tokens": 38_200],
                        ["id": "system_tools", "name": "System tools", "tokens": 14_600],
                        ["id": "system_prompt", "name": "System prompt", "tokens": 3_100],
                        ["id": "memory_files", "name": "Memory files", "tokens": 5_500],
                        ["id": "free_space", "name": "Free space", "tokens": 138_600],
                    ],
                ] as [String: Any], "limits": NSNull()]
            case let p where p.count == 3 && p[0] == "projects" && p[2] == "messages":
                return SampleProjectState.shared.messages(projectId: p[1], now: now).map { ["messages": $0] }
            case let p where p.count == 3 && p[0] == "projects" && p[2] == "delegate":
                return SampleDelegateBoards.shared.board(projectId: p[1])
            case ["threads"]:
                let threads = SampleWorkspace.threads(now: now).filter { thread in
                    (query["projectId"].map { thread["projectId"] as? String == $0 } ?? true)
                        && (query["botId"].map { thread["botId"] as? String == $0 } ?? true)
                }
                return ["threads": threads]
            case let p where p.count == 2 && p[0] == "threads":
                return SampleWorkspace.threads(now: now).first { $0["id"] as? String == p[1] }.map { ["thread": $0] }
            case let p where p.count == 3 && p[0] == "threads" && p[2] == "events":
                return ["events": [Any](), "cursor": NSNull()]
            case ["artifacts"]:
                return ["artifacts": query["scope"] == "shared" ? [] : SampleWorkspace.artifacts(now: now)]
            case ["automation", "goals"]:
                return SampleWorkspace.goals(now: now)
            case ["automation", "routines"]:
                return SampleWorkspace.routines(now: now) + SampleProjectState.shared.addedRoutines()
            case ["automation", "loops"]:
                return SampleWorkspace.loops(now: now)
            case ["gateway", "execution-bindings"]:
                return ["bindings": [Any]()]
            case ["inbox"]:
                return ["needsYou": [Any]()]
            case ["cowork", "projects"]:
                return ["projects": SampleWorkspace.coworkProjects(now: now)]
            case let p where p.count == 4 && p[0] == "cowork" && p[1] == "projects" && p[3] == "files":
                return ["files": SampleProjectState.shared.files(projectId: p[2])]
            case let p where p.count == 4 && p[0] == "cowork" && p[1] == "projects" && p[3] == "status":
                return ["status": SampleProjectState.shared.status(projectId: p[2], now: now)]
            case let p where p.count == 3 && p[0] == "cowork" && p[1] == "projects":
                return SampleProjectState.shared.project(id: p[2], now: now).map { ["project": $0] }
            case let p where p.count == 2 && p[0] == "project-workspaces":
                return SampleProjectState.shared.workspace(key: p[1])
            case ["memory", "v2", "observations"]:
                return ["observations": [Any]()]
            case ["memory", "v2", "facts"]:
                return ["facts": [Any]()]
            case ["me"]:
                return ["user": SampleWorkspace.me(now: now)]
            case ["workspaces"]:
                return ["workspaces": SampleWorkspace.workspaces(now: now)]
            case ["models"]:
                return SampleWorkspace.models
            case ["runtime-devices"]:
                return ["runtimes": SampleWorkspace.runtimeDevices(now: now)]
            case ["runtime-presence"]:
                return [["runtimeId": SampleWorkspace.runtimeId, "online": true, "lastSeen": SampleWorkspace.ago(0, now: now)]]
            case ["gizzi-instances"]:
                // The computer answers through the relay, not a tunnel.
                return ["instances": [Any]()]
            // Customize: the sample computer has no skill or plugin folders
            // yet, so Skills opens on the skill directory (Discover), as the
            // web preview does; connectors list the catalog, none connected.
            case ["files", "list"]:
                return ["path": query["path"] ?? "", "entries": [Any]()]
            case ["skills", "catalog", "installed"]:
                return ["skills": [Any]()]
            case ["connectors"]:
                return ["connectors": SampleWorkspace.connectors]
            default:
                return nil
            }
        case .v2:
            switch parts {
            case ["artifacts"]:
                if query["scope"] == "shared" { return ["items": [Any](), "next_cursor": NSNull()] }
                let kind = query["kind"].flatMap { $0.isEmpty ? nil : $0 }
                let search = query["q"]?.lowercased().trimmingCharacters(in: .whitespaces) ?? ""
                let items = SampleWorkspace.artifactV2Summaries(now: now).filter { item in
                    (kind.map { item["kind"] as? String == $0 } ?? true)
                        && (search.isEmpty || (item["title"] as? String)?.lowercased().contains(search) == true)
                }
                return ["items": items, "next_cursor": NSNull()]
            case let p where p.count == 2 && p[0] == "artifacts":
                return SampleWorkspace.artifactsV2(now: now).first { $0["id"] as? String == p[1] }
            case ["org", "artifact-settings"]:
                return ["enabled": true]
            default:
                return nil
            }
        case .factory:
            switch parts {
            case ["agents"]: return ["agents": SampleFactory.agents]
            case ["teams"]: return ["teams": SampleFactory.teams]
            case ["campaigns"]: return ["campaigns": SampleFactory.campaigns]
            case let p where p.count == 3 && p[0] == "campaigns" && p[2] == "board": return SampleFactory.board
            case let p where p.count == 2 && p[0] == "dags": return SampleFactory.flow
            case let p where p.count == 3 && p[0] == "nodes": return SampleFactory.node(nodeId: p[2])
            case ["mail", "threads"]: return ["threads": [Any]()]
            default: return nil
            }
        case .gizzi:
            switch parts {
            case ["cron", "jobs"]:
                return SampleWorkspace.cronJobs(now: now)
            case let p where p.count == 3 && p[0] == "cron" && p[1] == "jobs":
                return SampleWorkspace.cronJobs(now: now).first { $0["id"] as? String == p[2] }
            case let p where p.count == 4 && p[0] == "cron" && p[3] == "runs":
                return [Any]()
            case ["automations", "routines"]:
                return SampleWorkspace.gizziRoutines(now: now)
            case ["automations", "loops"]:
                return SampleWorkspace.gizziLoops(now: now)
            case ["session"]:
                return [Any]()
            default:
                return nil
            }
        case .other:
            // Teach: `/api/har-derived-api/*` sits under `/api`, not `/api/v1`.
            switch parts {
            case ["api", "har-derived-api", "teach", "skills"]:
                // The flour order's recorded steps in full, so Run lists them.
                let skills = SampleWorkspace.teachSkills(now: now).map { skill -> [String: Any] in
                    guard skill["id"] as? String == "teach-sample-order" else { return skill }
                    var next = skill
                    next["workflow"] = [
                        "version": 1,
                        "start_url": "https://orders.millsupply.example/reorder",
                        "steps": [
                            ["id": "w1", "kind": "goto", "url": "{{profile.supplier_url}}/reorder"],
                            ["id": "w2", "kind": "select", "target": ["label": "Delivery week"], "value": "{{week}}"],
                            ["id": "w3", "kind": "fill", "target": ["label": "Bread flour 25 kg"], "value": "{{bags}}"],
                            ["id": "w4", "kind": "click", "target": ["label": "Place order"], "unsafe_to_repeat": true],
                        ],
                    ]
                    return next
                }
                return ["skills": skills]
            case ["api", "har-derived-api", "contracts"]:
                return ["contracts": SampleWorkspace.teachContracts(now: now)]
            case ["api", "har-derived-api", "teach", "profile"]:
                // The bakery's business profile ({{profile.key}} values).
                return [
                    "values": [
                        "supplier_url": ["value": "https://orders.millsupply.example", "label": "Mill supply orders", "type": "url"],
                        "reports_root": ["value": "/Users/harbor/Reports", "label": "Reports folder", "type": "folder"],
                        "store_email": ["value": "orders@harborbakery.example", "label": "Store email", "type": "email"],
                    ],
                    "updated_at": SampleWorkspace.ago(1_440),
                ]
            default:
                return nil
            }
        }
    }

    // MARK: - Writes

    private static func write(method: String, namespace: Namespace, parts: [String], body: Data?) -> Answer {
        let payload = body.flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] } ?? [:]
        if namespace == .v1, parts.count >= 3, parts[0] == "projects", parts[2] == "delegate",
           let result = SampleDelegateBoards.shared.write(method: method, projectId: parts[1], rest: Array(parts.dropFirst(3)), body: payload) {
            return result.map { json($0) } ?? json(["error": "preview"], status: 400)
        }
        if namespace == .v1, let answer = SampleProjectState.shared.write(method: method, parts: parts, body: payload) {
            return json(answer)
        }
        // Teach Analyze: an imported .har becomes a contract, and distill
        // answers the model's draft skill for it.
        if namespace == .other, method == "POST", parts == ["api", "har-derived-api", "ingest"] {
            return json(["contract_id": "contract-sample-import", "domain": "orders.millsupply.example"])
        }
        if namespace == .other, method == "POST", parts == ["api", "har-derived-api", "teach", "distill"] {
            let contractId = payload["contract_id"] as? String
            let domain = SampleWorkspace.teachContracts().first { $0["id"] as? String == contractId }?["domain"] as? String
                ?? "orders.millsupply.example"
            let note = (payload["note"] as? String).map { " \($0)" } ?? ""
            return json([
                "title": domain == "pos.squarely.example" ? "Close out the day's sales" : "Reorder supplies",
                "intent": "Pull today's totals from \(domain) and log them for {{date}}.\(note)",
                "source": "model",
                "redactions": 1,
                "domain": domain,
                "steps": [
                    ["kind": "ui", "text": "Open https://\(domain) and sign in"],
                    ["kind": "api", "text": "Read the totals for {{date}}", "target": "GET \(domain)/api/reports/daily?date={{date}}"],
                    ["kind": "ui", "text": "Check the totals match the register"],
                    ["kind": "note", "text": "Stop before anything is submitted: ask first."],
                ],
                "inputs": [["name": "date", "label": "Date", "example": "2026-10-29", "type": "date"]],
            ])
        }
        // A new chat opens on a sample session row (AgentSession decodes it).
        if namespace == .v1, method == "POST", parts == ["agent-sessions"] {
            let at = SampleWorkspace.ago(0)
            var metadata: [String: Any] = ["originSurface": payload["origin_surface"] as? String ?? "chat"]
            if let agentId = payload["agent_id"] as? String { metadata["agent_id"] = agentId }
            if let agentName = payload["agent_name"] as? String { metadata["agentName"] = agentName }
            return json([
                "id": "preview-session-\(UUID().uuidString.prefix(8).lowercased())",
                "name": payload["name"] as? String ?? "New chat",
                "created_at": at, "updated_at": at, "last_accessed": at,
                "message_count": 0, "active": true, "tags": [String](), "metadata": metadata,
            ])
        }
        return json(["success": true, "ok": true])
    }

    // MARK: - Helpers

    private static func split(_ path: String) -> (Namespace, String) {
        for (prefix, namespace) in [("/api/v1/", Namespace.v1), ("/api/v2/", .v2), ("/api/factory/", .factory), ("/v1/", .gizzi)] {
            if path.hasPrefix(prefix) { return (namespace, String(path.dropFirst(prefix.count))) }
        }
        return (.other, path)
    }

    static func json(_ value: Any, status: Int = 200) -> Answer {
        let data = (try? JSONSerialization.data(withJSONObject: value)) ?? Data("{}".utf8)
        return Answer(status: status, body: data)
    }
}

// MARK: - Delegate board

/// The visitor's own copy of each sample Delegate board, for this launch only
/// (preview-api.ts `boards`): add/remove a task, assign it to a bot, Auto.
final class SampleDelegateBoards: @unchecked Sendable {
    static let shared = SampleDelegateBoards()

    private let lock = NSLock()
    private var boards: [String: [String: Any]] = [:]
    private var made = 0

    private struct Lane {
        let estimateMin: Int
        let costPerTaskCents: Int
        let costSample: Int
    }

    /// Lane estimates (minutes) and average finished-task cost (cents) per bot.
    private static let lanes: [String: Lane] = [
        "preview-bot-a": Lane(estimateMin: 10, costPerTaskCents: 3, costSample: 12),
        "preview-bot-researcher": Lane(estimateMin: 25, costPerTaskCents: 9, costSample: 7),
        "preview-bot-designer": Lane(estimateMin: 40, costPerTaskCents: 14, costSample: 5),
        "preview-bot-writer": Lane(estimateMin: 30, costPerTaskCents: 6, costSample: 9),
    ]

    private static let tasks: [(String, String?, Int?)] = [
        ("Pre-order FAQ for the launch page", "Pick-up times, allergens, how to change an order.", 20),
        ("Choose photos for the three posts", nil, 15),
        ("Allergen labels for the new tarts", "Check against the recipe cards.", 30),
    ]

    /// `GET /api/v1/projects/:id/delegate` (nil → 404).
    func board(projectId: String) -> [String: Any]? {
        lock.lock(); defer { lock.unlock() }
        return current(projectId)
    }

    /// The delegate writes (`.some(nil)` → 400, nil → not a delegate route).
    func write(method: String, projectId: String, rest: [String], body: [String: Any]) -> [String: Any]?? {
        lock.lock(); defer { lock.unlock() }
        guard let board = current(projectId) else { return .some(nil) }
        switch (method, rest.count, rest.first) {
        case ("PUT", 1, "auto"):
            let on = body["auto"] as? Bool ?? false
            var next = board
            next["auto"] = on
            return .some(save(on ? autoAssign(next) : next))
        case ("POST", 1, "tasks"):
            let title = (body["title"] as? String ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            guard !title.isEmpty else { return .some(nil) }
            made += 1
            let id = "preview-task-new-\(made)"
            var next = board
            next["tasks"] = (board["tasks"] as? [[String: Any]] ?? []) + [[
                "id": id, "title": title,
                "notes": orNull(body["notes"] as? String),
                "estimateMin": orNull(body["estimateMin"] as? Int),
            ]]
            if next["auto"] as? Bool == true { next = autoAssign(next) }
            var saved = save(next)
            saved["taskId"] = id
            return .some(saved)
        case ("DELETE", 2, "tasks"):
            var next = board
            next["tasks"] = (board["tasks"] as? [[String: Any]] ?? []).filter { $0["id"] as? String != rest[1] }
            return .some(save(next))
        case ("POST", 3, "tasks") where rest[2] == "assign":
            guard let result = assign(board, taskId: rest[1], botId: body["botId"] as? String ?? "") else { return .some(nil) }
            var saved = save(result.board)
            saved["thread"] = result.thread
            return .some(saved)
        default:
            return nil
        }
    }

    // MARK: Private (lock held)

    private func current(_ projectId: String) -> [String: Any]? {
        if boards[projectId] == nil, let fresh = Self.sample(projectId: projectId) { boards[projectId] = fresh }
        guard var board = boards[projectId] else { return nil }
        board["now"] = SampleWorkspace.iso(Date())
        return board
    }

    private func save(_ board: [String: Any]) -> [String: Any] {
        var next = board
        next["queuedCostCents"] = orNull(Self.laneCost(next["bots"] as? [[String: Any]] ?? []))
        if let id = next["projectId"] as? String { boards[id] = next }
        return next
    }

    private static func sample(projectId: String, now: Date = Date()) -> [String: Any]? {
        if projectId == SampleWorkspace.cateringProjectId || projectId.hasPrefix("preview-workspace-") {
            return ["projectId": projectId, "auto": false, "tasks": [Any](), "bots": [Any](),
                    "queuedCostCents": NSNull(), "now": SampleWorkspace.iso(now)]
        }
        guard projectId == SampleWorkspace.projectId else { return nil }
        let threads = SampleWorkspace.threads(now: now)
        let bots: [[String: Any]] = SampleWorkspace.botSeeds.map { seed in
            let lane = lanes[seed.id]!
            let queue: [[String: Any]] = threads.filter { $0["botId"] as? String == seed.id && $0["projectId"] as? String == projectId }.map { thread in
                ["threadId": thread["id"] ?? "", "taskId": NSNull(), "title": thread["title"] ?? "",
                 "status": thread["status"] ?? "", "estimateMin": lane.estimateMin]
            }
            var bot: [String: Any] = [
                "id": seed.id, "name": seed.name,
                "running": queue.contains { $0["status"] as? String == "working" },
                "costPerTaskCents": lane.costPerTaskCents, "costSample": lane.costSample,
                "unestimated": 0, "queue": queue,
            ]
            bot["busyMinutes"] = laneMinutes(bot)
            return bot
        }
        return [
            "projectId": projectId, "auto": false,
            "tasks": tasks.enumerated().map { index, task in
                ["id": "preview-task-\(index + 1)", "title": task.0,
                 "notes": orNull(task.1), "estimateMin": orNull(task.2)]
            },
            "bots": bots, "queuedCostCents": orNull(laneCost(bots)), "now": SampleWorkspace.iso(now),
        ]
    }

    private static func isLive(_ item: [String: Any]) -> Bool {
        let status = item["status"] as? String
        return status == "working" || status == "queued"
    }

    private static func laneMinutes(_ bot: [String: Any]) -> Int {
        (bot["queue"] as? [[String: Any]] ?? []).filter(isLive).reduce(0) { $0 + ($1["estimateMin"] as? Int ?? 0) }
    }

    /// Estimated cost of everything working or waiting in the lanes (nil when no bot has a cost yet).
    private static func laneCost(_ bots: [[String: Any]]) -> Int? {
        guard bots.contains(where: { $0["costPerTaskCents"] is Int }) else { return nil }
        return bots.reduce(0) { sum, bot in
            sum + (bot["queue"] as? [[String: Any]] ?? []).filter(isLive).count * (bot["costPerTaskCents"] as? Int ?? 0)
        }
    }

    private func assign(_ board: [String: Any], taskId: String, botId: String) -> (board: [String: Any], thread: [String: Any])? {
        let tasks = board["tasks"] as? [[String: Any]] ?? []
        let bots = board["bots"] as? [[String: Any]] ?? []
        guard let task = tasks.first(where: { $0["id"] as? String == taskId }),
              let bot = bots.first(where: { $0["id"] as? String == botId }) else { return nil }
        let now = SampleWorkspace.iso(Date())
        let busy = (bot["queue"] as? [[String: Any]] ?? []).contains { $0["status"] as? String == "working" }
        let status = busy ? "queued" : "working"
        made += 1
        var thread = SampleWorkspace.threads()[0]
        let title = task["title"] as? String ?? ""
        thread["id"] = "preview-thread-delegated-\(made)"
        thread["botId"] = botId
        thread["projectId"] = board["projectId"]
        thread["parentThreadId"] = NSNull()
        thread["title"] = title
        thread["objective"] = task["notes"] as? String ?? title
        thread["status"] = status
        thread["group"] = busy ? "queued" : "working"
        thread["statusLine"] = busy ? "Waits for the task ahead of it" : "Starting"
        thread["todo"] = [Any]()
        thread["progress"] = [0, 0]
        thread["dependsOn"] = [String]()
        thread["lastActivityAt"] = now
        thread["createdAt"] = now
        var nextBot = bot
        nextBot["running"] = true
        nextBot["queue"] = (bot["queue"] as? [[String: Any]] ?? []) + [[
            "threadId": thread["id"] ?? "", "taskId": taskId, "title": title,
            "status": status, "estimateMin": task["estimateMin"] ?? NSNull(),
        ]]
        nextBot["busyMinutes"] = Self.laneMinutes(nextBot)
        var next = board
        next["tasks"] = tasks.filter { $0["id"] as? String != taskId }
        next["bots"] = bots.map { $0["id"] as? String == botId ? nextBot : $0 }
        return (next, thread)
    }

    /// Free-first: the bot with the shortest lane takes the next task.
    private func autoAssign(_ board: [String: Any]) -> [String: Any] {
        var next = board
        for task in board["tasks"] as? [[String: Any]] ?? [] {
            let bots = next["bots"] as? [[String: Any]] ?? []
            guard let bot = bots.min(by: { Self.laneMinutes($0) < Self.laneMinutes($1) }),
                  let taskId = task["id"] as? String, let botId = bot["id"] as? String,
                  let result = assign(next, taskId: taskId, botId: botId) else { continue }
            next = result.board
        }
        return next
    }
}

/// A JSON value, or JSON null for nil.
func orNull(_ value: Any?) -> Any { value ?? NSNull() }

// MARK: - Project page state

/// What the visitor does on a sample project page, for this launch only: what
/// they tell Al, the context files they add, their status page edits.
final class SampleProjectState: @unchecked Sendable {
    static let shared = SampleProjectState()

    private let lock = NSLock()
    private var sent: [String: [[String: Any]]] = [:]
    private var attached: [String: [[String: Any]]] = [:]
    private var statuses: [String: [String: Any]] = [:]
    private var details: [String: [String: Any]] = [:]
    private var routines: [[String: Any]] = []

    func addedRoutines() -> [[String: Any]] {
        lock.lock()
        defer { lock.unlock() }
        return routines
    }

    func messages(projectId: String, now: Date) -> [[String: Any]]? {
        lock.lock()
        defer { lock.unlock() }
        let extra = sent[projectId] ?? []
        guard let base = SampleWorkspace.projectMessages(projectId: projectId, now: now) else {
            // A Chat project's coordinator (registered below).
            return sent[projectId] == nil && !projectId.hasPrefix("preview-workspace-") ? nil : extra
        }
        return base + extra
    }

    func files(projectId: String) -> [[String: Any]] {
        lock.lock()
        defer { lock.unlock() }
        return attached[projectId] ?? []
    }

    /// The launch's status page, the same on the card and the full view.
    func status(projectId: String, now: Date) -> [String: Any] {
        lock.lock()
        defer { lock.unlock() }
        if let edited = statuses[projectId] { return edited }
        guard projectId == SampleWorkspace.projectId else {
            return ["version": 1, "lifecycle": "active", "state": "on_track", "next_action": "", "sections": [String: Any](),
                    "log": [Any](), "archive": [Any](), "revision": 0, "closed_at": NSNull()]
        }
        let section: (String) -> [String: Any] = { text in
            ["text": text, "updated_by": "Al", "updated_at": SampleWorkspace.ago(20, now: now)]
        }
        return [
            "version": 1, "lifecycle": "active", "state": "needs_decision",
            "next_action": "Pick a hero for the launch page, then the email and posts can start.",
            "sections": [
                "goal": section("Launch the spring menu by Friday: a price check, a launch page, then the email and posts."),
                "status": section("Price check is under way. The launch page is drafted and waiting on a hero pick."),
                "next_actions": section("Pick a hero. Yessir starts the email and three posts after that."),
                "owners": section("Goldie: prices. Rimraf: page. Yessir: email and posts."),
            ] as [String: Any],
            "log": [["at": SampleWorkspace.ago(18, now: now), "by": "Rimraf", "section": "status",
                     "note": "Launch page draft is ready with two hero options.", "proof_url": NSNull()]],
            "archive": [Any](), "revision": 3, "closed_at": NSNull(),
        ]
    }

    func project(id: String, now: Date) -> [String: Any]? {
        lock.lock()
        defer { lock.unlock() }
        guard var row = SampleWorkspace.coworkProjects(now: now).first(where: { $0["id"] as? String == id }) else { return nil }
        for (key, value) in details[id] ?? [:] { row[key] = value }
        return row
    }

    func workspace(key: String) -> [String: Any]? {
        guard key.hasPrefix("chat-") else { return nil }
        return ["projectId": "preview-workspace-\(key)", "workerId": "preview", "settings": [String: Any]()]
    }

    /// nil: not one of these routes.
    func write(method: String, parts: [String], body: [String: Any]) -> [String: Any]? {
        lock.lock()
        defer { lock.unlock() }
        switch (method, parts.count) {
        case ("POST", 3) where parts[0] == "projects" && parts[2] == "messages":
            let at = SampleWorkspace.iso(Date())
            let text = body["text"] as? String ?? ""
            let user: [String: Any] = ["id": "preview-sent-\(UUID().uuidString)", "role": "user", "text": text,
                                       "payload": NSNull(), "createdAt": at]
            let reply: [String: Any] = ["id": "preview-sent-\(UUID().uuidString)", "role": "coordinator",
                                        "text": "This is a sample project, so nothing runs. Signed in, I would split that into threads for the team.",
                                        "payload": ["kind": "answer"], "createdAt": at]
            sent[parts[1], default: []] += [user, reply]
            return ["message": user]
        case ("POST", 1) where parts[0] == "uploads":
            let id = "preview-upload-\(UUID().uuidString.prefix(8))"
            return ["uploadId": id, "url": "/api/v1/uploads/\(id)"]
        case ("POST", 4) where parts[0] == "cowork" && parts[1] == "projects" && parts[3] == "files":
            let id = "preview-file-\(UUID().uuidString.prefix(8))"
            let row: [String: Any] = [
                "id": id, "project_id": parts[2], "name": body["name"] as? String ?? "File",
                "url": orNull(body["url"]), "upload_id": orNull(body["upload_id"]),
                "media_type": orNull(body["media_type"]), "created_at": SampleWorkspace.iso(Date()),
            ]
            attached[parts[2], default: []].append(row)
            return ["file": ["id": id]]
        case ("DELETE", 5) where parts[0] == "cowork" && parts[3] == "files":
            attached[parts[2]]?.removeAll { $0["id"] as? String == parts[4] }
            return ["ok": true]
        case ("PATCH", 4) where parts[0] == "cowork" && parts[3] == "status":
            lock.unlock()
            var next = status(projectId: parts[2], now: Date())
            lock.lock()
            if let state = body["state"] { next["state"] = state }
            if let key = body["section"] as? String, let text = body["text"] as? String {
                var sections = next["sections"] as? [String: Any] ?? [:]
                sections[key] = ["text": text, "updated_by": "you", "updated_at": SampleWorkspace.iso(Date())]
                next["sections"] = sections
            }
            statuses[parts[2]] = next
            return ["status": next]
        case ("PATCH", 3) where parts[0] == "cowork" && parts[1] == "projects":
            details[parts[2], default: [:]].merge(body) { _, new in new }
            return ["ok": true]
        case ("POST", 2) where parts == ["automation", "routines"]:
            let at = SampleWorkspace.iso(Date())
            var row = body
            row["id"] = "preview-routine-\(UUID().uuidString.prefix(8))"
            row["status"] = "active"
            row["created_at"] = at
            row["updated_at"] = at
            routines.append(row)
            return row
        case ("PUT", 2) where parts[0] == "project-workspaces":
            return workspace(key: parts[1])
        default:
            return nil
        }
    }
}

// MARK: - Factory

/// The launch as a Factory campaign, so the Team lanes, Board and Flow have
/// something to show. The ids match the sample bots and the spring project.
enum SampleFactory {
    private static let dagId = "preview-dag-spring"

    private static func card(_ id: String, _ title: String, _ status: String, _ assignee: String, _ proven: Int, _ total: Int,
                             depth: Int, blockedBy: [String] = [], needsYou: Bool = false) -> [String: Any] {
        ["dagId": dagId, "nodeId": id, "title": title, "status": status, "assignee": assignee,
         "bindingType": NSNull(), "proof": ["proven": proven, "total": total], "blockedBy": blockedBy,
         "needsYou": needsYou, "depth": depth]
    }

    private static var cards: [[String: Any]] {
        [
            card("prices", "Price check: spring menu", "working", "goldie", 1, 3, depth: 0),
            card("page", "Launch page", "needs_you", "rimraf", 2, 3, depth: 0, needsYou: true),
            card("email", "Announcement email and three posts", "blocked", "yessir", 0, 2, depth: 1, blockedBy: ["page"]),
        ]
    }

    static var campaigns: [[String: Any]] {
        [["id": "preview-campaign-spring", "projectId": SampleWorkspace.projectId, "title": "Harbor Bakery spring launch",
          "intent": "Launch the spring menu", "status": "active", "proven": 3, "total": 8, "needsYou": 1]]
    }

    static var board: [String: Any] {
        let all = cards
        return [
            "campaign": ["id": "preview-campaign-spring", "title": "Harbor Bakery spring launch", "intent": "Launch the spring menu"],
            "summary": ["now": [all[0]], "next": [all[2]], "proven": ["k": 3, "n": 8], "needsYou": [all[1]]] as [String: Any],
            "waves": [["depth": 0, "nodes": [all[0], all[1]]], ["depth": 1, "nodes": [all[2]]]],
        ]
    }

    static var flow: [String: Any] {
        ["dagId": dagId, "title": "Harbor Bakery spring launch", "nodes": cards,
         "edges": [["from": "page", "to": "email", "type": "blocked_by"]]]
    }

    static func node(nodeId: String) -> [String: Any]? {
        guard let found = cards.first(where: { $0["nodeId"] as? String == nodeId }) else { return nil }
        return [
            "card": found,
            "spec": ["intent": found["title"] ?? "", "miniRequirements": ["Follows the brief", "Ready for you to check"],
                     "proofContract": [["line": "Draft saved in the project", "checkedBy": "Al", "evidence": [Any]()]]] as [String: Any],
            "progressMd": "Started from the project's brief.",
        ]
    }

    static var agents: [[String: Any]] {
        [
            agent("preview-bot-a", "A://", role: "Coordinates the launch", state: "working", node: nil),
            agent("preview-bot-researcher", "Goldie", role: "Researcher", state: "working", node: ("prices", "Price check: spring menu")),
            agent("preview-bot-designer", "Rimraf", role: "Designer", state: "waiting", node: ("page", "Launch page")),
            agent("preview-bot-writer", "Yessir", role: "Writer", state: "idle", node: nil),
        ]
    }

    private static func agent(_ id: String, _ name: String, role: String, state: String, node: (String, String)?) -> [String: Any] {
        [
            "id": id, "slug": name.lowercased(), "name": name, "team": "spring", "address": "\(name.lowercased())@spring",
            "avatar": NSNull(), "role": role, "binding": ["type": "native"], "state": state, "machine": NSNull(), "pane": NSNull(),
            "currentNode": orNull(node.map { ["dagId": dagId, "nodeId": $0.0, "title": $0.1] }),
            "proof": NSNull(), "context": ["usedPct": NSNull(), "tokens": NSNull()], "reach": [Any](), "fields": [String: Any](),
        ]
    }

    static var teams: [[String: Any]] {
        [[
            "name": "spring", "preset": NSNull(), "presets": [Any](),
            "agents": ["preview-bot-a", "preview-bot-researcher", "preview-bot-designer", "preview-bot-writer"],
            "edges": [
                ["kind": "directs", "from": "preview-bot-a", "to": "preview-bot-researcher"],
                ["kind": "directs", "from": "preview-bot-a", "to": "preview-bot-designer"],
                ["kind": "delegates_to", "from": "preview-bot-designer", "to": "preview-bot-writer"],
            ],
        ]]
    }
}
