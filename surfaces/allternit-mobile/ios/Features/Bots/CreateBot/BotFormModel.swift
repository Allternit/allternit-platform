import Foundation
import SwiftUI

// -----------------------------------------------------------------------------
// The state and saves behind Create bot and Edit bot, ported from allternit-ai
// src/views/agent-view/components/create-bot: wizard-state.ts (defaults, template
// reset, payload), agent.service.ts (wire shapes), describeBot.ts, EditBotForm.tsx.
//
// Routes: POST agents, PATCH agents/:id (PUT on gateways that refuse it), GET
// agents/:id, GET brains, GET voice/voices, GET computers/quota, the cloud model
// route (GET /v1/models, POST /v1/chat/completions) and the Automation routines
// a template installs paused.
// -----------------------------------------------------------------------------

@MainActor
final class BotFormModel: ObservableObject {
    enum Mode: Equatable {
        case create
        case edit(botId: String)
    }

    struct KnowledgeBrain: Identifiable, Hashable {
        let id: String
        let label: String
    }

    struct Voice: Identifiable, Hashable {
        let id: String
        let label: String
    }

    struct Quota {
        let allowed: Bool
        let reason: String?
        let active: Int
        let activeLimit: Int?
        let monthlyMinutes: Int
        let monthlyLimit: Int?
    }

    let mode: Mode
    let look = BotLookModel()

    // Identity
    @Published var displayName = ""
    @Published var handle = ""
    @Published var tagline = ""
    @Published var category = "custom"
    @Published var accent = "#B08D6E"
    @Published var description = ""
    @Published var welcome = ""
    @Published var starterPrompts: [String] = []
    @Published var owns: [String] = []

    // Job
    @Published var selectedTemplateId: String?
    @Published var systemPrompt = ""
    @Published var allowedTools: [String] = []

    // Computer & runtime
    @Published var desktopEnabled = true
    @Published var desktopPresetId = "small"
    @Published var brainMode = "allternit_cloud"
    @Published var harnessId = "codex"
    @Published var nativeSessionId = ""
    @Published var uhpHarnessId = ""
    @Published var knowledgeBrainId = ""
    @Published var modelId = ""
    @Published var provider = ""
    @Published var harnessMode = "cloud"
    @Published var maxIterations = 10
    @Published var temperature = 0.7
    @Published var voiceEnabled = false
    @Published var voiceId = "default"

    // Loaded
    @Published private(set) var knowledgeBrains: [KnowledgeBrain] = []
    @Published private(set) var brainsLoading = false
    @Published private(set) var voices: [Voice] = []
    @Published private(set) var voicesLoading = false
    @Published private(set) var quota: Quota?

    // Status
    @Published private(set) var loading = false
    @Published private(set) var loadError: String?
    @Published private(set) var saving = false
    @Published var error: String?
    @Published private(set) var describing = false
    @Published private(set) var describeError: String?
    @Published private(set) var refining = false
    @Published private(set) var refineError: String?

    /// The loaded bot's stored profile and config, kept whole so a save never drops a field.
    private var storedProfile: [String: JSONValue] = [:]
    private var storedConfig: [String: JSONValue] = [:]
    /// The chosen template's create body (model filled in), the base of the payload.
    private var templateBody: [String: JSONValue]?
    private var lastApplied: (name: String, description: String, prompt: String)?

    init(mode: Mode) {
        self.mode = mode
        // Edit reads the bot first, so the Look editor opens on what it has saved.
        loading = mode != .create
        if mode == .create {
            let model = MeetTemplateCreate.currentModel()
            modelId = model.model
            provider = model.provider
            look.configure(avatar: nil, accent: accent, displayName: "", vendorId: nil)
        }
    }

    var isEditing: Bool { mode != .create }
    var nameValid: Bool { displayName.trimmingCharacters(in: .whitespacesAndNewlines).count >= 2 }
    var derivedHandle: String { Self.deriveHandle(displayName) }
    var canSave: Bool { nameValid && !saving && !look.busy && !look.blocked && !loading && loadError == nil }
    /// Name — Role is the only thing the form requires; a UHP brain needs its id too (BotBrainBindFields).
    var uhpMissing: Bool { brainMode == "uhp_harness" && uhpHarnessId.trimmingCharacters(in: .whitespaces).isEmpty }

    static func deriveHandle(_ name: String) -> String {
        let lowered = name.lowercased().trimmingCharacters(in: .whitespacesAndNewlines)
        var out = ""
        var pendingDash = false
        for scalar in lowered.unicodeScalars {
            if (scalar.value >= 97 && scalar.value <= 122) || (scalar.value >= 48 && scalar.value <= 57) {
                if pendingDash, !out.isEmpty { out.append("-") }
                pendingDash = false
                out.unicodeScalars.append(scalar)
            } else {
                pendingDash = true
            }
        }
        let trimmed = String(out.prefix(64))
        return trimmed.isEmpty ? "my-bot" : trimmed
    }

    // MARK: Loading

    /// Brains, models, voices (the wizard's load effect). Failures leave a list empty, as on the web.
    func loadRuntimeOptions() async {
        ModelStore.shared.fetchModelsIfNeeded()
        brainsLoading = true
        voicesLoading = true
        async let brainRows = try? ConnectHTTP.call("GET", "brains")
        async let voiceRows = try? ConnectHTTP.call("GET", "voice/voices")
        async let quotaRow = try? ConnectHTTP.call("GET", "computers/quota")
        let (b, v, q) = await (brainRows, voiceRows, quotaRow)
        let brainList = (b?.jArray ?? []).compactMap { row -> KnowledgeBrain? in
            guard let id = row.at("brain_id")?.jString else { return nil }
            return KnowledgeBrain(id: id, label: Self.brainLabel(id: id, cloneURL: row.at("clone_url")?.jString))
        }
        knowledgeBrains = brainList
        // Zero-config: the first brain is preselected, like the wizard.
        if knowledgeBrainId.isEmpty, !isEditing, let first = brainList.first { knowledgeBrainId = first.id }
        brainsLoading = false
        voices = (v?.at("voices")?.jArray ?? []).compactMap { row in
            guard let id = row.at("id")?.jString else { return nil }
            return Voice(id: id, label: row.at("label")?.jString ?? id)
        }
        voicesLoading = false
        if let q {
            func int(_ key: String) -> Int? { if case .number(let n)? = q.at(key) { return Int(n) } else { return nil } }
            quota = Quota(allowed: q.at("allowed")?.jBool ?? true, reason: q.at("reason")?.jString,
                          active: int("active") ?? 0, activeLimit: int("active_limit"),
                          monthlyMinutes: int("monthly_minutes") ?? 0, monthlyLimit: int("monthly_limit"))
        }
    }

    /// ComputerRuntimeStep brainLabel: the clone URL's last two parts, else a short id.
    private static func brainLabel(id: String, cloneURL: String?) -> String {
        if let cloneURL {
            let cleaned = cloneURL.replacingOccurrences(of: ".git", with: "")
            let parts = cleaned.split(separator: "/").suffix(2)
            if parts.count == 2 { return parts.joined(separator: "/") }
        }
        return id.count <= 16 ? id : "\(id.prefix(8))…\(id.suffix(8))"
    }

    /// Edit: the stored row, then the form and the Look editor prefilled from it (formFromBot).
    func loadBot() async {
        guard case .edit(let botId) = mode else { return }
        loading = true
        loadError = nil
        defer { loading = false }
        do {
            let path = "agents/\(botId.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? botId)"
            let response = try await ConnectHTTP.call("GET", path)
            guard let row = response.at("agent") ?? (response.at("id") != nil ? response : nil) else {
                loadError = "This bot couldn’t be read."
                return
            }
            prefill(from: row)
        } catch {
            loadError = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }

    private func prefill(from row: JSONValue) {
        let config = MeetAgentsAPI.object(row.at("config")) ?? [:]
        let profile = MeetAgentsAPI.object(row.at("bot_profile")) ?? MeetAgentsAPI.object(config["botProfile"]) ?? [:]
        storedProfile = profile
        storedConfig = config
        handle = row.at("name")?.jString ?? ""
        displayName = profile["displayName"]?.jString ?? handle
        tagline = profile["tagline"]?.jString ?? ""
        category = profile["botCategory"]?.jString ?? "custom"
        accent = profile["accentColor"]?.jString ?? "#B08D6E"
        welcome = profile["welcomeMessage"]?.jString ?? ""
        starterPrompts = (profile["starterPrompts"]?.jArray ?? []).compactMap(\.jString)
        description = row.at("description")?.jString ?? ""
        systemPrompt = row.at("system_prompt")?.jString ?? ""
        owns = (row.at("owns")?.jArray ?? []).compactMap(\.jString)
        let allowed = (row.at("allowed_tools")?.jArray ?? []).compactMap(\.jString)
        allowedTools = allowed.isEmpty ? (row.at("tools")?.jArray ?? []).compactMap(\.jString) : allowed
        let avatar = MeetAgentsAPI.object(config["avatar"]) ?? MeetAgentsAPI.object(row.at("avatar"))
        let vendorId = profile["vendorPackId"]?.jString
        look.configure(avatar: avatar.map(JSONValue.object), accent: accent, displayName: displayName, vendorId: vendorId)
    }

    // MARK: Editing helpers

    func setAccent(_ color: String) {
        accent = color
        look.push(accent: color)
    }

    func nameChanged() {
        look.push(displayName: displayName)
    }

    func toggleTool(_ id: String) {
        if let index = allowedTools.firstIndex(of: id) { allowedTools.remove(at: index) } else { allowedTools.append(id) }
    }

    func addStarter(_ prompt: String) {
        let trimmed = prompt.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, !starterPrompts.contains(trimmed), starterPrompts.count < BotFormData.starterPromptMax else { return }
        starterPrompts.append(trimmed)
    }

    func addOwn(_ item: String) {
        let trimmed = item.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, !owns.contains(where: { $0.caseInsensitiveCompare(trimmed) == .orderedSame }) else { return }
        owns.append(trimmed)
    }

    /// RoleField chip: the template fills the form, replacing what the last one
    /// wrote and keeping what the person typed (applyTemplateToForm "reset").
    func pick(template: MeetBotTemplate?) {
        guard let template, selectedTemplateId != template.id else {
            selectedTemplateId = nil
            templateBody = nil
            lastApplied = nil
            description = ""
            systemPrompt = ""
            allowedTools = []
            tagline = ""
            welcome = ""
            starterPrompts = []
            category = "custom"
            return
        }
        apply(template, keepTyped: true)
    }

    private func apply(_ template: MeetBotTemplate, keepTyped: Bool) {
        let model = (model: modelId, provider: provider)
        guard case .object(let body)? = template.createBody.map({ Self.fill($0, model) }) else { return }
        let templateProfile = MeetAgentsAPI.object(body["bot_profile"]) ?? [:]
        let name = templateProfile["displayName"]?.jString ?? template.name
        let templateDescription = body["description"]?.jString ?? ""
        let prompt = body["system_prompt"]?.jString ?? ""
        func replaceable(_ current: String, _ last: String?) -> Bool {
            let value = current.trimmingCharacters(in: .whitespacesAndNewlines)
            return value.isEmpty || (keepTyped && value == (last ?? "").trimmingCharacters(in: .whitespacesAndNewlines))
        }
        if replaceable(displayName, lastApplied?.name) {
            displayName = name
            nameChanged()
        }
        if replaceable(description, lastApplied?.description) { description = templateDescription }
        if replaceable(systemPrompt, lastApplied?.prompt) { systemPrompt = prompt }
        if keepTyped || allowedTools.isEmpty {
            allowedTools = (body["allowed_tools"]?.jArray ?? []).compactMap(\.jString)
        }
        tagline = templateProfile["tagline"]?.jString ?? ""
        welcome = templateProfile["welcomeMessage"]?.jString ?? ""
        starterPrompts = (templateProfile["starterPrompts"]?.jArray ?? []).compactMap(\.jString)
        category = templateProfile["botCategory"]?.jString ?? "custom"
        if let color = templateProfile["accentColor"]?.jString {
            setAccent(color)
        } else {
            setAccent(template.color)
        }
        if let enabled = MeetAgentsAPI.object(body["vm_operator"])?["enabled"]?.jBool { desktopEnabled = enabled }
        selectedTemplateId = template.id
        templateBody = body
        lastApplied = (name, templateDescription, prompt)
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

    // MARK: Describe and refine (describeBot.ts)

    /// "Draft instructions from this": one call; what the person wrote stays, the draft fills gaps.
    func draftFromDescription() async {
        guard !describing else { return }
        describing = true
        describeError = nil
        defer { describing = false }
        guard let result = await Self.describeBot(paragraph: description, displayName: nil, currentPrompt: nil) else {
            describeError = "Couldn't prefill — try again."
            return
        }
        if let id = result["suggestedTemplateId"]?.jString, let template = MeetBotCatalog.shared.template(id) {
            apply(template, keepTyped: false)
        }
        if description.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty, let d = result["description"]?.jString { description = d }
        if let prompt = result["systemPrompt"]?.jString, !prompt.isEmpty { systemPrompt = prompt }
        let tools = (result["allowedTools"]?.jArray ?? []).compactMap(\.jString).filter { id in BotFormData.tools.contains { $0.id == id } }
        if !tools.isEmpty { allowedTools = tools }
        if displayName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty, let name = result["displayName"]?.jString {
            displayName = name
            nameChanged()
        }
        if let value = result["tagline"]?.jString { tagline = value }
        if let value = result["welcomeMessage"]?.jString { welcome = value }
        let prompts = (result["starterPrompts"]?.jArray ?? []).compactMap(\.jString).prefix(5)
        if !prompts.isEmpty { starterPrompts = Array(prompts) }
        if let value = result["botCategory"]?.jString, BotFormData.categories.contains(where: { $0.id == value }) { category = value }
    }

    /// "Refine from my description": rewrites the job instructions.
    func refine() async {
        guard !refining else { return }
        refining = true
        refineError = nil
        defer { refining = false }
        let text = description.isEmpty ? tagline : description
        let result = await Self.describeBot(paragraph: text, displayName: displayName.isEmpty ? nil : displayName,
                                            currentPrompt: systemPrompt.isEmpty ? nil : systemPrompt)
        if let prompt = result?["systemPrompt"]?.jString, !prompt.isEmpty {
            systemPrompt = prompt
        } else {
            refineError = "Couldn't refine — try again."
        }
    }

    /// The authenticated cloud model route, JSON forced. Nil on any failure, never throws.
    private static func describeBot(paragraph: String, displayName: String?, currentPrompt: String?) async -> [String: JSONValue]? {
        let wanted = paragraph.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !wanted.isEmpty, let token = (try? await AuthManager.shared.getToken()) ?? nil, !token.isEmpty else { return nil }
        let origin = AppConfig.cloudAPIBaseURL
        let session = URLSession.shared
        var modelsRequest = URLRequest(url: origin.appendingPathComponent("v1/models"))
        modelsRequest.timeoutInterval = 10
        guard let (modelData, _) = try? await session.data(for: modelsRequest),
              let catalog = try? JSONDecoder().decode(JSONValue.self, from: modelData) else { return nil }
        let ids = (catalog.at("data")?.jArray ?? []).compactMap { $0.at("id")?.jString }
        let wantedModel = "kimi/kimi-for-coding"
        let family = wantedModel.split(separator: "/").first.map(String.init) ?? wantedModel
        guard let model = ids.first(where: { $0 == wantedModel }) ?? ids.first(where: { $0.hasPrefix("\(family)-") }) ?? ids.first else { return nil }

        let tools = BotFormData.tools.map(\.id).joined(separator: ", ")
        let templates = MeetBotCatalog.shared.templates.map(\.id).joined(separator: ", ")
        let schema = """
        {
          "displayName": "Name — Role",
          "tagline": "short description for the bot card",
          "botCategory": "one of: research | code | writing | data | sales | design | ops | custom",
          "description": "one or two sentences: what the bot does",
          "systemPrompt": "the bot's job instructions (what it does and does not do)",
          "welcomeMessage": "first message the bot sends in a new session",
          "starterPrompts": ["up to 5 clickable example prompts"],
          "allowedTools": ["subset of: \(tools)"],
          "suggestedTemplateId": "nearest of: \(templates)"
        }
        """
        let system = "You configure bots on a platform. Reply with ONLY a JSON object matching this schema (no prose, no code fences):\n\(schema)\nFill every field from the user's description. Keep every value plain and short. suggestedTemplateId must be the closest catalog template, not the user's words."
        var user = ""
        if let displayName { user += "Current bot name: \(displayName)\n" }
        if let currentPrompt { user += "Current job instructions (improve, keep the intent):\n\(currentPrompt)\n" }
        user += "Describe the bot I want:\n\(wanted)"
        let body: JSONValue = .object([
            "model": .string(model),
            "messages": .array([
                .object(["role": .string("system"), "content": .string(system)]),
                .object(["role": .string("user"), "content": .string(user)]),
            ]),
            "temperature": .number(0.4),
            "max_tokens": .number(1500),
            "stream": .bool(false),
            "response_format": .object(["type": .string("json_object")]),
        ])
        var request = URLRequest(url: origin.appendingPathComponent("v1/chat/completions"))
        request.httpMethod = "POST"
        request.timeoutInterval = 20
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try? JSONEncoder().encode(body)
        guard let (data, response) = try? await session.data(for: request),
              let http = response as? HTTPURLResponse, (200...299).contains(http.statusCode),
              let reply = try? JSONDecoder().decode(JSONValue.self, from: data),
              let content = reply.at("choices")?.jArray.first?.at("message")?.at("content")?.jString else { return nil }
        // Tolerate a code fence despite the instructions.
        var raw = content.trimmingCharacters(in: .whitespacesAndNewlines)
        if raw.hasPrefix("```"), let open = raw.firstIndex(of: "\n"), let close = raw.range(of: "```", options: .backwards), close.lowerBound > open {
            raw = String(raw[raw.index(after: open)..<close.lowerBound])
        }
        guard case .object(let parsed)? = try? JSONDecoder().decode(JSONValue.self, from: Data(raw.utf8)) else { return nil }
        return parsed
    }

    // MARK: Save

    private var botBrain: JSONValue {
        var brain: [String: JSONValue] = ["mode": .string(brainMode)]
        if brainMode == "native_harness" {
            brain["harness"] = .string(harnessId)
            let session = nativeSessionId.trimmingCharacters(in: .whitespaces)
            if !session.isEmpty { brain["nativeSessionId"] = .string(session) }
        } else if brainMode == "uhp_harness" {
            let id = uhpHarnessId.trimmingCharacters(in: .whitespaces)
            if !id.isEmpty { brain["uhpHarnessId"] = .string(id) }
        }
        if !provider.isEmpty, !modelId.isEmpty {
            let prefix = "\(provider)/"
            let cleaned = modelId.hasPrefix(prefix) ? String(modelId.dropFirst(prefix.count)) : modelId
            brain["modelRef"] = .object(["providerID": .string(provider), "modelID": .string(cleaned)])
        }
        return .object(brain)
    }

    private var vmOperator: JSONValue {
        let preset = BotFormData.desktopPresets.first { $0.id == desktopPresetId } ?? BotFormData.desktopPresets[0]
        return .object([
            "enabled": .bool(desktopEnabled),
            "provider": .string("cloud-desktop"),
            "computerKind": .string("cloud_desktop"),
            "persistence": .string("persistent"),
            "resources": .object(["cpu": .string(preset.cpu), "memory": .string(preset.memory), "disk": .string(preset.disk)]),
            "allowedActions": .array(["command", "browser", "file", "desktop"].map(JSONValue.string)),
            "networkPolicy": .string("restricted"),
            "autoStart": .bool(false),
        ])
    }

    /// The avatar the form saves when the Look editor couldn't load: the wizard's Gizzi default.
    private var fallbackAvatar: JSONValue {
        .object([
            "type": .string("mascot"), "mascotTemplate": .string("gizzi"),
            "colors": .object(["primary": .string(accent), "secondary": .string("#ffffff"), "glow": .string(accent)]),
            "currentEmotion": .string("pleased"),
        ])
    }

    /// buildCreateBotPayload + agent.service createAgent.
    private func createBody(name displayName: String, makeMain: Bool) -> [String: JSONValue] {
        var body = templateBody ?? [:]
        let base = BotFormModel.deriveHandle(displayName)
        let handle = base.count >= 3 ? base : "\(base)-bot"
        let tagline = self.tagline.trimmingCharacters(in: .whitespacesAndNewlines)
        let about = description.trimmingCharacters(in: .whitespacesAndNewlines)
        var profile = MeetAgentsAPI.object(body["bot_profile"]) ?? [:]
        profile["displayName"] = .string(displayName)
        profile["tagline"] = .string(tagline)
        profile["welcomeMessage"] = .string(welcome)
        profile["starterPrompts"] = .array(starterPrompts.map(JSONValue.string))
        profile["accentColor"] = .string(accent)
        if profile["groupChatEnabled"] == nil { profile["groupChatEnabled"] = .bool(true) }
        profile["botCategory"] = .string(category)
        if makeMain { profile["chiefOfStaff"] = .bool(true) }

        let avatar = look.avatar ?? fallbackAvatar
        var config = MeetAgentsAPI.object(body["config"]) ?? [:]
        config["avatar"] = avatar
        config["isBot"] = .bool(true)
        config["botProfile"] = .object(profile)
        config["botBrain"] = botBrain
        config["vmOperator"] = vmOperator
        if !knowledgeBrainId.isEmpty { config["brainId"] = .string(knowledgeBrainId) } else { config.removeValue(forKey: "brainId") }

        body["name"] = .string(handle)
        body["description"] = .string(tagline.isEmpty ? (about.isEmpty ? "\(displayName) is a custom Allternit bot." : about) : tagline)
        if body["type"] == nil { body["type"] = .string("worker") }
        body["model"] = .string(modelId)
        body["provider"] = .string(provider)
        if body["capabilities"] == nil { body["capabilities"] = .array([]) }
        body["system_prompt"] = .string(systemPrompt)
        if body["tools"] == nil { body["tools"] = .array([]) }
        body["max_iterations"] = .number(Double(maxIterations))
        body["temperature"] = .number(temperature)
        body["config"] = .object(config)
        if let data = try? JSONEncoder().encode(avatar) { body["avatar"] = .string(String(decoding: data, as: UTF8.self)) }
        if body["character_json"] == nil { body["character_json"] = Self.defaultCharacterLayer(name: handle, domain: category, displayName: displayName) }
        if body["trust_tier"] == nil { body["trust_tier"] = .string("standard") }
        body["harness_config"] = .object(["mode": .string(harnessMode)])
        if body["enabled_modes"] == nil { body["enabled_modes"] = .array([.string("chat")]) }
        if body["allowed_skills"] == nil { body["allowed_skills"] = .array([]) }
        body["allowed_tools"] = .array(allowedTools.map(JSONValue.string))
        if body["category"] == nil { body["category"] = .string("general") }
        if body["tags"] == nil { body["tags"] = .array([]) }
        if body["data_classification"] == nil { body["data_classification"] = .string("internal") }
        if body["write_scope"] == nil { body["write_scope"] = .string("workspace") }
        body["is_bot"] = .bool(true)
        body["bot_profile"] = .object(profile)
        body["vm_operator"] = vmOperator
        return body
    }

    /// buildDefaultCharacterLayer.
    private static func defaultCharacterLayer(name: String, domain: String, displayName: String) -> JSONValue {
        let empty: JSONValue = .array([])
        return .object([
            "identity": .object([
                "setup": .string("generalist"), "className": .string(displayName), "specialtySkills": empty,
                "temperament": .string("balanced"), "personalityTraits": empty, "backstory": .string(""),
            ]),
            "roleCard": .object([
                "domain": .string(domain.isEmpty ? name : domain), "inputs": empty, "outputs": empty,
                "definitionOfDone": empty, "hardBans": empty, "escalation": empty, "metrics": empty,
            ]),
            "voice": .object([
                "style": .string(""), "rules": empty, "microBans": empty,
                "tone": .object(["formality": .number(0.5), "enthusiasm": .number(0.5), "empathy": .number(0.5), "directness": .number(0.5)]),
            ]),
            "progression": .object([
                "class": .string("Generalist"), "relevantStats": empty,
                "level": .object(["maxLevel": .number(99), "xpFormula": .string("linear")]),
            ]),
            "avatar": .object([
                "type": .string("mascot"), "mascot": .object(["template": .string("bot")]),
                "style": .object(["primaryColor": .string("#B08D6E"), "accentColor": .string("#1e1c1a")]),
            ]),
        ])
    }

    /// Create bot: the bot, a template's routines (paused), the avatar asset. The caller opens its chat.
    func create() async -> BotSetupOutcome? {
        guard canSave, !uhpMissing else {
            if uhpMissing { error = "Add the UHP harness id, or pick another brain." }
            return nil
        }
        saving = true
        error = nil
        defer { saving = false }
        let name = displayName.trimmingCharacters(in: .whitespacesAndNewlines)
        do {
            let makeMain = await BotSetupMaker.needsMain()
            let botId = try await MeetAgentsAPI.create(body: createBody(name: name, makeMain: makeMain))
            // The create route takes no voice (agent.service): it goes on right after, best effort.
            if voiceEnabled {
                try? await MeetAgentsAPI.update(id: botId, body: ["voice": .object(["voice_id": .string(voiceId), "enabled": .bool(true)])])
            }
            if let id = selectedTemplateId, let template = MeetBotCatalog.shared.template(id) {
                for routine in template.routines where routine.cron != nil || routine.event != nil {
                    try? await MeetFinish.installRoutine(botId: botId, botName: name, title: routine.label, prompt: routine.prompt,
                                                         cron: routine.cron, event: routine.event, enabled: false)
                }
            }
            await look.syncAsset(botId: botId)
            AgentHubStore.shared.fetchAgentsIfNeeded(force: true)
            if makeMain { Task { await MainBotReachOut.run(botId: botId, botName: name) } }
            return BotSetupOutcome(botIds: [botId], mainBotId: makeMain ? botId : nil, summary: "\(name) created")
        } catch {
            self.error = "Couldn’t create the bot: \(FriendlyErrorMessage.from(error.localizedDescription))"
            return nil
        }
    }

    /// Edit bot (EditBotForm handleSave): identity, job, owns and look. An untouched look stays as saved.
    func saveEdit() async -> Bool {
        guard case .edit(let botId) = mode, canSave else {
            if !nameValid { error = "Add a display name (at least 2 characters) to continue." }
            return false
        }
        saving = true
        error = nil
        defer { saving = false }
        let name = displayName.trimmingCharacters(in: .whitespacesAndNewlines)
        var profile = storedProfile
        profile["displayName"] = .string(name)
        profile["tagline"] = .string(tagline)
        profile["welcomeMessage"] = .string(welcome)
        profile["starterPrompts"] = .array(starterPrompts.map(JSONValue.string))
        profile["accentColor"] = .string(accent)
        profile["botCategory"] = .string(category)
        var config = storedConfig
        config["botProfile"] = .object(profile)
        if look.dirty, let avatar = look.avatar { config["avatar"] = avatar }
        let cleanTagline = tagline.trimmingCharacters(in: .whitespacesAndNewlines)
        let cleanDescription = description.trimmingCharacters(in: .whitespacesAndNewlines)
        var body: [String: JSONValue] = [
            "owns": .array(owns.map(JSONValue.string)),
            "system_prompt": .string(systemPrompt),
            "allowed_tools": .array(allowedTools.map(JSONValue.string)),
            "bot_profile": .object(profile),
            "config": .object(config),
        ]
        let about = cleanTagline.isEmpty ? cleanDescription : cleanTagline
        if !about.isEmpty { body["description"] = .string(about) }
        do {
            try await MeetAgentsAPI.update(id: botId, body: body)
            _ = try? await AgentHubStore.shared.refreshAgent(id: botId)
            return true
        } catch {
            self.error = "Failed to update bot. \(FriendlyErrorMessage.from(error.localizedDescription))"
            return false
        }
    }
}
