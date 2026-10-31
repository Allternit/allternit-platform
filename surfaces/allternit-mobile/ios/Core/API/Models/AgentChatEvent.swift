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
            self = .toolResult(ToolResult(
                toolCallId: try container.decodeIfPresent(String.self, forKey: .toolCallId) ?? "",
                toolName: try container.decodeIfPresent(String.self, forKey: .toolName) ?? "Tool"
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
            content: finalContent
        )
    }

    private static let supportedArtifactKinds: Set<String> = [
        "image", "svg", "mermaid", "jsx", "html", "document",
        "slides", "sheet", "audio", "video", "podcast"
    ]
}
