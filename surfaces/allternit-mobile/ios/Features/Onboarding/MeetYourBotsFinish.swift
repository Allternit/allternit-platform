import Foundation

// -----------------------------------------------------------------------------
// What "Start the job" really does at the end of Meet your bots — the port of
// allternit-ai `src/components/bots/meet/meet-finish.ts` plus the main-bot
// rules of `src/views/bots/rail/main-bot.ts` (findMainBot / setMainBot /
// setUpPrimaryBot / dressBot). Set the main bot, create the picked team bots
// (skipping ones already on the account), install the kept routines, start
// thread #1 with the first job. Every item reports its own real result; one
// failure never blocks the rest, and a retry only redoes what failed.
// -----------------------------------------------------------------------------

/// The main bot's key in routine drafts and the job ("main"); team bots use their template id.
let meetMainKey = "main"

enum MeetEntry: Equatable {
    /// First run with no main bot yet: start → team → main → routines → notifications → job → done.
    case setup
    /// A main bot already exists: the main step is skipped.
    case botmode
}

/// Who leads the team, as picked in the main step.
enum MeetMainChoice: Equatable {
    /// A:// as it comes.
    case primary
    /// A:// dressed in one of the cast looks, with the name typed there.
    case look(id: String, name: String)
    /// A bot the person already has.
    case bot(id: String)
}

enum MeetItemStatus: Equatable {
    case pending, running, ok, skipped, failed
}

struct MeetFinishItem: Identifiable, Equatable {
    let key: String
    var label: String
    var status: MeetItemStatus
    /// What happened, in plain words (a failure says why).
    var detail: String?

    var id: String { key }
}

/// One routine in the draft: nothing is saved until the job starts.
struct MeetRoutineDraft: Identifiable, Equatable {
    /// "<templateId>:<routineId>".
    let id: String
    /// `meetMainKey` or a template id.
    let botKey: String
    let title: String
    let prompt: String
    let cron: String
    let scheduleLabel: String
    var enabled: Bool

    static func drafts(for template: MeetBotTemplate) -> [MeetRoutineDraft] {
        template.visibleRoutines.compactMap { r in
            guard let cron = r.cron, let label = r.scheduleLabel else { return nil }
            return MeetRoutineDraft(id: "\(template.id):\(r.id)", botKey: template.id, title: r.title,
                                    prompt: r.prompt, cron: cron, scheduleLabel: label, enabled: true)
        }
    }
}

struct MeetFinishInput {
    var entry: MeetEntry
    /// Template ids on the team.
    var members: [String]
    var main: MeetMainChoice
    var routines: [MeetRoutineDraft]
    /// The first job: the bot that owns it (`meetMainKey` or a template id) and its text.
    var job: (botKey: String, text: String)?
}

/// What already happened, so a retry doesn't redo it.
final class MeetFinishState {
    var mainBotId: String?
    var mainName = ""
    /// Template id → bot id, for created and already-existing bots.
    var botIds: [String: String] = [:]
    /// Template ids created now (not already on the account).
    var created: Set<String> = []
    var routinesDone: Set<String> = []
    var hiddenDone: Set<String> = []
    var sessionId: String?
    var sent = false

    /// The bot that owns thread #1, once it started (to open its chat afterwards).
    func startedThreadBotId(for input: MeetFinishInput) -> String? {
        guard sent, sessionId != nil, let job = input.job else { return nil }
        return job.botKey == meetMainKey ? mainBotId : botIds[job.botKey]
    }
}

// MARK: - Roster (raw agent rows)

/// A bot as the main-bot rules need it: raw `bot_profile` / `config`, so a
/// save never drops keys this app doesn't model (vendorPackId, avatar, …).
struct MeetRosterBot: Sendable {
    let id: String
    let name: String
    let profile: [String: JSONValue]
    let config: [String: JSONValue]
    let enabledModes: [String]

    var displayName: String {
        if case .string(let s)? = profile["displayName"], !s.trimmingCharacters(in: .whitespaces).isEmpty { return s }
        return name
    }

    var templateId: String? {
        if case .string(let s)? = config["templateId"], !s.isEmpty { return s }
        return nil
    }

    /// web bot-roster-preview `isChiefOfStaff`: `botProfile.chiefOfStaff === true`.
    var isMain: Bool {
        if case .bool(true)? = profile["chiefOfStaff"] { return true }
        if case .object(let mirrored)? = config["botProfile"], case .bool(true)? = mirrored["chiefOfStaff"] { return true }
        return false
    }

    /// API surface gate: `enabled_modes` holds the surface or "all".
    func allowed(on surface: String) -> Bool {
        enabledModes.contains(surface) || enabledModes.contains("all")
    }
}

/// The raw agent routes the meet flow needs (`/api/v1/agents`, agent_routes.rs),
/// with the web agent.service's wire shapes.
enum MeetAgentsAPI {
    private struct ListEnvelope: Decodable { let agents: [[String: JSONValue]] }
    private struct CreateResponse: Decodable {
        struct Ref: Decodable { let id: String }
        let agent: Ref
    }

    static func roster() async throws -> [MeetRosterBot] {
        let envelope: ListEnvelope = try await APIClient.shared.get(path: "agents")
        return envelope.agents.compactMap { row in
            guard case .string(let id)? = row["id"] else { return nil }
            let config = object(row["config"]) ?? [:]
            let profile = object(row["bot_profile"]) ?? object(config["botProfile"])
            var isBot = profile != nil
            if case .bool(true)? = row["is_bot"] { isBot = true }
            if case .bool(true)? = config["isBot"] { isBot = true }
            guard isBot else { return nil }
            var name = id
            if case .string(let n)? = row["name"] { name = n }
            return MeetRosterBot(id: id, name: name, profile: profile ?? [:], config: config,
                                 enabledModes: strings(row["enabled_modes"]) ?? ["chat"])
        }
    }

    /// `POST /api/v1/agents` → the new id. A 422 retries once with the web's
    /// minimal bot body (agent.service createAgent).
    static func create(body: [String: JSONValue]) async throws -> String {
        do {
            let response: CreateResponse = try await APIClient.shared.post(path: "agents", body: body)
            return response.agent.id
        } catch APIError.httpError(statusCode: let status, message: _) where status == 422 {
            var minimal: [String: JSONValue] = [:]
            for key in ["name", "description", "type", "model", "provider", "harness_config", "enabled_modes", "trust_tier", "is_bot", "bot_profile"] {
                if let value = body[key] { minimal[key] = value }
            }
            var config: [String: JSONValue] = ["isBot": .bool(true)]
            if let profile = body["bot_profile"] { config["botProfile"] = profile }
            if case .object(let full)? = body["config"], let avatar = full["avatar"] { config["avatar"] = avatar }
            minimal["config"] = .object(config)
            let response: CreateResponse = try await APIClient.shared.post(path: "agents", body: minimal)
            return response.agent.id
        }
    }

    /// web agent.service `updateAgent`: PATCH (merge), PUT on gateways that refuse it.
    static func update(id: String, body: [String: JSONValue]) async throws {
        let path = "agents/\(id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id)"
        do {
            try await APIClient.shared.patch(path: path, body: body)
        } catch APIError.httpError(statusCode: let status, message: _) where [404, 405, 422].contains(status) {
            try await APIClient.shared.put(path: path, body: body)
        }
    }

    static func object(_ value: JSONValue?) -> [String: JSONValue]? {
        switch value {
        case .object(let dict)?: return dict
        case .string(let text)?:
            guard text.trimmingCharacters(in: .whitespaces).hasPrefix("{"),
                  case .object(let dict)? = try? JSONDecoder().decode(JSONValue.self, from: Data(text.utf8)) else { return nil }
            return dict
        default: return nil
        }
    }

    private static func strings(_ value: JSONValue?) -> [String]? {
        var items: [JSONValue]?
        switch value {
        case .array(let a)?: items = a
        case .string(let text)?:
            if case .array(let a)? = try? JSONDecoder().decode(JSONValue.self, from: Data(text.utf8)) { items = a }
        default: break
        }
        let out = items?.compactMap { item -> String? in if case .string(let s) = item { return s } else { return nil } }
        return (out?.isEmpty ?? true) ? nil : out
    }
}

// MARK: - Main bot (main-bot.ts)

@MainActor
enum MeetMainBot {
    static func find(_ bots: [MeetRosterBot]) -> MeetRosterBot? { bots.first(where: \.isMain) }

    /// The bot's profile with `chiefOfStaff` set, displayName kept (web `withChief`).
    private static func withChief(_ bot: MeetRosterBot, _ on: Bool) -> [String: JSONValue] {
        var profile = bot.profile
        profile["displayName"] = .string(bot.displayName)
        profile["chiefOfStaff"] = .bool(on)
        var config = bot.config
        config["botProfile"] = .object(profile)
        return ["bot_profile": .object(profile), "config": .object(config)]
    }

    /// Make `botId` the main bot, clearing the flag on every other bot first,
    /// and enable it for Cowork (Cowork runs as the main bot; bots are made chat-only).
    static func set(_ botId: String, in bots: [MeetRosterBot]) async throws {
        for bot in bots where bot.id != botId && bot.isMain {
            try await MeetAgentsAPI.update(id: bot.id, body: withChief(bot, false))
        }
        guard let target = bots.first(where: { $0.id == botId }) else { return }
        let needsCowork = !target.allowed(on: "cowork")
        guard !target.isMain || needsCowork else { return }
        var body = withChief(target, true)
        if needsCowork {
            var modes = target.enabledModes.isEmpty ? ["chat"] : target.enabledModes
            if !modes.contains("cowork") { modes.append("cowork") }
            body["enabled_modes"] = .array(modes.map { .string($0) })
        }
        try await MeetAgentsAPI.update(id: botId, body: body)
    }

    /// Set up A:// as the main bot: reuse the A:// bot when there is one,
    /// otherwise create it from the primary template (its main chat opens
    /// with its greeting), dress it when a look was picked, make it main.
    static func setUpPrimary(name: String?, avatar: JSONValue?, dress: Bool, bots: [MeetRosterBot]) async throws -> String {
        let catalog = MeetBotCatalog.shared
        let name = name?.trimmingCharacters(in: .whitespacesAndNewlines).nilIfEmpty
        var roster = bots
        let botId: String
        if let existing = bots.first(where: { $0.templateId == catalog.primary.id }) {
            botId = existing.id
        } else {
            botId = try await MeetTemplateCreate.create(catalog.primary, name: name, makeMain: false)
            if let record = try? await AgentHubStore.shared.refreshAgent(id: botId) {
                _ = try? await BotModeStore.shared.mainChatSessionId(for: record)
            }
        }
        if dress {
            roster = (try? await MeetAgentsAPI.roster()) ?? roster
            if let bot = roster.first(where: { $0.id == botId }) {
                try await dressBot(bot, name: name, avatar: avatar)
                roster = (try? await MeetAgentsAPI.roster()) ?? roster
            }
        } else if !roster.contains(where: { $0.id == botId }) {
            roster = (try? await MeetAgentsAPI.roster()) ?? roster
        }
        try await set(botId, in: roster)
        return botId
    }

    /// Name a bot and put a picked look on it (`config.avatar`, what Edit
    /// bot writes and avatars read first). Saves nothing when nothing changes.
    static func dressBot(_ bot: MeetRosterBot, name: String?, avatar: JSONValue?) async throws {
        var profile = bot.profile
        var config = bot.config
        var changed = false
        if let name, name != bot.displayName {
            profile["displayName"] = .string(name)
            config["botProfile"] = .object(profile)
            changed = true
        }
        if let avatar {
            config["avatar"] = avatar
            changed = true
        }
        guard changed else { return }
        try await MeetAgentsAPI.update(id: bot.id, body: ["bot_profile": .object(profile), "config": .object(config)])
    }
}

// MARK: - Template → bot (templates/apply.ts useBotTemplate)

@MainActor
enum MeetTemplateCreate {
    /// The model a template bot starts on: the app's current model, like the
    /// web's `templateModel()` (runtime id "provider/model"); the web's
    /// default when none is picked yet.
    static func currentModel() -> (model: String, provider: String) {
        guard let id = ModelStore.shared.selectedModelId?.trimmingCharacters(in: .whitespaces), !id.isEmpty else {
            return ("kimi/kimi-for-coding", "custom")
        }
        guard let slash = id.firstIndex(of: "/"), slash != id.startIndex else { return (id, "allternit") }
        return (String(id[id.index(after: slash)...]), String(id[..<slash]))
    }

    /// Creates the bot from the template's web create body; returns its id.
    static func create(_ template: MeetBotTemplate, name: String? = nil, makeMain: Bool) async throws -> String {
        guard case .object(var body)? = template.createBody.map({ fill($0, currentModel()) }) else {
            throw MeetFinishError("This bot template can’t be created on this device.")
        }
        if let name, name != template.name {
            body = renamed(body, to: name, template: template)
        }
        if makeMain, case .object(var profile)? = body["bot_profile"] {
            profile["chiefOfStaff"] = .bool(true)
            body["bot_profile"] = .object(profile)
            if case .object(var config)? = body["config"] {
                config["botProfile"] = .object(profile)
                body["config"] = .object(config)
            }
        }
        return try await MeetAgentsAPI.create(body: body)
    }

    private static func fill(_ value: JSONValue, _ model: (model: String, provider: String)) -> JSONValue {
        switch value {
        case .string("__MODEL__"): return .string(model.model)
        case .string("__PROVIDER__"): return .string(model.provider)
        case .array(let items): return .array(items.map { fill($0, model) })
        case .object(let dict): return .object(dict.mapValues { fill($0, model) })
        default: return value
        }
    }

    /// `botTemplateToFormData(template, { name })`: the display name, the
    /// welcome that says it, and the handle derived from it.
    private static func renamed(_ body: [String: JSONValue], to name: String, template: MeetBotTemplate) -> [String: JSONValue] {
        var body = body
        guard case .object(var profile)? = body["bot_profile"] else { return body }
        profile["displayName"] = .string(name)
        if let welcome = template.welcome, !welcome.isEmpty {
            profile["welcomeMessage"] = .string(welcome.replacingOccurrences(of: "{name}", with: name))
        }
        body["bot_profile"] = .object(profile)
        if case .object(var config)? = body["config"] {
            config["botProfile"] = .object(profile)
            body["config"] = .object(config)
        }
        let handle = BotModeStore.slugify(name)
        body["name"] = .string(handle.count >= 3 ? handle : "\(handle.isEmpty ? "my" : handle)-bot")
        return body
    }
}

struct MeetFinishError: LocalizedError {
    let message: String
    init(_ message: String) { self.message = message }
    var errorDescription: String? { message }
}

// MARK: - The run (meet-finish.ts runFinish)

@MainActor
enum MeetFinish {
    /// The items a run reports, all pending (the done step lists them while it works).
    static func plannedItems(_ input: MeetFinishInput) -> [MeetFinishItem] {
        let catalog = MeetBotCatalog.shared
        var items = [MeetFinishItem(key: "main", label: "Main bot", status: .pending)]
        for id in input.members {
            items.append(MeetFinishItem(key: "bot:\(id)", label: catalog.template(id)?.name ?? id, status: .pending))
        }
        let hidden = input.members.contains { !(catalog.template($0)?.hiddenRoutines.isEmpty ?? true) }
        if !input.routines.isEmpty || hidden {
            items.append(MeetFinishItem(key: "routines", label: "Routines", status: .pending))
        }
        if input.job != nil { items.append(MeetFinishItem(key: "thread", label: "Thread #1", status: .pending)) }
        return items
    }

    static func run(
        _ input: MeetFinishInput,
        state: MeetFinishState,
        prior: [MeetFinishItem]? = nil,
        report: @escaping @MainActor ([MeetFinishItem]) -> Void
    ) async -> [MeetFinishItem] {
        let catalog = MeetBotCatalog.shared
        var items = (prior?.isEmpty == false) ? prior! : plannedItems(input)
        func set(_ key: String, _ status: MeetItemStatus, label: String? = nil, detail: String? = nil) {
            if let i = items.firstIndex(where: { $0.key == key }) {
                items[i].status = status
                items[i].detail = detail
                if let label { items[i].label = label }
            } else {
                items.append(MeetFinishItem(key: key, label: label ?? key, status: status, detail: detail))
            }
            report(items)
        }
        func todo(_ key: String) -> Bool {
            let status = items.first(where: { $0.key == key })?.status
            return status != .ok && status != .skipped
        }
        func message(_ error: Error) -> String {
            FriendlyErrorMessage.from(error.localizedDescription)
        }

        // The roster must load before anything decides "no A:// yet" or
        // "not on the account": otherwise a slow computer makes duplicates.
        var roster: [MeetRosterBot]
        do {
            roster = try await MeetAgentsAPI.roster()
        } catch {
            let why = "Couldn’t reach your bots (\(message(error))). Check that your computer is online, then retry."
            for item in items where item.status != .ok && item.status != .skipped {
                set(item.key, .failed, detail: why)
            }
            return items
        }

        // ── main bot ──
        if todo("main") {
            set("main", .running)
            do {
                let mainId: String
                switch (input.entry, input.main) {
                case (.botmode, _):
                    guard let main = MeetMainBot.find(roster) else {
                        throw MeetFinishError("No main bot found. Set one up from the Bots rail, then try again.")
                    }
                    mainId = main.id
                case (.setup, .bot(let id)):
                    try await MeetMainBot.set(id, in: roster)
                    mainId = id
                case (.setup, .look(let lookId, let name)):
                    let avatar = catalog.looks.first(where: { $0.id == lookId })?.avatar
                    mainId = try await MeetMainBot.setUpPrimary(name: name, avatar: avatar, dress: true, bots: roster)
                case (.setup, .primary):
                    mainId = try await MeetMainBot.setUpPrimary(name: nil, avatar: nil, dress: false, bots: roster)
                }
                roster = (try? await MeetAgentsAPI.roster()) ?? roster
                state.mainBotId = mainId
                state.mainName = roster.first(where: { $0.id == mainId })?.displayName ?? "A://"
                set("main", .ok, label: "Main bot · \(state.mainName)",
                    detail: input.entry == .botmode ? "Already your main bot" : "Pinned to the top of the Bots rail")
            } catch {
                set("main", .failed, detail: message(error))
            }
        }

        // ── team bots ──
        for id in input.members {
            let key = "bot:\(id)"
            guard todo(key) else { continue }
            guard let template = catalog.template(id) else {
                set(key, .failed, detail: "This bot template isn’t in the catalog.")
                continue
            }
            set(key, .running)
            if let existing = roster.first(where: { $0.templateId == id }) {
                state.botIds[id] = existing.id
                set(key, .skipped, detail: "Already on your account")
                continue
            }
            do {
                // The first bot made becomes main when there is none (agent.store createAgent).
                let makeMain = !roster.contains(where: \.isMain) && state.mainBotId == nil
                let botId = try await MeetTemplateCreate.create(template, makeMain: makeMain)
                state.botIds[id] = botId
                state.created.insert(id)
                set(key, .ok, detail: "Added to your bots")
            } catch {
                set(key, .failed, detail: message(error))
            }
        }
        if !state.created.isEmpty { roster = (try? await MeetAgentsAPI.roster()) ?? roster }

        func botId(for key: String) -> String? { key == meetMainKey ? state.mainBotId : state.botIds[key] }
        func name(of botId: String) -> String { roster.first(where: { $0.id == botId })?.displayName ?? "Bot" }

        // ── routines: the kept drafts, then each new bot's unseen template routines (paused) ──
        if items.contains(where: { $0.key == "routines" }) && todo("routines") {
            set("routines", .running)
            var problems: [String] = []
            var added = 0
            for r in input.routines {
                if state.routinesDone.contains(r.id) { added += 1; continue }
                guard let owner = botId(for: r.botKey) else { problems.append("\(r.title): its bot isn’t set up"); continue }
                do {
                    try await installRoutine(botId: owner, botName: name(of: owner), title: r.title, prompt: r.prompt,
                                             cron: r.cron, event: nil, enabled: r.enabled)
                    state.routinesDone.insert(r.id)
                    added += 1
                } catch {
                    problems.append("\(r.title): \(message(error))")
                }
            }
            for id in state.created.sorted() {
                guard let template = catalog.template(id), let owner = state.botIds[id] else { continue }
                for r in template.hiddenRoutines {
                    let k = "\(id):\(r.id)"
                    guard !state.hiddenDone.contains(k) else { continue }
                    do {
                        try await installRoutine(botId: owner, botName: name(of: owner), title: r.label, prompt: r.prompt,
                                                 cron: r.cron, event: r.event, enabled: false)
                        state.hiddenDone.insert(k)
                    } catch {
                        problems.append("\(r.label): \(message(error))")
                    }
                }
            }
            let on = input.routines.filter { $0.enabled && state.routinesDone.contains($0.id) }.count
            if problems.isEmpty {
                set("routines", .ok, detail: "\(on) on" + (added > on ? ", \(added - on) paused" : ""))
            } else {
                let shown = problems.prefix(2).joined(separator: "; ") + (problems.count > 2 ? "…" : "")
                set("routines", .failed, detail: "\(added) added, \(problems.count) didn’t: \(shown)")
            }
        }

        // ── thread #1: the first job, sent as the person's own message, in the bot's main chat ──
        if let job = input.job, items.contains(where: { $0.key == "thread" }), todo("thread") {
            set("thread", .running)
            do {
                guard let owner = botId(for: job.botKey) else {
                    throw MeetFinishError("The bot that owns this job isn’t set up.")
                }
                if state.sessionId == nil {
                    let record = try await AgentHubStore.shared.refreshAgent(id: owner)
                    state.sessionId = try await BotModeStore.shared.mainChatSessionId(for: record)
                }
                if !state.sent, let sessionId = state.sessionId {
                    try await BotModeClient.shared.sendUserMessage(
                        sessionId: sessionId, text: job.text.trimmingCharacters(in: .whitespacesAndNewlines)
                    )
                    state.sent = true
                }
                set("thread", .ok, label: "Thread #1 · \(name(of: owner))", detail: "Running")
            } catch {
                set("thread", .failed, detail: message(error))
            }
        }

        AgentHubStore.shared.fetchAgentsIfNeeded(force: true)
        return items
    }

    /// web bot-routine.service `installTemplateRoutine`: an Automation routine
    /// with `agent_id` = the bot, run by the computer's local scheduler as a
    /// real turn in the bot's main thread. Paused routines are created, then
    /// paused (removed again if the pause fails), so they never fire between.
    static func installRoutine(botId: String, botName: String, title: String, prompt: String,
                               cron: String?, event: String?, enabled: Bool) async throws {
        var config = ["jobType": "agent", "prompt": prompt]
        if cron == nil, let event { config["trigger"] = event }
        let request = CreateAutomationRoutineRequest(
            name: "[bot:\(botName)] \(title)",
            description: prompt,
            agentId: botId,
            scheduleType: cron == nil ? "manual" : "cron",
            scheduleExpression: cron ?? "",
            timezone: TimeZone.current.identifier,
            executionDomain: "local",
            config: config,
            metadata: [
                "source": .string("bot-routine"), "botName": .string(botName),
                "title": .string(title), "fromTemplate": .bool(true),
            ]
        )
        let row = try await AutomationClient.shared.createRoutine(request)
        guard !enabled else { return }
        do {
            try await AutomationClient.shared.setEnabled(id: row.id, enabled: false)
        } catch {
            try? await AutomationClient.shared.deleteRoutine(id: row.id)
            throw error
        }
    }
}

private extension String {
    var nilIfEmpty: String? { isEmpty ? nil : self }
}
