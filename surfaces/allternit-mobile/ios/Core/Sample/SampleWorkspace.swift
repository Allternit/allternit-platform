import Foundation

/// `-sample-workspace` (DEBUG only): the app runs signed-in on the same sample
/// workspace as the web's public preview (allternit-ai src/embed/
/// sample-workspace.ts), for side-by-side and App Store screenshots. One small
/// studio running a bakery's spring launch, so Bots, Projects, Artifacts and
/// Automation Tasks all tell the same story.
///
/// Turning it on (`activate()`, called from `AuthManager.configure`):
/// - `SampleURLProtocol` answers the app's API calls from this data
///   (`SampleWorkspaceAPI`, the port of src/embed/preview-api.ts);
/// - `AuthManager` reports signed in and onboarding is skipped;
/// - one computer is listed online, so nothing asks to connect one.
enum SampleWorkspace {
    /// True for the whole launch when `-sample-workspace` is passed to a
    /// DEBUG build. Always false in Release.
    static let isEnabled: Bool = {
        #if DEBUG
        launchArgumentEnabled(LaunchArgument.sampleWorkspace)
        #else
        false
        #endif
    }()

    /// Registers the sample API on every URLSession the app uses. Idempotent.
    static func activate() {
        guard isEnabled else { return }
        SampleURLProtocol.register()
    }

    static let userId = "preview"
    static let userName = "You"
    static let workspaceId = "preview-workspace"
    static let projectId = "preview-project-spring-launch"
    static let cateringProjectId = "preview-project-catering"
    static let runtimeId = "preview-computer"
    static let primaryBotTemplateId = "primary-a"

    // MARK: - Time

    /// An ISO time `minutes` before now, so the sample always looks current.
    static func ago(_ minutes: Double, now: Date = Date()) -> String {
        iso(now.addingTimeInterval(-minutes * 60))
    }

    static func iso(_ date: Date) -> String {
        date.formatted(Date.ISO8601FormatStyle(includingFractionalSeconds: true))
    }

    // MARK: - Bots

    struct BotSeed {
        let id: String
        let name: String
        let title: String
        let tagline: String
        /// Gizzi cast look (pack sprite); nil = the A:// mark.
        let look: String?
        let primary: Bool
        let category: String
    }

    static let botSeeds: [BotSeed] = [
        BotSeed(id: "preview-bot-a", name: "A://", title: "Main bot",
                tagline: "Your first stop. Splits work into threads and hands it to the team.",
                look: nil, primary: true, category: "ops"),
        BotSeed(id: "preview-bot-researcher", name: "Goldie", title: "Researcher",
                tagline: "Finds sources, checks numbers and writes the short version.",
                look: "goldie", primary: false, category: "research"),
        BotSeed(id: "preview-bot-designer", name: "Rimraf", title: "Designer",
                tagline: "Pages, decks and launch graphics.",
                look: "rimraf", primary: false, category: "design"),
        BotSeed(id: "preview-bot-writer", name: "Yessir", title: "Writer",
                tagline: "Emails, posts and the copy on the site.",
                look: "yessir", primary: false, category: "writing"),
    ]

    /// The cast look's avatar config (web bot-looks.ts `castAvatarConfig`):
    /// the gizzi-youtube pack sprite sheet.
    static func avatar(look: String?) -> [String: Any]? {
        guard let look else { return nil }
        return [
            "type": "mascot",
            "mascotTemplate": "pet",
            "look": look,
            "pet": ["spriteUrl": "/avatar-packs/gizzi-youtube/\(look)-sheet.webp"],
        ]
    }

    /// `GET /api/v1/agents` rows, in the agent_routes.rs wire shape AgentRecord
    /// decodes (snake_case columns, bot profile in `config` and top level).
    static func bots(now: Date = Date()) -> [[String: Any]] {
        botSeeds.enumerated().map { index, seed in
            var profile: [String: Any] = [
                "displayName": seed.name,
                "title": seed.title,
                "tagline": seed.tagline,
                "botCategory": seed.category,
                "groupChatEnabled": true,
            ]
            if seed.primary { profile["chiefOfStaff"] = true }
            var config: [String: Any] = ["isBot": true, "botProfile": profile]
            if seed.primary { config["templateId"] = primaryBotTemplateId }
            if let avatar = avatar(look: seed.look) { config["avatar"] = avatar }
            return [
                "id": seed.id,
                "name": seed.name,
                "description": seed.tagline,
                "type": "bot",
                "model": "kimi-k3",
                "provider": "allternit",
                "status": "idle",
                "workspace_id": workspaceId,
                "trust_tier": "standard",
                "enabled_modes": ["chat", "cowork", "code", "browser"],
                "category": seed.category,
                "mode": "primary",
                "is_primary": seed.primary,
                "created_at": ago(Double(60 * 24 * 9 - index), now: now),
                "updated_at": ago(Double(30 + index * 7), now: now),
                "config": config,
                "bot_profile": profile,
            ]
        }
    }

    // MARK: - Threads

    private struct ThreadSeed {
        let bot: String
        let title: String
        let status: String
        let group: String
        let line: String
        let todo: [(String, String)]
        let minutes: Double
    }

    private static let threadSeeds: [ThreadSeed] = [
        ThreadSeed(bot: "preview-bot-researcher", title: "Price check: spring menu vs. three nearby bakeries",
                   status: "working", group: "working", line: "Comparing 14 items; two shops still to read",
                   todo: [("Collect menus", "done"), ("Match items by size", "active"), ("Write the summary table", "pending")],
                   minutes: 3),
        ThreadSeed(bot: "preview-bot-designer", title: "Launch page for the spring menu",
                   status: "review", group: "waiting", line: "Draft ready. Two hero options for you to pick",
                   todo: [("Layout", "done"), ("Photos and captions", "done"), ("Pick a hero", "active")],
                   minutes: 18),
        ThreadSeed(bot: "preview-bot-writer", title: "Announcement email and three posts",
                   status: "queued", group: "queued", line: "Starts when the launch page is approved",
                   todo: [("Email", "pending"), ("Posts", "pending")],
                   minutes: 25),
        ThreadSeed(bot: "preview-bot-a", title: "Spring launch plan",
                   status: "working", group: "working", line: "Tracking 3 threads; next check-in at 4 pm",
                   todo: [("Split the work", "done"), ("Research and page", "active"), ("Email and posts", "pending")],
                   minutes: 1),
    ]

    /// `GET /api/v1/threads` rows (lib/bots/threads.ts Thread, camelCase).
    static func threads(now: Date = Date()) -> [[String: Any]] {
        threadSeeds.enumerated().map { index, seed in
            let done = seed.todo.filter { $0.1 == "done" }.count
            return [
                "id": "preview-thread-\(index + 1)",
                "botId": seed.bot,
                "projectId": projectId,
                "parentThreadId": orNull(seed.bot == "preview-bot-a" ? nil : "preview-thread-4"),
                "kind": "task",
                "incognito": false,
                "title": seed.title,
                "objective": seed.title,
                "successCriteria": NSNull(),
                "status": seed.status,
                "group": seed.group,
                "statusLine": seed.line,
                "todo": seed.todo.map { ["text": $0.0, "state": $0.1] },
                "progress": [done, seed.todo.count],
                "summary": NSNull(),
                "checkpoint": NSNull(),
                "currentSessionId": NSNull(),
                "generation": 1,
                "contextUsed": 0.2 + Double(index) * 0.1,
                "dependsOn": seed.status == "queued" ? ["preview-thread-2"] : [],
                "createdBy": userId,
                "lastActivityAt": ago(seed.minutes, now: now),
                "resolvedAt": NSNull(),
                "createdAt": ago(Double(60 * 5 + index), now: now),
            ]
        }
    }

    // MARK: - Projects

    private struct ProjectSeed {
        let id: String
        let title: String
        let description: String
        let botIds: [String]
        let counts: [String: Int]
        let minutes: Double
        let favorite: Bool
    }

    private static let projectSeeds: [ProjectSeed] = [
        ProjectSeed(id: projectId, title: "Harbor Bakery spring launch",
                    description: "New spring menu: research, launch page, email and posts.",
                    botIds: botSeeds.map(\.id),
                    counts: ["waiting": 1, "working": 2, "queued": 1, "idle": 0, "resolved": 3],
                    minutes: 1, favorite: true),
        ProjectSeed(id: cateringProjectId, title: "Catering price sheet",
                    description: "One page of catering prices for offices.",
                    botIds: ["preview-bot-researcher", "preview-bot-writer"],
                    counts: ["waiting": 0, "working": 0, "queued": 0, "idle": 1, "resolved": 2],
                    minutes: 60 * 26, favorite: false),
    ]

    /// `GET /api/v1/bot-projects` rows (BotProjectSummary, camelCase).
    static func projects(now: Date = Date()) -> [[String: Any]] {
        projectSeeds.map { seed in
            [
                "id": seed.id,
                "title": seed.title,
                "description": seed.description,
                "botIds": seed.botIds,
                "counts": seed.counts,
                "lastActivityAt": ago(seed.minutes, now: now),
                "archived": false,
                "favorite": seed.favorite,
            ]
        }
    }

    /// The same two projects as `GET /api/v1/cowork/projects` rows
    /// (cowork_routes.rs ProjectRow, snake_case) for the Projects tab.
    static func coworkProjects(now: Date = Date()) -> [[String: Any]] {
        projectSeeds.map { seed in
            [
                "id": seed.id,
                "user_id": userId,
                "title": seed.title,
                "description": seed.description,
                "instructions": NSNull(),
                "git_remote": NSNull(),
                "default_branch": NSNull(),
                "created_at": ago(seed.minutes + 60 * 24 * 3, now: now),
                "updated_at": ago(seed.minutes, now: now),
            ]
        }
    }

    // MARK: - Artifacts

    private struct ArtifactSeed {
        let title: String
        let type: String
        let summary: String
        let tags: [String]
        let minutes: Double
        let body: String
    }

    private static let artifactSeeds: [ArtifactSeed] = [
        ArtifactSeed(title: "Spring menu price check", type: "research",
                     summary: "14 items compared with three nearby bakeries.", tags: ["research", "pricing"], minutes: 40,
                     body: "Our croissant is 40 cents under the area average. Sourdough is level. Two seasonal tarts have no local match."),
        ArtifactSeed(title: "Launch page draft", type: "design",
                     summary: "Spring menu page with two hero options.", tags: ["site"], minutes: 18,
                     body: "Hero A: the full spring table. Hero B: one tart, close up. Menu below, with prices and the pre-order link."),
        ArtifactSeed(title: "Spring launch plan", type: "plan",
                     summary: "Who does what, and in which order.", tags: ["plan"], minutes: 60 * 5,
                     body: "1. Price check (Goldie). 2. Launch page (Rimraf). 3. Email and posts after the page is approved (Yessir)."),
        ArtifactSeed(title: "Catering price sheet", type: "document",
                     summary: "Office catering prices, one page.", tags: ["catering"], minutes: 60 * 26,
                     body: "Breakfast box, lunch tray and pastry platter, each for 10, 20 and 40 people."),
    ]

    /// `GET /api/v1/artifacts` rows (services/artifacts-api.ts ArtifactDto).
    static func artifacts(now: Date = Date()) -> [[String: Any]] {
        artifactSeeds.enumerated().map { index, seed in
            let id = "preview-artifact-\(index + 1)"
            let at = ago(seed.minutes, now: now)
            return [
                "id": id,
                "workspaceId": workspaceId,
                "title": seed.title,
                "type": seed.type,
                "status": index == 1 ? "draft" : "active",
                "summary": seed.summary,
                "tags": seed.tags,
                "createdAt": at,
                "updatedAt": at,
                "sections": [[
                    "id": "\(id)-s1", "artifactId": id, "heading": seed.title, "kind": "text",
                    "body": seed.body, "position": 0, "createdAt": at, "updatedAt": at,
                ]],
                "revisions": [],
                "visibility": "private",
                "is_owner": true,
                "can_edit": false,
            ]
        }
    }

    static let launchPageHTML = """
    <!doctype html><html><head><meta charset="utf-8"><title>Spring at Harbor Bakery</title>
    <style>body{margin:0;font:16px/1.5 Georgia,serif;color:#2b2420;background:#fbf7f1}header{padding:56px 24px;text-align:center;background:#f3e6d4}h1{margin:0;font-size:40px}main{max-width:640px;margin:0 auto;padding:32px 24px}li{display:flex;justify-content:space-between;border-bottom:1px solid #e8dccb;padding:10px 0}a{color:#b5532f}</style></head>
    <body><header><h1>Spring at Harbor Bakery</h1><p>New tarts and breads, from Friday.</p></header>
    <main><ul style="list-style:none;padding:0"><li><span>Rhubarb custard tart</span><span>$6.50</span></li><li><span>Lemon poppy loaf</span><span>$9.00</span></li><li><span>Strawberry croissant</span><span>$4.75</span></li><li><span>Spring onion focaccia</span><span>$7.25</span></li></ul>
    <p><a href="#">Pre-order for Friday pickup</a></p></main></body></html>
    """

    /// The same four artifacts as the Artifacts library (v2) opens them
    /// (artifacts_v2.rs `artifact_json`).
    static func artifactsV2(now: Date = Date()) -> [[String: Any]] {
        artifactSeeds.enumerated().map { index, seed in
            let at = ago(seed.minutes, now: now)
            let design = seed.type == "design"
            return [
                "id": "preview-artifact-\(index + 1)",
                "kind": design ? "design" : "doc",
                "title": seed.title,
                "owner": ["id": userId, "name": userName],
                "updated_at": at,
                "created_at": at,
                "current_version": 1,
                "shared_version": NSNull(),
                "visibility": "private",
                "my_access": "view",
                "origin": ["surface": "bot"],
                "runtime_version": 1,
                "capabilities": [String: Any](),
                "version": [
                    "version": 1,
                    "body": design ? launchPageHTML : "# \(seed.title)\n\n\(seed.summary)\n\n\(seed.body)\n",
                    "body_format": design ? "text/html" : "text/markdown",
                    "meta": [String: Any](),
                    "author_id": userId,
                    "created_at": at,
                ],
            ]
        }
    }

    /// List rows are summaries; the body comes with GET /:id.
    static func artifactV2Summaries(now: Date = Date()) -> [[String: Any]] {
        artifactsV2(now: now).map { item in
            var summary = item
            for key in ["version", "capabilities", "shared_version", "created_at"] { summary[key] = nil }
            return summary
        }
    }

    // MARK: - Automation

    /// `GET /api/v1/automation/goals` (bare array).
    static func goals(now: Date = Date()) -> [[String: Any]] {
        [[
            "id": "preview-goal-1", "user_id": userId, "workspace_id": workspaceId, "agent_id": "preview-bot-a",
            "title": "Launch the spring menu by Friday", "description": "Page live, email sent, three posts scheduled.",
            "status": "active", "priority": "high", "progress": 45,
            "created_at": ago(60 * 5, now: now), "updated_at": ago(1, now: now),
        ]]
    }

    struct RoutineSeed {
        let id: String
        let bot: String
        let name: String
        let description: String
        let cron: String
        let createdMinutes: Double
        let updatedMinutes: Double
        let nextInMinutes: Double
        let lastMinutes: Double
    }

    static let routineSeeds: [RoutineSeed] = [
        RoutineSeed(id: "preview-routine-1", bot: "preview-bot-researcher", name: "Morning sales summary",
                    description: "Yesterday’s sales by item, in one short message.", cron: "0 7 * * *",
                    createdMinutes: 60 * 24 * 9, updatedMinutes: 60 * 24, nextInMinutes: 60 * 9, lastMinutes: 60 * 15),
        RoutineSeed(id: "preview-routine-2", bot: "preview-bot-writer", name: "Weekly reviews digest",
                    description: "New reviews from the week, with suggested replies.", cron: "0 9 * * 1",
                    createdMinutes: 60 * 24 * 9, updatedMinutes: 60 * 24 * 3, nextInMinutes: 60 * 50, lastMinutes: 60 * 24 * 6),
    ]

    static let timezone = "America/Chicago"

    /// `GET /api/v1/automation/routines` (bare array of AutomationRoutine rows).
    static func routines(now: Date = Date()) -> [[String: Any]] {
        routineSeeds.map { seed in
            [
                "id": seed.id, "user_id": userId, "workspace_id": workspaceId, "agent_id": seed.bot,
                "name": seed.name, "description": seed.description,
                "status": "active", "schedule_type": "cron", "schedule_expression": seed.cron, "timezone": timezone,
                "execution_domain": "cloud", "config": [String: Any](),
                "created_at": ago(seed.createdMinutes, now: now), "updated_at": ago(seed.updatedMinutes, now: now),
                "next_run_at": iso(now.addingTimeInterval(seed.nextInMinutes * 60)), "last_run_at": ago(seed.lastMinutes, now: now),
                "recentRuns": [[
                    "id": "\(seed.id)-run-1", "status": "succeeded",
                    "startedAt": ago(seed.lastMinutes, now: now), "finishedAt": ago(seed.lastMinutes - 1, now: now),
                    "durationMs": 48_000,
                ]],
            ]
        }
    }

    /// `GET /api/v1/automation/loops` (bare array).
    static func loops(now: Date = Date()) -> [[String: Any]] {
        [[
            "id": "preview-loop-1", "user_id": userId, "workspace_id": workspaceId, "agent_id": "preview-bot-a",
            "name": "Watch the launch threads",
            "description": "Checks the spring launch threads every hour and pings you when one needs you.",
            "status": "active", "schedule_type": "interval", "schedule_expression": "1h", "execution_domain": "cloud",
            "config": [String: Any](), "created_at": ago(60 * 5, now: now), "updated_at": ago(1, now: now),
        ]]
    }

    /// The routines as the computer's scheduler lists them
    /// (`GET v1/cron/jobs`, gizzi-code cron/types.ts, camelCase) — the
    /// Automation Tasks list's source on the phone.
    static func cronJobs(now: Date = Date()) -> [[String: Any]] {
        routineSeeds.map { seed in
            [
                "id": seed.id, "name": seed.name, "description": seed.description,
                "type": "agent", "status": "active",
                "schedule": ["type": "cron", "expression": seed.cron, "timezone": timezone],
                "createdAt": ago(seed.createdMinutes, now: now), "updatedAt": ago(seed.updatedMinutes, now: now),
                "lastRunAt": ago(seed.lastMinutes, now: now),
                "nextRunAt": iso(now.addingTimeInterval(seed.nextInMinutes * 60)),
                "runCount": 9, "failCount": 0, "tags": [String](),
                "config": ["prompt": seed.description, "agentId": seed.bot],
            ]
        }
    }

    /// `GET v1/automations/routines` (gizzi-code automations.ts, ms epochs).
    static func gizziRoutines(now: Date = Date()) -> [[String: Any]] {
        routineSeeds.map { seed in
            [
                "id": seed.id, "agent_id": seed.bot, "name": seed.name,
                "steps": [["command": seed.description, "status": "done"]],
                "trigger": "schedule", "schedule": seed.cron, "state": "active",
                "time_created": ms(now, minutesAgo: seed.createdMinutes),
                "time_updated": ms(now, minutesAgo: seed.updatedMinutes),
            ]
        }
    }

    /// `GET v1/automations/loops` (gizzi-code automations.ts, ms epochs).
    static func gizziLoops(now: Date = Date()) -> [[String: Any]] {
        [[
            "id": "preview-loop-1", "agent_id": "preview-bot-a",
            "command": "Checks the spring launch threads every hour and pings you when one needs you.",
            "exit_condition": NSNull(), "max_iterations": 24, "iteration_log": [],
            "state": "running",
            "time_created": ms(now, minutesAgo: 60 * 5), "time_updated": ms(now, minutesAgo: 1),
        ]]
    }

    private static func ms(_ now: Date, minutesAgo: Double) -> Double {
        (now.timeIntervalSince1970 - minutesAgo * 60) * 1000
    }

    // MARK: - Project home: the project conversation and the Delegate board

    static let projectBrief = "Launch the spring menu: a price check, a launch page, then the email and posts."

    /// `GET /api/v1/projects/:id/messages` (nil → 404 for unknown projects).
    static func projectMessages(projectId id: String, now: Date = Date()) -> [[String: Any]]? {
        guard id == projectId else { return id == cateringProjectId ? [] : nil }
        let fanout = threads(now: now).filter { ($0["botId"] as? String) != "preview-bot-a" }.map { thread in
            ["id": thread["id"] ?? "", "title": thread["title"] ?? "", "botId": thread["botId"] ?? "", "status": thread["status"] ?? ""]
        }
        return [
            ["id": "preview-msg-1", "role": "user", "text": projectBrief, "payload": NSNull(), "createdAt": ago(60 * 5 + 2, now: now)],
            [
                "id": "preview-msg-2", "role": "coordinator",
                "text": "Started three threads: Goldie checks prices, Rimraf builds the launch page, and Yessir writes the email and posts once the page is approved.",
                "payload": ["kind": "fanout", "threads": fanout],
                "createdAt": ago(60 * 5, now: now),
            ],
        ]
    }

    // MARK: - Account and computer

    /// `GET /api/v1/me` (`{ user }`).
    static func me(now: Date = Date()) -> [String: Any] {
        [
            "id": userId, "clerk_id": NSNull(), "email": "you@harborbakery.example", "name": userName,
            "avatar_url": NSNull(), "role": "owner", "status": "active",
            "created_at": ago(60 * 24 * 9, now: now), "organization_id": NSNull(), "organization_role": NSNull(),
        ]
    }

    /// `GET /api/v1/workspaces` rows.
    static func workspaces(now: Date = Date()) -> [[String: Any]] {
        [[
            "id": workspaceId, "name": "Harbor Bakery", "slug": "harbor-bakery", "owner_id": userId,
            "description": "A small studio running a bakery’s spring launch.",
            "created_at": ago(60 * 24 * 9, now: now), "updated_at": ago(1, now: now),
        ]]
    }

    /// `GET {cloud}/api/v1/runtime-devices` — one computer, online, holding a
    /// relay connection, so every "Connect a computer" state stays hidden.
    static func runtimeDevices(now: Date = Date()) -> [[String: Any]] {
        [[
            "id": runtimeId, "name": "Harbor Bakery Mac", "runtimeType": "desktop",
            "hostname": "harbor-bakery-mac", "platform": "darwin", "status": "online",
            "lastSeenAt": ago(0, now: now), "preferred": true,
            "relayConnections": [["client": "desktop", "capabilities": ["http"]]],
        ]]
    }

    /// The composer's model pill (`GET /api/v1/models`, bare array).
    /// `GET /api/v1/connectors` for the sample: a few catalog connectors a
    /// bakery would use, none connected yet.
    static var connectors: [[String: Any]] {
        [
            ("gmail", "Gmail", "communication", "Read, draft and send email."),
            ("google-calendar", "Google Calendar", "productivity", "See and schedule events."),
            ("google-drive", "Google Drive", "productivity", "Find and read files in Drive."),
            ("slack", "Slack", "communication", "Read channels and post messages."),
            ("notion", "Notion", "productivity", "Search and update pages."),
            ("stripe", "Stripe", "finance", "Look up payments, customers and invoices."),
        ].map { id, name, category, description in
            ["id": id, "name": name, "category": category, "description": description,
             "connectable": true, "executable": true, "auth_type": "oauth",
             "connection": ["status": "disconnected", "account": NSNull()]]
        }
    }

    static var models: [[String: Any]] { [
        ["id": "allternit/kimi-k3", "name": "Kimi K3", "provider": "allternit", "tier": "flagship", "supports_effort": true],
    ] }
}
