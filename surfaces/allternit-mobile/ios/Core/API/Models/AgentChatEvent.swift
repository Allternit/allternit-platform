import Foundation

// -----------------------------------------------------------------------------
// AgentChatEvent — wire frames of `POST /api/agent-chat` (LIVE path).
//
// The endpoint streams SSE-style `data: {json}\n\n` lines on the POST response
// body. Canonical frames emitted by the Rust bridge
// (cmd/allternit-api/src/v1_routes.rs:247-388):
//
//   {"type":"message_start", messageId, modelId, runtimeModelId}
//   {"type":"content_block_delta", messageId, partId, delta:{type:"text_delta", text}}
//   {"type":"finish", messageId, status:"complete"|"error", metadata:{status, error?}}
//
// Additional frames TOLERATED (mirroring the web parser,
// native-agent-api.ts:661-807, which accepts them for other Gizzi daemon
// modes even though the current bridge never emits them):
//
//   {chunk, chunk_type:"text"|"tool_call"|"tool_result"|"error"|"done", session_id}
//   {"type":"content_block_start", content_block:{type:"tool_use", id, name, input}}
//   {"type":"tool_result"|"tool_error"|"error"|"message_stop", …}
//   {"type":"artifact"|"artifact.created"|"artifact-created", …}
//   data: [DONE]   (handled line-level by AgentChatClient, not decoded here)
//
// Unlike the (quarantined) ReplyEvent decoder, unknown frames map to
// `.ignored` instead of throwing — the web parser skips-and-continues on
// anything it doesn't recognize, and so do we.
// -----------------------------------------------------------------------------

/// A single parsed frame from the agent-chat stream.
enum AgentChatEvent: Decodable, Sendable {
    /// `{"type":"message_start"}` — stream opened; carries model labels.
    case messageStart(MessageStart)

    /// Text delta for the assistant message
    /// (`content_block_delta`/`text_delta`, or a `chunk_type:"text"` chunk).
    case textDelta(TextDelta)

    /// Reasoning ("thinking") delta (`content_block_delta`/`thinking_delta`).
    case thinkingDelta(String)

    /// Tool invocation started (`content_block_start`/`tool_use`, or a
    /// `chunk_type:"tool_call"` chunk with a JSON payload).
    case toolCall(ToolCall)

    /// Tool invocation finished (`{"type":"tool_result"}` or chunk).
    case toolResult(ToolResult)

    /// Tool invocation failed (`{"type":"tool_error"}`).
    case toolError(ToolError)

    /// Artifact produced (`artifact` / `artifact.created` / `artifact-created`),
    /// pre-normalized exactly like the web parser (kind whitelist, title
    /// fallback, image url/data-URL handling).
    case artifact(Artifact)

    /// Terminal frame (`{"type":"finish"}`). `status` is "complete"|"error";
    /// `error` comes from `metadata.error` (v1_routes.rs:267-309).
    case finish(Finish)

    /// Non-terminal error (`{"type":"error"}` or `chunk_type:"error"`).
    /// Note: the web surfaces `chunk_type:"error"` chunks as plain message
    /// text via its blanket `onChunk` call; we map them here instead (see
    /// ChatViewModel for the inline ⚠️ rendering).
    case streamError(String)

    /// Done signal (`{"type":"message_stop"}` or `chunk_type:"done"`).
    case done

    /// The agent asked the person a question (session-worker `question.asked`).
    /// Answered in place from the message's question card.
    case question(AgentQuestionRequest)

    /// A question was answered or dismissed (`question.replied` /
    /// `question.rejected`, payload `{sessionID, requestID}`).
    case questionClosed(requestID: String)

    /// The agent is waiting on a permission (session-worker `permission.asked`).
    /// The tool calls rail draws Deny / Allow on the matching tool row.
    case permissionAsked(AgentPermissionAsk)

    /// Known-but-unrendered or unknown frame — parser tolerance.
    case ignored

    struct MessageStart: Decodable, Sendable {
        let messageId: String?
        let modelId: String?
        let runtimeModelId: String?
    }

    struct TextDelta: Sendable {
        let messageId: String?
        let partId: String?
        let text: String
    }

    /// What the session-worker's tool part says beyond its name: the input
    /// (string-valued fields only — enough to describe the step and join a
    /// subagent call to its task), its title, error and timing.
    struct ToolDetail: Sendable, Equatable {
        var input: [String: String] = [:]
        var title: String? = nil
        var error: String? = nil
        var startedAt: Date? = nil
        var finishedAt: Date? = nil
        /// The tool's result text (the message_agent delivery receipt is read from it).
        var output: String? = nil
        /// send_user_message: the message card (the tool part's `metadata`).
        var userMessage: AgentUserMessage? = nil
        /// automation: the scheduled-task card (the tool part's `metadata`).
        var automation: AgentAutomationResult? = nil
    }

    struct ToolCall: Sendable {
        let toolCallId: String
        let toolName: String
        var detail: ToolDetail? = nil
    }

    struct ToolResult: Sendable {
        let toolCallId: String
        let toolName: String
        var detail: ToolDetail? = nil
        /// run_code: produced files + run summary (top-level `files` / `run`).
        var runCode: RunCodeResult? = nil
    }

    struct ToolError: Sendable {
        let toolCallId: String
        let toolName: String?
        let error: String
        var detail: ToolDetail? = nil
    }

    struct Finish: Decodable, Sendable {
        struct Metadata: Decodable, Sendable {
            /// The runtime's structured error, passed through by the bridge
            /// (v1_routes.rs) as `metadata.errorDetails` — e.g.
            /// `{"name":"ProviderModelNotFoundError","message":"…","data":{…}}`.
            struct ErrorDetails: Decodable, Sendable {
                let name: String?
                let message: String?
            }

            let status: String?
            let error: String?
            let errorDetails: ErrorDetails?
        }

        let messageId: String?
        let status: String?
        let metadata: Metadata?
        /// What the run used (v1_routes.rs `usage_from_message_info`); absent
        /// when the runtime reported nothing.
        let usage: CodeRunUsage?
    }

    /// Normalized artifact payload (mirrors the `ArtifactUIPart` the web
    /// parser builds at native-agent-api.ts:784-802).
    struct Artifact: Sendable {
        let artifactId: String
        let kind: String
        let title: String
        let url: String?
        let content: String?
    }

    /// Frames that end the stream (`finish` / `done`). `[DONE]` is terminal
    /// too but is intercepted line-level by the client before JSON decoding.
    var isTerminal: Bool {
        switch self {
        case .finish, .done:
            return true
        default:
            return false
        }
    }

    // MARK: - Decoding

    private enum CodingKeys: String, CodingKey {
        // Discriminators
        case type
        case chunkType = "chunk_type"
        // BackendChatChunk
        case chunk
        case sessionId = "session_id"
        // message_start / content_block_delta
        case messageId, modelId, runtimeModelId, partId, delta
        // content_block_start
        case contentBlock = "content_block"
        // tool_result / tool_error / error
        case toolCallId, toolName, error
        // finish
        case status, metadata
        // artifact
        case artifactId, id, kind, title, url, content
        // connector_suggest / settings tool results
        case output
    }

    private struct DeltaPayload: Decodable {
        let type: String?
        let text: String?
        let thinking: String?
    }

    private struct ContentBlock: Decodable {
        let type: String?
        let id: String?
        let name: String?
        // `input` skipped (see TS parser).
    }

    /// JSON payload that `chunk_type:"tool_call"|"tool_result"` chunks may
    /// carry as a *stringified* object (web `tryParseChunkPayload`).
    private struct ChunkToolPayload: Decodable {
        let toolCallId: String?
        let toolName: String?
        // `input`/`args`/`result`/`output` skipped (see TS parser).
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)

        // BackendChatChunk format: { chunk, chunk_type, session_id }.
        if let chunkType = try container.decodeIfPresent(String.self, forKey: .chunkType) {
            // `chunk` is typed `string` in TS; tolerate non-string/absent.
            let chunk = try? container.decode(String.self, forKey: .chunk)
            switch chunkType {
            case "text":
                if let chunk {
                    self = .textDelta(TextDelta(messageId: nil, partId: nil, text: chunk))
                } else {
                    self = .ignored
                }
            case "tool_call", "tool_result":
                self = Self.decodeChunkTool(chunkType: chunkType, chunk: chunk)
            case "error":
                self = .streamError(chunk ?? "Stream error")
            case "done":
                self = .done
            default:
                self = .ignored
            }
            return
        }

        guard let type = try container.decodeIfPresent(String.self, forKey: .type) else {
            self = .ignored
            return
        }

        switch type {
        case "message_start":
            self = .messageStart(MessageStart(
                messageId: try container.decodeIfPresent(String.self, forKey: .messageId),
                modelId: try container.decodeIfPresent(String.self, forKey: .modelId),
                runtimeModelId: try container.decodeIfPresent(String.self, forKey: .runtimeModelId)
            ))

        case "content_block_delta":
            // Web: thinking_delta w/ thinking → thinking chunk; else text → text chunk.
            let delta = try container.decodeIfPresent(DeltaPayload.self, forKey: .delta)
            if delta?.type == "thinking_delta", let thinking = delta?.thinking {
                self = .thinkingDelta(thinking)
            } else if let text = delta?.text {
                self = .textDelta(TextDelta(
                    messageId: try container.decodeIfPresent(String.self, forKey: .messageId),
                    partId: try container.decodeIfPresent(String.self, forKey: .partId),
                    text: text
                ))
            } else {
                self = .ignored
            }

        case "content_block_start":
            let block = try container.decodeIfPresent(ContentBlock.self, forKey: .contentBlock)
            if block?.type == "tool_use", let id = block?.id {
                self = .toolCall(ToolCall(toolCallId: id, toolName: block?.name ?? "Tool"))
            } else {
                self = .ignored
            }

        case "tool_result":
            let toolName = try container.decodeIfPresent(String.self, forKey: .toolName) ?? "Tool"
            // send_user_message: the card, when the frame carries the part's `metadata`.
            let userMessage = toolName == AgentUserMessage.toolName
                ? ((try? container.decodeIfPresent(AgentUserMessage.self, forKey: .metadata)) ?? nil)
                : nil
            // automation: the scheduled-task card, from the same `metadata`.
            let automation = toolName == AgentAutomationResult.toolName
                ? ((try? container.decodeIfPresent(AgentAutomationResult.self, forKey: .metadata)) ?? nil)
                : nil
            // connector_suggest / settings: the card data (the part's `metadata`,
            // v1_routes.rs METADATA_CARD_TOOLS) rides in `output` as JSON, the
            // field the record keeps; the cards parse it (AgentToolCards).
            let cardOutput = AgentToolCards.isCardTool(toolName) ? Self.cardOutput(container) : nil
            self = .toolResult(ToolResult(
                toolCallId: try container.decodeIfPresent(String.self, forKey: .toolCallId) ?? "",
                toolName: toolName,
                detail: (userMessage != nil || automation != nil || cardOutput != nil)
                    ? ToolDetail(output: cardOutput, userMessage: userMessage, automation: automation)
                    : nil,
                runCode: (try? RunCodeKeys(from: decoder))?.result
            ))

        case "tool_error":
            self = .toolError(ToolError(
                toolCallId: try container.decodeIfPresent(String.self, forKey: .toolCallId) ?? "",
                toolName: try container.decodeIfPresent(String.self, forKey: .toolName),
                error: try container.decodeIfPresent(String.self, forKey: .error) ?? "Tool execution failed"
            ))

        case "finish":
            self = .finish(try Finish(from: decoder))

        case "message_stop":
            self = .done

        case "error":
            self = .streamError(
                try container.decodeIfPresent(String.self, forKey: .error) ?? "Stream error"
            )

        case "artifact", "artifact.created", "artifact-created":
            if let artifact = Self.decodeArtifact(container) {
                self = .artifact(artifact)
            } else {
                self = .ignored
            }

        case "question.asked":
            struct Envelope: Decodable { let properties: AgentQuestionRequest? }
            if let req = (try? Envelope(from: decoder).properties) ?? (try? AgentQuestionRequest(from: decoder)) {
                self = .question(req)
            } else {
                self = .ignored
            }

        case "question.replied", "question.rejected":
            struct Ids: Decodable { let requestID: String? }
            struct Envelope: Decodable { let properties: Ids? }
            if let id = ((try? Envelope(from: decoder).properties?.requestID) ?? nil)
                ?? (try? Ids(from: decoder).requestID) ?? nil {
                self = .questionClosed(requestID: id)
            } else {
                self = .ignored
            }

        default:
            // Unknown frame — skip-and-continue (web parser tolerance).
            self = .ignored
        }
    }

    // MARK: - Card tool payloads (connector_suggest, settings)

    /// The frame's `metadata` as a JSON string, else its `output` text.
    private static func cardOutput(_ container: KeyedDecodingContainer<CodingKeys>) -> String? {
        if let meta = (try? container.decodeIfPresent(AnyCodable.self, forKey: .metadata)) ?? nil,
           meta.value is [String: Any],
           JSONSerialization.isValidJSONObject(meta.value),
           let data = try? JSONSerialization.data(withJSONObject: meta.value),
           let text = String(data: data, encoding: .utf8) {
            return text
        }
        return (try? container.decodeIfPresent(String.self, forKey: .output)) ?? nil
    }

    // MARK: - Chunk tool payloads (web `tryParseChunkPayload`)

    /// Tool chunks carry their payload as a stringified JSON object; a
    /// plain-string payload yields NO tool callback in the web parser, so it
    /// decodes to `.ignored` here as well.
    private static func decodeChunkTool(chunkType: String, chunk: String?) -> AgentChatEvent {
        guard let chunk,
              let data = chunk.data(using: .utf8),
              let payload = try? JSONDecoder().decode(ChunkToolPayload.self, from: data) else {
            return .ignored
        }
        // Web fallback id: `${toolName ?? "tool"}-${Date.now()}`.
        let fallbackId = "\(payload.toolName ?? "tool")-\(Int(Date().timeIntervalSince1970 * 1000))"
        if chunkType == "tool_call" {
            return .toolCall(ToolCall(
                toolCallId: payload.toolCallId ?? fallbackId,
                toolName: payload.toolName ?? "Tool"
            ))
        }
        return .toolResult(ToolResult(
            toolCallId: payload.toolCallId ?? fallbackId,
            toolName: payload.toolName ?? "Tool"
        ))
    }

    // MARK: - Artifact normalization (web parser, native-agent-api.ts:784-802)

    private static func decodeArtifact(
        _ container: KeyedDecodingContainer<CodingKeys>
    ) -> Artifact? {
        // `try?` on `decodeIfPresent` yields a double-optional; flatten with `?? nil`.
        guard let kind = (try? container.decodeIfPresent(String.self, forKey: .kind)) ?? nil,
              supportedArtifactKinds.contains(kind) else {
            return nil
        }
        let content = (try? container.decodeIfPresent(String.self, forKey: .content)) ?? nil
        let hasDataURL = content?.hasPrefix("data:") == true
        let rawURL = (try? container.decodeIfPresent(String.self, forKey: .url)) ?? nil

        let rawTitle = (try? container.decodeIfPresent(String.self, forKey: .title)) ?? nil
        guard let title = rawTitle?.trimmingCharacters(in: .whitespacesAndNewlines),
              !title.isEmpty else {
            return nil
        }

        let decodedArtifactId = (try? container.decodeIfPresent(String.self, forKey: .artifactId)) ?? nil
        let decodedId = (try? container.decodeIfPresent(String.self, forKey: .id)) ?? nil
        guard let artifactId = decodedArtifactId ?? decodedId, !artifactId.isEmpty else {
            return nil
        }

        let url: String?
        let finalContent: String?
        if kind == "image" {
            url = rawURL ?? (hasDataURL ? content : nil)
            finalContent = hasDataURL ? content : nil
        } else {
            url = rawURL
            finalContent = content
        }

        return Artifact(
            artifactId: artifactId,
            kind: kind,
            title: title,
            url: url,
            content: finalContent
        )
    }

    private static let supportedArtifactKinds: Set<String> = [
        "image", "svg", "mermaid", "jsx", "html", "document",
        "slides", "sheet", "audio", "video", "podcast"
    ]
}

// MARK: - connector_suggest and settings cards (Primitives Wave D, D2)

/// Reads the card data of the `connector_suggest` and `settings` tools from
/// a tool call's `output`. That text is the tool part's `metadata` JSON on a
/// live agent-chat frame (`cardOutput` above), or the tool's own result JSON
/// on the session-worker stream and a reloaded chat (gizzi
/// runtime/tools/builtins/connector.ts and settings.ts). Both shapes parse.
enum AgentToolCards {
    static func isCardTool(_ name: String) -> Bool {
        let n = ToolDescriber.normalized(name)
        return n == "connectorsuggest" || n == "settings"
    }

    static func json(_ output: String?) -> [String: Any]? {
        guard let output, let data = output.data(using: .utf8) else { return nil }
        return (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
    }

    static func string(_ value: Any?) -> String? {
        guard let s = value as? String else { return nil }
        let t = s.trimmingCharacters(in: .whitespacesAndNewlines)
        return t.isEmpty ? nil : t
    }
}

/// What `connector_suggest` offered (gizzi primitives/connectors.ts
/// `SuggestCard`, carried as metadata `card`: `{type: "connector_suggest",
/// connectors: [{id, name, group}], reason}`; `state` is connected |
/// available | unavailable when present). No link, code or token, ever: the
/// user's tap connects through the app's own Connect flow.
struct AgentConnectorSuggestion: Equatable, Sendable {
    struct Connector: Equatable, Sendable, Identifiable {
        let id: String
        let name: String
        let group: String?
        /// "connected", "available" or "unavailable"; nil when not sent.
        let state: String?
    }

    let connectors: [Connector]
    let reason: String?

    static func isTool(_ name: String) -> Bool { ToolDescriber.normalized(name) == "connectorsuggest" }

    /// `output` is the card metadata (`{shown, reason, card}`) or the tool's
    /// result (`{shown, dropped?}`); `input` is the call's string input
    /// (`reason`), `title` the call title ("Suggested Gmail, Slack").
    static func parse(toolName: String, output: String?, input: [String: String] = [:], title: String? = nil) -> AgentConnectorSuggestion? {
        guard isTool(toolName), let json = AgentToolCards.json(output) else { return nil }
        let card = (json["card"] as? [String: Any]) ?? json
        var connectors: [Connector] = []
        if let list = card["connectors"] as? [[String: Any]] {
            connectors = list.compactMap { item in
                guard let id = AgentToolCards.string(item["id"]) else { return nil }
                return Connector(
                    id: id,
                    name: AgentToolCards.string(item["name"]) ?? ConnectionsCatalog.entry(id)?.name ?? id,
                    group: AgentToolCards.string(item["group"]),
                    state: AgentToolCards.string(item["state"])
                )
            }
        }
        if connectors.isEmpty {
            // The tool's own result: ids only. Names come from the call title,
            // in the same order, else the app's catalog, else the id.
            let ids = ((json["ids"] as? [String]) ?? (json["shown"] as? [String]) ?? []).filter { !$0.isEmpty }
            var names: [String] = []
            if let title, title.hasPrefix("Suggested ") {
                names = title.dropFirst("Suggested ".count).components(separatedBy: ", ")
            }
            connectors = ids.enumerated().map { i, id in
                let name = names.count == ids.count ? names[i] : (ConnectionsCatalog.entry(id)?.name ?? id)
                return Connector(id: id, name: name, group: nil, state: nil)
            }
        }
        guard !connectors.isEmpty else { return nil }
        let reason = AgentToolCards.string(card["reason"]) ?? AgentToolCards.string(json["reason"]) ?? AgentToolCards.string(input["reason"])
        return AgentConnectorSuggestion(connectors: connectors, reason: reason)
    }
}

/// A setting's value as the `settings` tool reports it.
enum AgentSettingValue: Equatable, Sendable {
    case bool(Bool)
    case string(String)
    case number(Double)
    case list([String])

    init?(json: Any?) {
        switch json {
        case let n as NSNumber where CFGetTypeID(n) == CFBooleanGetTypeID(): self = .bool(n.boolValue)
        case let b as Bool: self = .bool(b)
        case let n as NSNumber: self = .number(n.doubleValue)
        case let s as String: self = .string(s)
        case let a as [Any]: self = .list(a.compactMap { $0 as? String })
        default: return nil
        }
    }

    /// What the card says: "On", "Off", "Dark", "a, b".
    var display: String {
        switch self {
        case .bool(let b): return b ? "On" : "Off"
        case .string(let s):
            guard let first = s.first else { return "Empty" }
            return first.uppercased() + s.dropFirst().replacingOccurrences(of: "_", with: " ")
        case .number(let d): return d.rounded() == d ? String(Int(d)) : String(d)
        case .list(let l): return l.isEmpty ? "None" : l.joined(separator: ", ")
        }
    }

    var boolValue: Bool? {
        switch self {
        case .bool(let b): return b
        case .string(let s):
            switch s.lowercased() {
            case "true", "on": return true
            case "false", "off": return false
            default: return nil
            }
        default: return nil
        }
    }

    var stringValue: String? {
        if case .string(let s) = self { return s }
        return nil
    }
}

/// What a `settings` write did, one row per change (gizzi
/// primitives/settings.ts `ChangeResult`; allternit-api
/// `/bot-routing/settings/write`): `{key, outcome|status, value?, previous?,
/// reason?, settings_path|link?, label?, scope?}`. Keys and labels:
/// contracts/settings/definitions.json.
struct AgentSettingsResult: Equatable, Sendable {
    enum Status: String, Sendable {
        case applied
        case applyOnDevice = "apply_on_device"
        case needsConfirmation = "needs_confirmation"
        case locked
        case invalid
    }

    struct Change: Equatable, Sendable, Identifiable {
        let key: String
        let label: String
        let value: AgentSettingValue?
        let previous: AgentSettingValue?
        let status: Status
        /// The Settings screen (`settings/<section>`) or a link.
        let link: String?
        let reason: String?
        var id: String { key }
    }

    let changes: [Change]

    static func isTool(_ name: String) -> Bool { ToolDescriber.normalized(name) == "settings" }

    static func parse(toolName: String, output: String?) -> AgentSettingsResult? {
        guard isTool(toolName), let json = AgentToolCards.json(output),
              let list = json["changes"] as? [[String: Any]] else { return nil }
        let changes: [Change] = list.compactMap { item in
            guard let key = AgentToolCards.string(item["key"]),
                  let raw = AgentToolCards.string(item["status"]) ?? AgentToolCards.string(item["outcome"]),
                  let status = Status(rawValue: raw) else { return nil }
            return Change(
                key: key,
                label: AgentToolCards.string(item["label"]) ?? labels[key] ?? key,
                value: AgentSettingValue(json: item["value"]),
                previous: AgentSettingValue(json: item["previous"]),
                status: status,
                link: AgentToolCards.string(item["link"]) ?? AgentToolCards.string(item["settings_path"]),
                reason: AgentToolCards.string(item["reason"])
            )
        }
        return changes.isEmpty ? nil : AgentSettingsResult(changes: changes)
    }

    /// Labels from contracts/settings/definitions.json, for results that
    /// carry only the key.
    static let labels: [String: String] = [
        "appearance.theme": "Theme",
        "appearance.chat_font": "Chat font",
        "appearance.reduce_motion": "Reduce motion",
        "notifications.response": "Notify when a reply is ready",
        "notifications.approvals": "Notify when an approval is needed",
        "notifications.scheduled": "Notify about scheduled tasks",
        "notifications.handoffs": "Notify about handoffs",
        "notifications.sound": "Notification sound",
        "chat.answer_style": "Answer style",
        "browser.preferred": "Browser for agents",
        "browser.open_links_in_app": "Open links in the app",
        "gizzi.tui_theme": "Terminal theme",
        "gizzi.model": "Model",
        "gizzi.verbose": "Verbose output",
        "instructions.global": "Global instructions",
        "instructions.custom": "Custom instructions",
        "memory.capture.channels": "Remember from channels",
        "memory.capture.gizzi": "Remember from Gizzi Code",
        "memory.capture.agent_folders": "Remember from agent folders",
        "memory.capture.vendors": "Remember from assistant logins",
        "memory.dreaming": "Memory dreaming",
        "security.require_trusted_devices": "Require trusted devices",
        "files.trusted_folders": "Trusted folders",
        "browser.allowed_sites": "Allowed sites",
        "gizzi.permission_mode": "Permission mode",
        "account.plan": "Plan",
        "account.billing": "Billing",
        "account.email": "Email",
        "account.password": "Password",
        "account.keys": "API keys",
    ]
}
