import Foundation

// -----------------------------------------------------------------------------
// Full agent registry row — the hub's AND the composer pill's model
// (AgentModeStore caches these). Field names mirror the serde names of `AgentRow` in
// cmd/allternit-api/src/agent_routes.rs:128-165 exactly; the JSON columns
// (capabilities/tools/config/harness_config/…) the hub doesn't render are
// simply not decoded.
// -----------------------------------------------------------------------------

/// One full agents row (`GET /api/v1/agents` / `GET /api/v1/agents/:id`).
/// `enabledModes` tolerates the column arriving as a JSON-encoded string,
/// same as AgentSummary (agent_routes.rs:1041-1042 defaults it to
/// ["chat"] when unparseable).
struct AgentRecord: Decodable, Sendable, Identifiable, Equatable, Hashable {
    let id: String
    let name: String
    let description: String?
    /// The `type` column ("worker" / "companion" / …, serde-renamed).
    let type: String
    let parentAgentId: String?
    let model: String
    let provider: String
    let systemPrompt: String?
    let status: String
    let workspaceId: String?
    let avatar: String?
    let trustTier: String
    let enabledModes: [String]
    let category: String?
    /// Agent mode: "primary" | "subagent" | "orchestrator" | "council".
    let mode: String
    let isPrimary: Bool
    let createdAt: String
    let updatedAt: String
    let lastRunAt: String?
    /// The `config` JSON column — free-form, no fixed backend schema
    /// (agent_routes.rs's `UpdateAgentBody`/`CreateAgentBody` pass it through
    /// opaquely). `greeting`/`suggestedPrompts` below read known keys out of
    /// it; unknown keys are kept in `config` itself so an update round-trip
    /// (full-replace COALESCE on the backend, not a JSON merge) doesn't
    /// silently drop fields this client doesn't know about.
    let config: [String: JSONValue]?

    /// Bot/runtime fields. The backend currently returns `harness_config`
    /// top-level; the other fields may arrive top-level (future backend) or
    /// nested inside `config` (current web parity path). Each has a computed
    /// fallback to `config` so the UI can render them regardless of where the
    /// server puts them.
    let harnessConfig: HarnessConfig?
    let topLevelBotProfile: BotProfile?
    let topLevelConnectorBindings: [ConnectorBinding]?
    let topLevelSecretRefs: [SecretRef]?
    let topLevelIdentityChannels: IdentityChannels?
    let topLevelVMOperator: VMOperatorConfig?

    enum CodingKeys: String, CodingKey {
        case id, name, description, type, model, provider, status, avatar, category, mode, config
        case parentAgentId = "parent_agent_id"
        case systemPrompt = "system_prompt"
        case workspaceId = "workspace_id"
        case trustTier = "trust_tier"
        case enabledModes = "enabled_modes"
        case isPrimary = "is_primary"
        case createdAt = "created_at"
        case updatedAt = "updated_at"
        case lastRunAt = "last_run_at"
        case harnessConfig = "harness_config"
        case topLevelBotProfile = "bot_profile"
        case topLevelConnectorBindings = "connector_bindings"
        case topLevelSecretRefs = "secret_refs"
        case topLevelIdentityChannels = "identity_channels"
        case topLevelVMOperator = "vm_operator"
    }

    /// Optional first-message greeting for a fresh chat with this agent
    /// (`config.greeting`) — PocketPal-Pal-inspired, shown only when the
    /// agent is explicitly selected, per Eoj (the generic "suggestion rows
    /// on every empty chat" pattern was deliberately removed elsewhere).
    var greeting: String? {
        if case .string(let value)? = config?["greeting"], !value.isEmpty { return value }
        return nil
    }

    /// Quick-start prompt chips shown alongside the greeting
    /// (`config.suggested_prompts`, array of strings).
    var suggestedPrompts: [String] {
        guard case .array(let items)? = config?["suggested_prompts"] else { return [] }
        return items.compactMap { if case .string(let s) = $0 { return s } else { return nil } }
    }

    // MARK: - Bot / runtime fields (top-level or config fallback)

    var botProfile: BotProfile? {
        if let topLevelBotProfile { return topLevelBotProfile }
        return configValue(for: "botProfile")
    }

    var connectorBindings: [ConnectorBinding] {
        if let topLevelConnectorBindings { return topLevelConnectorBindings }
        return configArray(for: "connectorBindings") ?? []
    }

    var secretRefs: [SecretRef] {
        if let topLevelSecretRefs { return topLevelSecretRefs }
        return configArray(for: "secretRefs") ?? []
    }

    var identityChannels: IdentityChannels? {
        if let topLevelIdentityChannels { return topLevelIdentityChannels }
        return configValue(for: "identityChannels")
    }

    var vmOperator: VMOperatorConfig? {
        if let topLevelVMOperator { return topLevelVMOperator }
        return configValue(for: "vmOperator")
    }

    var harness: HarnessConfig? {
        if let harnessConfig { return harnessConfig }
        return configValue(for: "harness")
    }

    /// True for packaged bots (openmausbot / Bot Home parity). Mirrors web
    /// `isBot`: an agent is treated as a bot when it has a bot profile or
    /// explicit `isBot` flag in config.
    var isBot: Bool {
        if botProfile != nil { return true }
        if case .bool(let flag)? = config?["isBot"] { return flag }
        if case .bool(let flag)? = config?["is_bot"] { return flag }
        return false
    }

    var botDisplayName: String {
        botProfile?.displayName.isEmpty == false ? botProfile!.displayName : name
    }

    var botTagline: String {
        botProfile?.tagline?.isEmpty == false ? botProfile!.tagline! : (description ?? "@\(name)")
    }

    var botAccentColor: String? {
        botProfile?.accentColor?.isEmpty == false ? botProfile!.accentColor : nil
    }

    private func configValue<T: Decodable>(for key: String) -> T? {
        guard let value = config?[key] else { return nil }
        do {
            let data = try JSONEncoder().encode(value)
            return try JSONDecoder().decode(T.self, from: data)
        } catch {
            return nil
        }
    }

    private func configArray<T: Decodable>(for key: String) -> [T]? {
        guard case .array(let items)? = config?[key] else { return nil }
        return items.compactMap { item in
            do {
                let data = try JSONEncoder().encode(item)
                return try JSONDecoder().decode(T.self, from: data)
            } catch {
                return nil
            }
        }
    }

    /// `[String: JSONValue]?` breaks synthesized `Hashable` (Dictionary
    /// never conforms to Hashable in the stdlib, regardless of Key/Value
    /// conformance) — `id` + `updatedAt` is a sufficient, collision-safe
    /// substitute since both change whenever the row actually changes.
    func hash(into hasher: inout Hasher) {
        hasher.combine(id)
        hasher.combine(updatedAt)
    }

    /// Builds the full `config` object for a PUT, with `greeting`/
    /// `suggested_prompts` set/cleared and every other existing key
    /// preserved as-is (the backend replaces `config` wholesale).
    func configReplacing(greeting: String?, suggestedPrompts: [String]) -> [String: JSONValue] {
        var next = config ?? [:]
        if let greeting, !greeting.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            next["greeting"] = .string(greeting)
        } else {
            next.removeValue(forKey: "greeting")
        }
        let prompts = suggestedPrompts.filter { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
        if prompts.isEmpty {
            next.removeValue(forKey: "suggested_prompts")
        } else {
            next["suggested_prompts"] = .array(prompts.map { .string($0) })
        }
        return next
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(String.self, forKey: .id)
        name = try container.decodeIfPresent(String.self, forKey: .name) ?? "Agent"
        description = try container.decodeIfPresent(String.self, forKey: .description)
        type = try container.decodeIfPresent(String.self, forKey: .type) ?? "worker"
        parentAgentId = try container.decodeIfPresent(String.self, forKey: .parentAgentId)
        model = try container.decodeIfPresent(String.self, forKey: .model) ?? ""
        provider = try container.decodeIfPresent(String.self, forKey: .provider) ?? ""
        systemPrompt = try container.decodeIfPresent(String.self, forKey: .systemPrompt)
        status = try container.decodeIfPresent(String.self, forKey: .status) ?? "idle"
        workspaceId = try container.decodeIfPresent(String.self, forKey: .workspaceId)
        avatar = try container.decodeIfPresent(String.self, forKey: .avatar)
        trustTier = try container.decodeIfPresent(String.self, forKey: .trustTier) ?? "standard"
        category = try container.decodeIfPresent(String.self, forKey: .category)
        mode = try container.decodeIfPresent(String.self, forKey: .mode) ?? "primary"
        createdAt = try container.decodeIfPresent(String.self, forKey: .createdAt) ?? ""
        updatedAt = try container.decodeIfPresent(String.self, forKey: .updatedAt) ?? ""
        lastRunAt = try container.decodeIfPresent(String.self, forKey: .lastRunAt)
        // parse_json_column (agent_routes.rs) turns the stored TEXT column
        // into a native JSON value before the response is serialized, so
        // this decodes directly — no string-unwrapping fallback needed.
        config = try container.decodeIfPresent([String: JSONValue].self, forKey: .config)

        harnessConfig = try container.decodeIfPresent(HarnessConfig.self, forKey: .harnessConfig)
        topLevelBotProfile = try container.decodeIfPresent(BotProfile.self, forKey: .topLevelBotProfile)
        topLevelConnectorBindings = try container.decodeIfPresent([ConnectorBinding].self, forKey: .topLevelConnectorBindings)
        topLevelSecretRefs = try container.decodeIfPresent([SecretRef].self, forKey: .topLevelSecretRefs)
        topLevelIdentityChannels = try container.decodeIfPresent(IdentityChannels.self, forKey: .topLevelIdentityChannels)
        topLevelVMOperator = try container.decodeIfPresent(VMOperatorConfig.self, forKey: .topLevelVMOperator)

        if let modes = try? container.decode([String].self, forKey: .enabledModes) {
            enabledModes = modes
        } else if let raw = try? container.decode(String.self, forKey: .enabledModes),
                  let data = raw.data(using: .utf8),
                  let modes = try? JSONDecoder().decode([String].self, from: data) {
            // Tolerates the column arriving as a JSON-encoded string.
            enabledModes = modes
        } else {
            enabledModes = ["chat"]
        }

        if let flag = try? container.decode(Bool.self, forKey: .isPrimary) {
            isPrimary = flag
        } else if let flag = try? container.decode(Int.self, forKey: .isPrimary) {
            isPrimary = flag != 0
        } else {
            isPrimary = false
        }
    }

    /// Whether this agent may be selected on the given surface (BottomDock's
    /// `allowedSurfaces.includes(agentModeSurface)` filter — same helper
    /// AgentSummary carries for the composer pill).
    func allows(surface: AppMode) -> Bool {
        enabledModes.contains(surface.rawValue)
    }

    /// Catalog-style "provider/model" id for send-time `runtimeModelId`
    /// (RuntimeModel.id) — nil unless both columns are set, so an agent
    /// without an explicit model defers to the composer/backend default.
    var runtimeModelId: String? {
        guard !model.isEmpty, !provider.isEmpty else { return nil }
        return "\(provider)/\(model)"
    }

    /// Memberwise init for DEBUG screenshot fixtures and local splices after
    /// a hub edit (the PUT answers only `{"success": true}`).
    init(id: String, name: String, description: String? = nil, type: String = "worker",
         parentAgentId: String? = nil, model: String = "", provider: String = "",
         systemPrompt: String? = nil, status: String = "idle", workspaceId: String? = nil,
         avatar: String? = nil, trustTier: String = "standard", enabledModes: [String] = ["chat"],
         category: String? = nil, mode: String = "primary", isPrimary: Bool = false,
         createdAt: String = "", updatedAt: String = "", lastRunAt: String? = nil,
         config: [String: JSONValue]? = nil,
         harnessConfig: HarnessConfig? = nil,
         botProfile: BotProfile? = nil,
         connectorBindings: [ConnectorBinding]? = nil,
         secretRefs: [SecretRef]? = nil,
         identityChannels: IdentityChannels? = nil,
         vmOperator: VMOperatorConfig? = nil) {
        self.id = id
        self.name = name
        self.description = description
        self.type = type
        self.parentAgentId = parentAgentId
        self.model = model
        self.provider = provider
        self.systemPrompt = systemPrompt
        self.status = status
        self.workspaceId = workspaceId
        self.avatar = avatar
        self.trustTier = trustTier
        self.enabledModes = enabledModes
        self.category = category
        self.mode = mode
        self.isPrimary = isPrimary
        self.createdAt = createdAt
        self.config = config
        self.updatedAt = updatedAt
        self.lastRunAt = lastRunAt
        self.harnessConfig = harnessConfig
        self.topLevelBotProfile = botProfile
        self.topLevelConnectorBindings = connectorBindings
        self.topLevelSecretRefs = secretRefs
        self.topLevelIdentityChannels = identityChannels
        self.topLevelVMOperator = vmOperator
    }

    #if DEBUG
    /// Demo fixture for UI regression screenshots when running with
    /// `-auto-skip-auth -open-bot-home-demo`. Populates enough bot fields
    /// that `BotHomeView` renders every card with sample content.
    static var demoBot: AgentRecord {
        AgentRecord(
            id: "demo-bot-home-001",
            name: "OpenMausBot",
            description: "A sample bot dashboard for offline smoke testing.",
            type: "worker",
            model: "gpt-4o",
            provider: "openai",
            systemPrompt: "You are a helpful assistant.",
            status: "idle",
            avatar: nil,
            trustTier: "standard",
            enabledModes: ["chat"],
            category: "assistant",
            mode: "primary",
            isPrimary: true,
            createdAt: "2026-08-20T00:00:00Z",
            updatedAt: "2026-08-20T00:00:00Z",
            lastRunAt: "2026-08-20T00:00:00Z",
            config: [
                "greeting": .string("Hello! I'm OpenMausBot. Pick a starter prompt or run a task."),
                "suggested_prompts": .array([
                    .string("Summarize my latest run"),
                    .string("Open the desktop preview"),
                    .string("Check webhook status")
                ]),
                "isBot": .bool(true)
            ],
            harnessConfig: HarnessConfig(
                mode: "autonomous",
                byok: [
                    "openai": HarnessBYOKProvider(apiKey: "sk-demo")
                ]
            ),
            botProfile: BotProfile(
                displayName: "OpenMausBot",
                tagline: "Autonomous execution, one tap away.",
                welcomeMessage: "Hello! I'm OpenMausBot. Pick a starter prompt or run a task.",
                starterPrompts: [
                    "Summarize my latest run",
                    "Open the desktop preview",
                    "Check webhook status"
                ],
                accentColor: "#E07A5F"
            ),
            vmOperator: VMOperatorConfig(
                enabled: true,
                provider: "docker",
                image: "allternit/openmaus:latest",
                vncEnabled: true,
                vncUrl: "http://localhost:5900",
                allowedActions: ["start", "stop", "restart"],
                resources: VMResources(cpu: "2", memory: "4GB", disk: "20GB")
            )
        )
    }
    #endif
}

/// Envelope of the registry list (`{ "agents": [...] }`) decoded with full
/// rows for the hub — same payload AgentListResponse decodes as summaries.
struct AgentRecordListResponse: Decodable, Sendable {
    let agents: [AgentRecord]
}

/// Envelope of `GET /api/v1/agents/:id` (`{ "agent": {...} }`,
/// agent_routes.rs:1063).
struct AgentRecordResponse: Decodable, Sendable {
    let agent: AgentRecord
}

// MARK: - Templates (GET /api/v1/agent-templates)

/// One row of the pattern-template catalog (agent_routes.rs:820-873):
/// `{id, name, description?, category, spec, is_builtin, created_at?}`.
/// Only `spec.pattern` is read out of the spec blob — the hub shows the
/// pattern as the row's subtitle tag.
struct AgentTemplate: Decodable, Sendable, Identifiable {
    let id: String
    let name: String
    let description: String?
    let category: String
    let isBuiltin: Bool
    /// `spec.pattern` ("researcher" / "reviewer" / …), nil for custom specs.
    let pattern: String?

    enum CodingKeys: String, CodingKey {
        case id, name, description, category
        case spec
        case isBuiltin = "is_builtin"
    }

    private struct Spec: Decodable {
        let pattern: String?
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(String.self, forKey: .id)
        name = try container.decodeIfPresent(String.self, forKey: .name) ?? "Template"
        description = try container.decodeIfPresent(String.self, forKey: .description)
        category = try container.decodeIfPresent(String.self, forKey: .category) ?? "custom"
        isBuiltin = (try? container.decode(Bool.self, forKey: .isBuiltin)) ?? false
        let spec = try? container.decodeIfPresent(Spec.self, forKey: .spec)
        pattern = spec?.pattern ?? nil
    }
}

/// Envelope of `GET /api/v1/agent-templates` (`{ "templates": [...] }`).
struct AgentTemplateListResponse: Decodable, Sendable {
    let templates: [AgentTemplate]
}

/// Body of `POST /api/v1/agents/from-template` (`InstantiateBody`,
/// agent_routes.rs:813-818). `brain` and `name_override` are left nil — the
/// backend then resolves the user's default model itself.
struct InstantiateTemplateBody: Encodable, Sendable {
    let templateId: String
    let nameOverride: String?

    enum CodingKeys: String, CodingKey {
        case templateId = "template_id"
        case nameOverride = "name_override"
    }
}

/// Response of `POST /api/v1/agents/from-template` (201):
/// `{pattern, orchestrator: {id, name}, subagents: [{id, name}]}`
/// (agent_routes.rs:974-978). The hub lands on the orchestrator's detail.
struct InstantiateTemplateResponse: Decodable, Sendable {
    struct AgentRef: Decodable, Sendable {
        let id: String
        let name: String?
    }

    let pattern: String?
    let orchestrator: AgentRef
    let subagents: [AgentRef]
}

// MARK: - Workspace files (GET/PUT /api/v1/agents/:id/workspace/*)

/// One entry of `GET /agents/:id/workspace/files`
/// (`{files: [{path, size_bytes, modified_at}]}`).
struct WorkspaceFileInfo: Decodable, Sendable, Identifiable, Equatable, Hashable {
    let path: String
    let sizeBytes: Int
    let modifiedAt: String?

    var id: String { path }

    enum CodingKeys: String, CodingKey {
        case path
        case sizeBytes = "size_bytes"
        case modifiedAt = "modified_at"
    }

    /// List-row label ("2.1 KB"), matching the mockup's size column.
    var sizeLabel: String {
        ByteCountFormatter.string(fromByteCount: Int64(sizeBytes), countStyle: .file)
    }
}

struct WorkspaceFileListResponse: Decodable, Sendable {
    let files: [WorkspaceFileInfo]
}

/// Payload of `GET /agents/:id/workspace/file?path=…` (`{path, content}`).
struct WorkspaceFileContent: Decodable, Sendable {
    let path: String
    let content: String
}

/// Body of `PUT /agents/:id/workspace/file` (`{path, content}` — overwrite).
struct WriteWorkspaceFileBody: Encodable, Sendable {
    let path: String
    let content: String
}
