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
//   {"type":"tool_result", toolCallId, toolName, result, metadata?}  — metadata
//     only for card tools (send_user_message, lookup: `{kind, card?, credit?}`)
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

    /// `{"type":"session_status","status":{"type":"busy","phase","note"}}`:
    /// the run's one-line status. `busy == false` (idle) clears it.
    case sessionStatus(SessionStatus)

    /// Non-terminal error (`{"type":"error"}` or `chunk_type:"error"`).
    /// Note: the web surfaces `chunk_type:"error"` chunks as plain message
    /// text via its blanket `onChunk` call; we map them here instead (see
    /// ChatViewModel for the inline ⚠️ rendering).
    case streamError(String)

    /// An `{"type":"error"}` frame that says why: `{error, code?, reset_at?}`
    /// (platform #2807). The chat draws one typed card (usage limit with its
    /// reset time, signed out, network) instead of the bare sentence.
    case streamErrorFrame(StreamErrorFrame)

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

    /// A long-running MCP task changed (`mcp.task`), a server asked a question
    /// (`mcp.elicitation`) or it was answered (`mcp.elicitation.resolved`).
    /// Drawn by Features/McpTasks, behind the `mcp.tasks` flag.
    case mcpTask(sessionID: String?, part: McpTaskPart)
    case mcpElicitation(sessionID: String?, part: McpElicitationPart)
    case mcpElicitationResolved(requestID: String, action: String?)

    /// `{"type":"control",language?,speed?,reply_mode?,end_call?}`: a voice_control
    /// tool part asked the voice session to change.
    case control(VoiceControl)

    /// Known-but-unrendered or unknown frame — parser tolerance.
    case ignored

    struct VoiceControl: Sendable, Equatable {
        var language: String? = nil
        var speed: Double? = nil
        var replyMode: String? = nil
        var endCall: Bool = false
    }

    struct MessageStart: Decodable, Sendable {
        let messageId: String?
        let modelId: String?
        let runtimeModelId: String?
        /// `true` on the first frame of a Settings style preview turn: the runtime
        /// honoured `stylePreview` (nothing saved, no chat listed). Absent on a normal turn.
        var stylePreview: Bool = false
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
        /// lookup: the answer card from the tool part's `metadata.card`.
        var lookupCard: AnswerCardEnvelope? = nil
        /// worktree_enter/exit, second_opinion, pr_comments, status_update,
        /// feedback_note, user_location, voice_control, end_conversation:
        /// the tool part's `metadata` (gizzi runtime/primitives/gap-types.ts).
        var gap: GapToolMetadata? = nil
        /// The tool part's `metadata` object as raw JSON text (connector_suggest
        /// and settings cards ride there; v1_routes.rs METADATA_CARD_TOOLS).
        var metadata: String? = nil
    }

    struct ToolCall: Sendable {
        let toolCallId: String
        let toolName: String
        var detail: ToolDetail? = nil
    }

    struct SessionStatus: Sendable, Equatable {
        var busy: Bool
        var phase: String? = nil
        var note: String? = nil
        /// "phase - note", or nil when neither is set.
        var line: String? {
            let text = [phase, note].compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: " \u{2014} ")
            return text.isEmpty ? nil : text
        }
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
            /// Memories the reply used or changed. The v1 stream does not send
            /// this today (history does); decoded so a frame that carries it
            /// shows the chip live. A bad entry is dropped, never the frame.
            let memoryRefs: [MemoryRef]?

            private enum CodingKeys: String, CodingKey {
                case status, error, errorDetails, memoryRefs
            }

            init(from decoder: Decoder) throws {
                let c = try decoder.container(keyedBy: CodingKeys.self)
                status = try? c.decodeIfPresent(String.self, forKey: .status)
                error = try? c.decodeIfPresent(String.self, forKey: .error)
                errorDetails = try? c.decodeIfPresent(ErrorDetails.self, forKey: .errorDetails)
                memoryRefs = (try? c.decodeIfPresent(LossyArray<MemoryRef>.self, forKey: .memoryRefs))??.elements
            }
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
        /// `tool-call://<callID>` when this picture belongs to a media_generate call's card.
        var sourceUri: String? = nil
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

    /// `reset_at` as epoch seconds or milliseconds, or as a string of either or an ISO date.
    private static func resetDate(_ container: KeyedDecodingContainer<CodingKeys>) -> Date? {
        if let n = (try? container.decodeIfPresent(Double.self, forKey: .resetAt)) ?? nil {
            return ChatErrorInfo.resetDate(epoch: n)
        }
        guard let text = (try? container.decodeIfPresent(String.self, forKey: .resetAt)) ?? nil else { return nil }
        if let n = Double(text) { return ChatErrorInfo.resetDate(epoch: n) }
        return ISO8601DateFormatter().date(from: text)
    }

    private enum CodingKeys: String, CodingKey {
        // Discriminators
        case type
        case chunkType = "chunk_type"
        // BackendChatChunk
        case chunk
        case sessionId = "session_id"
        // message_start / content_block_delta
        case messageId, modelId, runtimeModelId, partId, delta, stylePreview
        // content_block_start
        case contentBlock = "content_block"
        // tool_result / tool_error / error
        case toolCallId, toolName, error
        // error: the machine code and the usage-limit reset time
        case errorCode = "code"
        case resetAt = "reset_at"
        // finish
        case status, metadata
        // artifact
        case artifactId, id, kind, title, url, content, sourceUri
        // tool_result: the tool's output text (v1_routes.rs `result`)
        case result
        // connector_suggest / settings tool results
        case output
    }

    /// Tools whose live card reads the result text (history reads it from
    /// the part): browser_tasks → task cards, capture_screen_context → the
    /// "Looked at your screen" chip.
    static let resultCardTools: Set<String> = ["browser_tasks", "capture_screen_context"]

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
                runtimeModelId: try container.decodeIfPresent(String.self, forKey: .runtimeModelId),
                stylePreview: (try? container.decodeIfPresent(Bool.self, forKey: .stylePreview)) ?? false
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
            // lookup: the fact card (`metadata.card`), drawn under the reply.
            let lookupCard = toolName == LookupToolMetadata.toolName
                ? ((try? container.decodeIfPresent(LookupToolMetadata.self, forKey: .metadata)) ?? nil)?.card
                : nil
            // Gap tools: the `*Metadata` object (all fields optional).
            let gap = GapToolMetadata.isGapTool(toolName)
                ? ((try? container.decodeIfPresent(GapToolMetadata.self, forKey: .metadata)) ?? nil)
                : nil
            // `metadata.label`: the model's own short title for this call.
            struct LabelMeta: Decodable { let label: String? }
            let label = ((try? container.decodeIfPresent(LabelMeta.self, forKey: .metadata)) ?? nil)?.label?
                .trimmingCharacters(in: .whitespacesAndNewlines)
            var detail: ToolDetail? = nil
            if userMessage != nil || automation != nil || lookupCard != nil || gap != nil {
                detail = ToolDetail(userMessage: userMessage, automation: automation, lookupCard: lookupCard, gap: gap)
            }
            if let label, !label.isEmpty {
                detail = detail ?? ToolDetail()
                detail?.title = label
            }
            if Self.resultCardTools.contains(toolName),
               let result = (try? container.decodeIfPresent(String.self, forKey: .result)) ?? nil {
                detail = detail ?? ToolDetail()
                detail?.output = result
            }
            // The part's `metadata` as raw JSON (connector_suggest and settings
            // cards read it; v1_routes.rs METADATA_CARD_TOOLS). The card tools
            // also keep their result text as a fallback.
            let metadata = AgentToolCards.jsonText((try? container.decodeIfPresent(AnyCodable.self, forKey: .metadata)) ?? nil)
            let cardOutput = AgentToolCards.isCardTool(toolName)
                ? ((try? container.decodeIfPresent(String.self, forKey: .output)) ?? nil)
                : nil
            if cardOutput != nil || metadata != nil {
                detail = detail ?? ToolDetail()
                if detail?.output == nil { detail?.output = cardOutput }
                detail?.metadata = metadata
            }
            self = .toolResult(ToolResult(
                toolCallId: try container.decodeIfPresent(String.self, forKey: .toolCallId) ?? "",
                toolName: toolName,
                detail: detail,
                runCode: (try? RunCodeKeys(from: decoder))?.result
            ))

        case "tool_error":
            self = .toolError(ToolError(
                toolCallId: try container.decodeIfPresent(String.self, forKey: .toolCallId) ?? "",
                toolName: try container.decodeIfPresent(String.self, forKey: .toolName),
                error: try container.decodeIfPresent(String.self, forKey: .error) ?? "Tool execution failed"
            ))

        case "session_status":
            struct StatusBody: Decodable { let type: String?; let phase: String?; let note: String? }
            let body = (try? container.decodeIfPresent(StatusBody.self, forKey: .status)) ?? nil
            self = .sessionStatus(SessionStatus(
                busy: body?.type == "busy",
                phase: body?.phase,
                note: body?.note
            ))

        case "control":
            struct ControlBody: Decodable {
                let language: String?; let speed: Double?
                let replyMode: String?; let endCall: Bool?
                enum CodingKeys: String, CodingKey { case language, speed, replyMode = "reply_mode", endCall = "end_call" }
            }
            let body = try ControlBody(from: decoder)
            self = .control(VoiceControl(language: body.language, speed: body.speed,
                                         replyMode: body.replyMode, endCall: body.endCall ?? false))

        case "finish":
            self = .finish(try Finish(from: decoder))

        case "message_stop":
            self = .done

        case "error":
            let message = (try? container.decodeIfPresent(String.self, forKey: .error)) ?? nil ?? "Stream error"
            let code = ((try? container.decodeIfPresent(String.self, forKey: .errorCode)) ?? nil)?
                .trimmingCharacters(in: .whitespacesAndNewlines)
            let reset = Self.resetDate(container)
            if (code?.isEmpty == false) || reset != nil {
                self = .streamErrorFrame(StreamErrorFrame(message: message, code: code?.isEmpty == false ? code : nil, resetAt: reset))
            } else {
                self = .streamError(message)
            }

        case "tool_progress":
            // A running tool's live report: `{toolCallId, toolName, metadata}`.
            // media_generate carries `progress` + `progress_detail` there; the
            // call stays running and keeps the newest metadata.
            let metadata = AgentToolCards.jsonText((try? container.decodeIfPresent(AnyCodable.self, forKey: .metadata)) ?? nil)
            if let id = try container.decodeIfPresent(String.self, forKey: .toolCallId), metadata != nil {
                var detail = ToolDetail()
                detail.metadata = metadata
                self = .toolCall(ToolCall(
                    toolCallId: id,
                    toolName: try container.decodeIfPresent(String.self, forKey: .toolName) ?? "media_generate",
                    detail: detail
                ))
            } else {
                self = .ignored
            }

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

        case "mcp.task":
            if let env = try? McpBusEnvelope<McpTaskPart>(from: decoder), let part = env.part {
                self = .mcpTask(sessionID: env.sessionID, part: part)
            } else {
                self = .ignored
            }

        case "mcp.elicitation":
            if let env = try? McpBusEnvelope<McpElicitationPart>(from: decoder), let part = env.part {
                self = .mcpElicitation(sessionID: env.sessionID, part: part)
            } else {
                self = .ignored
            }

        case "mcp.elicitation.resolved":
            if let done = try? McpElicitationResolved(from: decoder), let id = done.requestID {
                self = .mcpElicitationResolved(requestID: id, action: done.action)
            } else {
                self = .ignored
            }

        default:
            // Unknown frame — skip-and-continue (web parser tolerance).
            self = .ignored
        }
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
            content: finalContent,
            sourceUri: (try? container.decodeIfPresent(String.self, forKey: .sourceUri)) ?? nil
        )
    }

    private static let supportedArtifactKinds: Set<String> = [
        "image", "svg", "mermaid", "jsx", "html", "document",
        "slides", "sheet", "audio", "video", "podcast"
    ]
}

// MARK: - connector_suggest and settings cards (Primitives Wave D, D2)

/// Reads the card data of the `connector_suggest` and `settings` tools. The
/// card rides in the tool part's `metadata` (ToolCallRecord.metadata: live
/// agent-chat frames, session-worker parts and reloaded chats); the tool's
/// own result text (`output`, gizzi runtime/tools/builtins/connector.ts and
/// settings.ts) is the fallback for runtimes that don't send metadata.
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

    /// A decoded `metadata` object as raw JSON text; nil when it isn't an object.
    static func jsonText(_ value: AnyCodable?) -> String? {
        guard let object = value?.value as? [String: Any] else { return nil }
        let clean = plain(object)
        guard JSONSerialization.isValidJSONObject(clean),
              let data = try? JSONSerialization.data(withJSONObject: clean) else { return nil }
        return String(data: data, encoding: .utf8)
    }

    /// AnyCodable keeps JSON null as a sentinel; JSONSerialization needs NSNull.
    private static func plain(_ value: Any) -> Any {
        switch value {
        case let d as [String: Any]: return d.mapValues(plain)
        case let a as [Any]: return a.map(plain)
        case is String, is Bool, is Int, is Double, is NSNumber: return value
        default: return NSNull()
        }
    }

    /// The card's JSON: the metadata when it has the card, else the result text.
    static func source(metadata: String?, output: String?, has: ([String: Any]) -> Bool) -> [String: Any]? {
        if let meta = json(metadata), has(meta) { return meta }
        return json(output)
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

    /// `metadata` is the part's metadata (`{shown, reason, card}`); `output`
    /// the tool's result text (`{shown, dropped?}`), the fallback; `input` is
    /// the call's string input (`reason`), `title` the call title
    /// ("Suggested Gmail, Slack").
    static func parse(
        toolName: String, metadata: String? = nil, output: String?, input: [String: String] = [:], title: String? = nil
    ) -> AgentConnectorSuggestion? {
        guard isTool(toolName),
              let json = AgentToolCards.source(metadata: metadata, output: output, has: { $0["card"] is [String: Any] })
        else { return nil }
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
/// `/bot-routing/settings/write`): `{key, outcome, label?, scope?, value?,
/// previous?, reason?, code?, settings_path?}`. Scopes: account (the server
/// applies it), device (the app that sent the turn), runtime (gizzi.* keys,
/// Gizzi Code applies them). Keys and labels: contracts/settings/definitions.json.
struct AgentSettingsResult: Equatable, Sendable {
    enum Status: String, Sendable {
        case applied
        case applyOnDevice = "apply_on_device"
        case needsConfirmation = "needs_confirmation"
        case locked
        case invalid
    }

    enum Scope: String, Sendable {
        case account
        case device
        case runtime
    }

    struct Change: Equatable, Sendable, Identifiable {
        let key: String
        let label: String
        /// Nil when the result didn't say; runtime keys (gizzi.*) are Gizzi Code's.
        var scope: Scope? = nil
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

    /// `metadata` is the part's metadata (`{action, toolCallId, changes}`);
    /// `output` the tool's result text (`{action, changes}`), the fallback.
    static func parse(toolName: String, metadata: String? = nil, output: String?) -> AgentSettingsResult? {
        guard isTool(toolName),
              let json = AgentToolCards.source(metadata: metadata, output: output, has: { $0["changes"] is [[String: Any]] }),
              json["action"] as? String != "read",
              let list = json["changes"] as? [[String: Any]] else { return nil }
        let changes: [Change] = list.compactMap { item in
            guard let key = AgentToolCards.string(item["key"]),
                  let raw = AgentToolCards.string(item["outcome"]),
                  let status = Status(rawValue: raw) else { return nil }
            return Change(
                key: key,
                label: AgentToolCards.string(item["label"]) ?? labels[key] ?? key,
                scope: AgentToolCards.string(item["scope"]).flatMap(Scope.init(rawValue:))
                    ?? (key.hasPrefix("gizzi.") ? .runtime : nil),
                value: AgentSettingValue(json: item["value"]),
                previous: AgentSettingValue(json: item["previous"]),
                status: status,
                link: AgentToolCards.string(item["settings_path"]),
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

// MARK: - Gap tools (worktree, second opinion, PR comments, status, feedback, location, voice, end chat)

/// The result `metadata` of the nine prompt-gap tools, one union of the
/// `*Metadata` schemas in gizzi runtime/primitives/gap-types.ts. Every field
/// is optional, so a runtime that sends fewer fields still decodes. G4-G7
/// fill in the cards that read it.
struct PrCommentItem: Decodable, Equatable, Sendable {
    var id: String? = nil
    var author: String? = nil
    var body: String? = nil
    var path: String? = nil
    var line: Int? = nil
}

struct GapToolMetadata: Decodable, Equatable, Sendable {
    static let worktreeEnter = "worktree_enter"
    static let worktreeExit = "worktree_exit"
    static let secondOpinion = "second_opinion"
    static let prComments = "pr_comments"
    static let statusUpdate = "status_update"
    static let feedbackNote = "feedback_note"
    static let userLocation = "user_location"
    static let voiceControl = "voice_control"
    static let endConversation = "end_conversation"

    static let toolNames: Set<String> = [
        worktreeEnter, worktreeExit, secondOpinion, prComments, statusUpdate,
        feedbackNote, userLocation, voiceControl, endConversation,
    ]

    static func isGapTool(_ name: String) -> Bool { toolNames.contains(name) }

    /// Reads the raw metadata JSON text (the record keeps it for every tool).
    static func decode(toolName: String, json: String?) -> GapToolMetadata? {
        guard isGapTool(toolName), let data = json?.data(using: .utf8) else { return nil }
        return try? JSONDecoder().decode(GapToolMetadata.self, from: data)
    }

    var ok: Bool? = nil
    /// worktree_enter / worktree_exit / pr_comments / voice_control.
    var action: String? = nil
    var path: String? = nil
    var branch: String? = nil
    var base: String? = nil
    var createdBy: String? = nil
    /// worktree_exit: "kept", "removed" or "refused".
    var result: String? = nil
    /// worktree_exit / end_conversation: why.
    var reason: String? = nil
    /// second_opinion ("plan" | "diff") and feedback_note ("bug" | ...).
    var kind: String? = nil
    /// second_opinion: "agree", "concerns" or "disagree".
    var verdict: String? = nil
    var reasons: [String]? = nil
    var tier: String? = nil
    /// pr_comments.
    var pr: Int? = nil
    var url: String? = nil
    var commentCount: Int? = nil
    var comments: [PrCommentItem]? = nil
    var replyId: String? = nil
    /// status_update.
    var phase: String? = nil
    var note: String? = nil
    var at: Double? = nil
    /// feedback_note.
    var noteId: String? = nil
    /// feedback_note: the one-line summary and when it was saved (ms).
    var summary: String? = nil
    var createdAt: Double? = nil
    /// user_location.
    var precision: String? = nil
    var status: String? = nil
    var city: String? = nil
    var region: String? = nil
    var country: String? = nil
    var timeZone: String? = nil
    var latitude: Double? = nil
    var longitude: Double? = nil
    var accuracy: Double? = nil
    /// voice_control.
    var language: String? = nil
    var speed: Double? = nil
    var replyMode: String? = nil
    /// end_conversation.
    var ended: Bool? = nil

    private enum CodingKeys: String, CodingKey {
        case ok, action, path, branch, base, result, reason, kind, verdict, reasons, tier
        case pr, url, comments, phase, note, at, precision, status, city, region, country
        case latitude, longitude, accuracy, language, speed, ended, summary
        case createdAt = "created_at"
        case createdBy = "created_by"
        case replyId = "reply_id"
        case noteId = "note_id"
        case timeZone = "time_zone"
        case replyMode = "reply_mode"
    }

    private struct Ignored: Decodable {}

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        func opt<T: Decodable>(_ type: T.Type, _ key: CodingKeys) -> T? {
            (try? c.decodeIfPresent(type, forKey: key)) ?? nil
        }
        ok = opt(Bool.self, .ok)
        action = opt(String.self, .action)
        path = opt(String.self, .path)
        branch = opt(String.self, .branch)
        base = opt(String.self, .base)
        createdBy = opt(String.self, .createdBy)
        result = opt(String.self, .result)
        reason = opt(String.self, .reason)
        kind = opt(String.self, .kind)
        verdict = opt(String.self, .verdict)
        reasons = opt([String].self, .reasons)
        tier = opt(String.self, .tier)
        pr = opt(Int.self, .pr)
        url = opt(String.self, .url)
        comments = opt([PrCommentItem].self, .comments)
        commentCount = comments?.count
        replyId = opt(String.self, .replyId)
        phase = opt(String.self, .phase)
        note = opt(String.self, .note)
        at = opt(Double.self, .at)
        noteId = opt(String.self, .noteId)
        summary = opt(String.self, .summary)
        createdAt = opt(Double.self, .createdAt)
        precision = opt(String.self, .precision)
        status = opt(String.self, .status)
        city = opt(String.self, .city)
        region = opt(String.self, .region)
        country = opt(String.self, .country)
        timeZone = opt(String.self, .timeZone)
        latitude = opt(Double.self, .latitude)
        longitude = opt(Double.self, .longitude)
        accuracy = opt(Double.self, .accuracy)
        language = opt(String.self, .language)
        speed = opt(Double.self, .speed)
        replyMode = opt(String.self, .replyMode)
        ended = opt(Bool.self, .ended)
    }
}
