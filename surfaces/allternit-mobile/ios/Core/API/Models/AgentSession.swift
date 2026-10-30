import Foundation

// -----------------------------------------------------------------------------
// Agent-sessions REST models — base path /api/v1/agent-sessions (LIVE path).
//
// Mirrors the web client's `BackendSession` / `BackendMessage` types in
// surfaces/ai.allternit.com/src/lib/agents/native-agent-api.ts:78-129, verified
// against the Rust producers in cmd/allternit-api/src/agent_session_routes.rs
// (`transform_session`:270-303, `transform_message`:351-383).
//
// The API emits snake_case keys on the wire; Swift properties stay camelCase
// via explicit CodingKeys. Optional fields mirror the web's
// `normalizeSessionPayload` fallbacks (native-agent-api.ts:346-363).
// -----------------------------------------------------------------------------

// MARK: - Session record

/// One agent session (`ses_*` id), as listed/created by the backend.
struct AgentSession: Decodable, Sendable, Identifiable {
    let id: String
    /// `var` only so a rename can update the list optimistically (`renamed`).
    var name: String?
    let description: String?
    let createdAt: String
    let updatedAt: String
    let lastAccessed: String
    let messageCount: Int
    let active: Bool
    let tags: [String]
    /// `metadata.originSurface` — the original frontend surface
    /// ("chat" | "cowork" | "code" | …). The API normalizes the surface for
    /// Gizzi on create but remembers the original in its
    /// `session_origin_surface` table and restores it here on read
    /// (agent_session_routes.rs:274-301). Sessions with no recorded surface
    /// decode as "" (Rust `unwrap_or_default`), never nil.
    let originSurface: String?
    /// `metadata.agent_id` — set when the session was created with a registry
    /// agent (create forwards `agentID`, agent_session_routes.rs:597-602).
    /// The wire has NO `session_mode` field: the web keeps regular/agent in
    /// local-only store metadata (mode-session-store.ts:72-77), so this is the
    /// only wire signal distinguishing agent sessions from regular ones.
    let agentId: String?
    /// `metadata.project_id` — the cowork project this session was stamped
    /// with at create (agent_session_routes.rs:294). Nil for unassigned
    /// sessions; the Projects feature groups chats by this id.
    let projectId: String?
    /// `metadata.ephemeral` — incognito chats (Phase 6). The backend excludes
    /// these from list responses. Decodes the bool the backend emits or the
    /// "true" string form (the create path carries the flag as a string
    /// inside the metadata bag).
    let ephemeral: Bool
    /// `metadata.agentName` — the agent or bot the session runs as (web
    /// DecisionsDeck "<agent> asks").
    let agentName: String?
    /// `metadata.archived` — closed from the session overview on the web
    /// (useModeOverview `archiveOverviewItem`). Archived sessions leave the
    /// open lists even while `active` is still true.
    let archived: Bool
    // The rest of `metadata` (directory, version, permission, …) stays
    // skipped — not rendered anywhere on iOS.

    enum CodingKeys: String, CodingKey {
        case id, name, description
        case createdAt = "created_at"
        case updatedAt = "updated_at"
        case lastAccessed = "last_accessed"
        case messageCount = "message_count"
        case active, tags
        case metadata
    }

    /// Keys inside the `metadata` object emitted by `transform_session`
    /// (agent_session_routes.rs:293-301). Note the mixed casing on the wire:
    /// camelCase `originSurface` vs snake_case `agent_id` / `project_id`.
    private enum MetadataKeys: String, CodingKey {
        case originSurface
        case agentId = "agent_id"
        case projectId = "project_id"
        case ephemeral
        case agentName
        case archived
    }

    /// Tolerant decode mirroring the web's `normalizeSessionPayload`: only
    /// `id` and `created_at` are required on the wire; everything else falls
    /// back (`updated_at` → `created_at`, `message_count` → 0, …).
    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(String.self, forKey: .id)
        name = try container.decodeIfPresent(String.self, forKey: .name)
        description = try container.decodeIfPresent(String.self, forKey: .description)
        createdAt = try container.decode(String.self, forKey: .createdAt)
        let updatedAt = try container.decodeIfPresent(String.self, forKey: .updatedAt) ?? createdAt
        self.updatedAt = updatedAt
        lastAccessed = try container.decodeIfPresent(String.self, forKey: .lastAccessed) ?? updatedAt
        messageCount = try container.decodeIfPresent(Int.self, forKey: .messageCount) ?? 0
        active = try container.decodeIfPresent(Bool.self, forKey: .active) ?? true
        tags = try container.decodeIfPresent([String].self, forKey: .tags) ?? []
        if container.contains(.metadata), try !container.decodeNil(forKey: .metadata) {
            let metadata = try container.nestedContainer(keyedBy: MetadataKeys.self, forKey: .metadata)
            originSurface = try metadata.decodeIfPresent(String.self, forKey: .originSurface)
            agentId = try metadata.decodeIfPresent(String.self, forKey: .agentId)
            projectId = try metadata.decodeIfPresent(String.self, forKey: .projectId)
            agentName = (try? metadata.decodeIfPresent(String.self, forKey: .agentName)) ?? nil
            archived = ((try? metadata.decodeIfPresent(Bool.self, forKey: .archived)) ?? nil) ?? false
            // The backend emits a bool; the create path carries the flag as
            // the string "true" inside the metadata bag.
            if let flag = try metadata.decodeIfPresent(Bool.self, forKey: .ephemeral) {
                ephemeral = flag
            } else {
                ephemeral = (try metadata.decodeIfPresent(String.self, forKey: .ephemeral)) == "true"
            }
        } else {
            originSurface = nil
            agentId = nil
            projectId = nil
            agentName = nil
            archived = false
            ephemeral = false
        }
    }
}

extension AgentSession {
    /// A copy with a new name (optimistic rename in ShellSessionsStore).
    func renamed(_ newName: String) -> AgentSession {
        var copy = self
        copy.name = newName
        return copy
    }
}

/// `GET /api/v1/agent-sessions` envelope (`{ sessions, count }`,
/// agent_session_routes.rs:537-540).
struct AgentSessionListResponse: Decodable, Sendable {
    let sessions: [AgentSession]
    let count: Int
}

// MARK: - Context window

/// `GET /api/v1/agent-sessions/:id/context` envelope (native-agent-api.ts
/// `SessionContextUsage`); `breakdown` is null before the first turn.
struct SessionContextResponse: Decodable, Sendable {
    let breakdown: SessionContextBreakdown?
}

/// What fills a session's context window (gizzi `ContextBreakdown.Snapshot`):
/// the totals the composer meter shows, and the rows its panel lists.
struct SessionContextBreakdown: Decodable, Sendable, Equatable {
    struct Category: Decodable, Sendable, Equatable {
        let id: String
        let name: String
        let tokens: Double
        let deferred: Bool?
    }

    let contextWindow: Double
    let usedTokens: Double
    /// 0–1
    let utilization: Double
    let basis: String?
    let source: String?
    let brainName: String?
    let categories: [Category]
    /// The MCP tools the runtime loaded, by server (the Connectors card).
    let mcpTools: [McpTool]?

    struct McpTool: Decodable, Sendable, Equatable {
        let name: String
        let server: String
        let tokens: Double
    }

    /// ContextUsage.tsx `contextLevel`: amber from 75%, red from 90%.
    var level: Level {
        utilization >= 0.9 ? .reached : utilization >= 0.75 ? .approaching : .ok
    }

    enum Level { case ok, approaching, reached }

    /// 144600 → "144.6k", 1000000 → "1M" (ContextUsage.tsx `formatTokens`).
    static func format(_ tokens: Double) -> String {
        let n = max(0, tokens.rounded())
        func trim(_ value: Double) -> String {
            let text = String(format: "%.1f", value)
            return text.hasSuffix(".0") ? String(text.dropLast(2)) : text
        }
        if n >= 1_000_000 { return "\(trim(n / 1_000_000))M" }
        if n >= 1_000 { return "\(trim(n / 1_000))k" }
        return String(Int(n))
    }
}

// MARK: - Background tasks

/// One piece of work a session delegated (gizzi `SessionTasks.Item`): a
/// background shell, or a subagent. `childSessionID` is the subagent's
/// transcript.
struct SessionTask: Decodable, Sendable, Identifiable, Equatable {
    struct Time: Decodable, Sendable, Equatable {
        let created: Double
        let updated: Double?
        let finished: Double?
    }

    let id: String
    /// "shell" | "subagent" | …
    let kind: String
    /// queued | running | completed | failed | cancelled | interrupted
    let status: String
    let background: Bool?
    let description: String
    let agent: String?
    let childSessionID: String?
    let output: String?
    let error: String?
    let time: Time

    var isActive: Bool { status == "queued" || status == "running" }

    /// CodeTasksPane `kindLabel`: "Shell", "Agent · explore".
    var kindLabel: String {
        switch kind {
        case "shell": return "Shell"
        case "subagent": return agent.map { "Agent · \($0)" } ?? "Agent"
        default: return kind.prefix(1).uppercased() + kind.dropFirst()
        }
    }
}

// MARK: - Pull requests

/// A pull request linked to a session (code-session-prs.ts `LinkedPR`).
struct LinkedPullRequest: Decodable, Sendable, Identifiable, Equatable {
    let url: String
    let repo: String
    let number: Int
    let title: String
    /// merged | conflict | open | draft | closed
    let state: String
    let additions: Int?
    let deletions: Int?

    var id: String { url }
}

struct SessionPullRequestsResponse: Decodable, Sendable {
    let bySession: [String: [LinkedPullRequest]]?
}

// MARK: - Create request

/// Body of `POST /api/v1/agent-sessions`, matching the web's
/// `sessionApi.createSession` payload (native-agent-api.ts:450-474).
///
/// `origin_surface` / `session_mode` are dynamic: the composer passes the
/// current app mode ("chat" | "cowork") and "agent" when the agent pill is
/// on (mode-session-store.ts:849-850,892-893). `session_mode` is sent for
/// parity even though the Rust handler (`CreateSessionBody`,
/// agent_session_routes.rs:138-146) doesn't read it — serde drops it.
///
/// `agent_id` / `agent_name` / `metadata` are read by the backend; nil
/// optionals are omitted from the JSON body (synthesized Encodable skips
/// them), matching the web dropping `undefined`. The web carries the
/// composer's selected agent-mode tile as `metadata.agentModeId`
/// (mode-session-store.ts:897-905,952) — mirrored here.
struct CreateAgentSessionRequest: Encodable, Sendable {
    let name: String
    let originSurface: String
    let sessionMode: String
    let agentId: String?
    let agentName: String?
    let metadata: [String: String]?

    enum CodingKeys: String, CodingKey {
        case name
        case originSurface = "origin_surface"
        case sessionMode = "session_mode"
        case agentId = "agent_id"
        case agentName = "agent_name"
        case metadata
    }
}

// MARK: - Message records

/// One message in a session (`GET /api/v1/agent-sessions/:id/messages` →
/// bare array). Mirrors `BackendMessage` (native-agent-api.ts:91-98) plus the
/// `metadata.parts` the Rust `transform_message` embeds
/// (agent_session_routes.rs:364-382).
///
/// Note: the backend's `content` already includes reasoning-part text
/// (`extract_message_content` matches "text" | "reasoning" | "agent") AND
/// repeats reasoning separately in `thinking` — the web renders both as-is,
/// and so do we.
struct AgentSessionMessage: Decodable, Sendable, Identifiable {
    /// `GizziMessagePart` as serialized inside `metadata.parts`
    /// (agent_session_routes.rs `GizziMessagePart`). A tool part's `state`
    /// (`serde_json::Value`, gizzi's ToolPart state) feeds the tool calls
    /// rail on reloaded chats; it decodes tolerantly and never fails the
    /// message.
    struct Part: Decodable, Sendable {
        let type: String
        let text: String?
        let filename: String?
        let url: String?
        let tool: String?
        let id: String?
        let state: ToolState?

        /// gizzi tool state, as much as the rail draws. Never throws.
        struct ToolState: Decodable, Sendable {
            let status: String?
            let input: [String: String]
            let title: String?
            let error: String?
            let startedAt: Date?
            let finishedAt: Date?
            /// The input as sent (lists and objects kept), for the todo
            /// tools' task list (CodeProgress).
            let rawInput: [String: AnyCodable]?
            /// The tool's result (gizzi `state.output`).
            let output: AnyCodable?

            private enum CodingKeys: String, CodingKey { case status, input, title, error, time, output }
            private struct Time: Decodable { let start: Double?; let end: Double? }

            init(from decoder: Decoder) throws {
                let c = try? decoder.container(keyedBy: CodingKeys.self)
                status = (try? c?.decodeIfPresent(String.self, forKey: .status)) ?? nil
                title = (try? c?.decodeIfPresent(String.self, forKey: .title)) ?? nil
                error = (try? c?.decodeIfPresent(String.self, forKey: .error)) ?? nil
                let raw = (try? c?.decodeIfPresent([String: AnyCodable].self, forKey: .input)) ?? nil
                rawInput = raw
                output = (try? c?.decodeIfPresent(AnyCodable.self, forKey: .output)) ?? nil
                input = SessionWorkerEvent.PartInfo.PartState.flatten(raw)
                let time = (try? c?.decodeIfPresent(Time.self, forKey: .time)) ?? nil
                startedAt = time?.start.map { Date(timeIntervalSince1970: $0 / 1000) }
                finishedAt = time?.end.map { Date(timeIntervalSince1970: $0 / 1000) }
            }
        }
    }

    /// `metadata: { agent, model, parts, error }` — only `parts` is decoded;
    /// `agent`/`model`/`error` are `unknown`-typed in TS and skipped.
    struct Metadata: Decodable, Sendable {
        let parts: [Part]?
    }

    let id: String
    let role: String // "user" | "assistant" (Gizzi role strings)
    let content: String
    let thinking: String?
    let timestamp: String
    let metadata: Metadata?
}
