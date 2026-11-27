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
    /// `metadata.artifactSession.kind` — the session is bound to an artifact
    /// (web artifact-session.ts), so it opens beside its editor, not as a chat.
    let artifactSessionKind: String?
    /// `metadata.sourceRef.harness == "gizzi-tui"` — a live Gizzi terminal
    /// session. It runs in the terminal; messages, Stop and approvals sent from
    /// here reach it at once (docs: Continue a terminal session anywhere).
    let isTerminal: Bool
    /// `metadata.mission.label` — Gizzi /mission progress, "Mission · M2/5".
    /// nil when absent or malformed.
    let missionLabel: String?
    /// `metadata.glance.text` — a one-line glance summary. nil when absent
    /// or malformed.
    let glanceText: String?
    /// `metadata.needsInput` — server-owned (code_session_status.rs): the
    /// agent is waiting on the user (a permission prompt or a question).
    /// Missing = false.
    let needsInput: Bool
    /// `metadata.needsInputKind` — "permission" | "question" while
    /// `needsInput`; nil otherwise.
    let needsInputKind: String?
    /// `metadata.needsInputAt` — ISO 8601 time the wait started.
    let needsInputAt: String?
    /// `metadata.readyForReview` — server-owned: the last run changed files
    /// or made an artifact and nobody has opened the session since. Cleared
    /// by `CodeSessionsClient.markReviewed`. Missing = false.
    let readyForReview: Bool
    /// `metadata.readyForReviewAt` — ISO 8601 time the run ended.
    let readyForReviewAt: String?
    /// `metadata.completed` — the session was marked finished (web
    /// code-session-state.ts; the Code sessions view's Done, C6). Missing = false.
    let completed: Bool
    /// `metadata.ended` {at, reason}: the chat was ended (end_conversation). It
    /// refuses new prompts; the composer shows "Start a new chat". nil = open.
    let ended: EndedInfo?
    /// `metadata.worktree.branch` / `.path` — the session runs in a git
    /// worktree (agent_session_routes.rs). nil when not in one.
    let worktreeBranch: String?
    let worktreePath: String?
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
        case artifactSession
        case sourceRef
        case mission
        case glance
        case needsInput
        case needsInputKind
        case needsInputAt
        case readyForReview
        case readyForReviewAt
        case completed
        case ended
        case worktree
    }

    struct EndedInfo: Decodable, Sendable, Equatable { let at: String?; let reason: String? }
    private struct ArtifactSessionTag: Decodable { let kind: String? }
    private struct SourceRefTag: Decodable { let harness: String? }
    private struct MissionTag: Decodable { let label: String? }
    private struct GlanceTag: Decodable { let text: String? }
    private struct WorktreeTag: Decodable { let path: String?; let branch: String? }

    private static func clean(_ text: String?) -> String? {
        let t = text?.trimmingCharacters(in: .whitespacesAndNewlines)
        return (t?.isEmpty ?? true) ? nil : t
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
            artifactSessionKind = ((try? metadata.decodeIfPresent(ArtifactSessionTag.self, forKey: .artifactSession)) ?? nil)?.kind
            isTerminal = ((try? metadata.decodeIfPresent(SourceRefTag.self, forKey: .sourceRef)) ?? nil)?.harness == "gizzi-tui"
            missionLabel = Self.clean(((try? metadata.decodeIfPresent(MissionTag.self, forKey: .mission)) ?? nil)?.label)
            glanceText = Self.clean(((try? metadata.decodeIfPresent(GlanceTag.self, forKey: .glance)) ?? nil)?.text)
            let worktree = ((try? metadata.decodeIfPresent(WorktreeTag.self, forKey: .worktree)) ?? nil)
            worktreeBranch = Self.clean(worktree?.branch)
            worktreePath = Self.clean(worktree?.path)
            needsInput = ((try? metadata.decodeIfPresent(Bool.self, forKey: .needsInput)) ?? nil) ?? false
            needsInputKind = needsInput ? Self.clean((try? metadata.decodeIfPresent(String.self, forKey: .needsInputKind)) ?? nil) : nil
            needsInputAt = needsInput ? ((try? metadata.decodeIfPresent(String.self, forKey: .needsInputAt)) ?? nil) : nil
            readyForReview = ((try? metadata.decodeIfPresent(Bool.self, forKey: .readyForReview)) ?? nil) ?? false
            readyForReviewAt = readyForReview ? ((try? metadata.decodeIfPresent(String.self, forKey: .readyForReviewAt)) ?? nil) : nil
            completed = ((try? metadata.decodeIfPresent(Bool.self, forKey: .completed)) ?? nil) ?? false
            ended = ((try? metadata.decodeIfPresent(EndedInfo.self, forKey: .ended)) ?? nil)
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
            artifactSessionKind = nil
            isTerminal = false
            missionLabel = nil
            glanceText = nil
            needsInput = false
            needsInputKind = nil
            needsInputAt = nil
            readyForReview = false
            readyForReviewAt = nil
            completed = false
            ended = nil
            worktreeBranch = nil
            worktreePath = nil
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
struct SessionContextBreakdown: Decodable, Sendable, Hashable {
    struct Category: Decodable, Sendable, Hashable {
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

    struct McpTool: Decodable, Sendable, Hashable {
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

/// The Tasks pane's view of a session's delegated work (`SessionTask`,
/// RunTreeModel.swift).
extension SessionTask {
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
        /// run_code parts only: produced files and the run summary.
        let files: [RunCodeResult.File]?
        let run: RunCodeResult.Run?
        /// File parts: `source.uri`, `tool-call://<callID>` for an engine picture that belongs to that call's card.
        let sourceUri: String?

        private enum CodingKeys: String, CodingKey {
            case type, text, filename, url, tool, id, state, files, run, source
        }

        private struct Source: Decodable { let uri: String? }

        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            type = try c.decode(String.self, forKey: .type)
            text = try c.decodeIfPresent(String.self, forKey: .text)
            filename = try c.decodeIfPresent(String.self, forKey: .filename)
            url = try c.decodeIfPresent(String.self, forKey: .url)
            tool = try c.decodeIfPresent(String.self, forKey: .tool)
            id = try c.decodeIfPresent(String.self, forKey: .id)
            state = try c.decodeIfPresent(ToolState.self, forKey: .state)
            files = (try? c.decodeIfPresent(LossyArray<RunCodeResult.File>.self, forKey: .files))??.elements
            run = (try? c.decodeIfPresent(RunCodeResult.Run.self, forKey: .run)) ?? nil
            sourceUri = ((try? c.decodeIfPresent(Source.self, forKey: .source)) ?? nil)?.uri
        }

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
            /// `state.metadata` read as a send_user_message card; nil when it
            /// isn't one (the record keeps it only for that tool).
            let userMessage: AgentUserMessage?
            /// `state.metadata` read as an automation result; nil when it
            /// isn't one (the record keeps it only for that tool).
            let automation: AgentAutomationResult?
            /// `state.metadata` read as lookup's `{kind, card?, credit?}`; nil otherwise.
            let lookup: LookupToolMetadata?
            /// `state.metadata` as raw JSON text (connector_suggest and
            /// settings cards read it).
            let metadata: String?

            private enum CodingKeys: String, CodingKey { case status, input, title, error, time, output, metadata }
            private struct Time: Decodable { let start: Double?; let end: Double? }

            init(from decoder: Decoder) throws {
                let c = try? decoder.container(keyedBy: CodingKeys.self)
                status = (try? c?.decodeIfPresent(String.self, forKey: .status)) ?? nil
                title = (try? c?.decodeIfPresent(String.self, forKey: .title)) ?? nil
                error = (try? c?.decodeIfPresent(String.self, forKey: .error)) ?? nil
                let raw = (try? c?.decodeIfPresent([String: AnyCodable].self, forKey: .input)) ?? nil
                rawInput = raw
                output = (try? c?.decodeIfPresent(AnyCodable.self, forKey: .output)) ?? nil
                userMessage = (try? c?.decodeIfPresent(AgentUserMessage.self, forKey: .metadata)) ?? nil
                automation = (try? c?.decodeIfPresent(AgentAutomationResult.self, forKey: .metadata)) ?? nil
                lookup = (try? c?.decodeIfPresent(LookupToolMetadata.self, forKey: .metadata)) ?? nil
                metadata = AgentToolCards.jsonText((try? c?.decodeIfPresent(AnyCodable.self, forKey: .metadata)) ?? nil)
                input = SessionWorkerEvent.PartInfo.PartState.flatten(raw)
                let time = (try? c?.decodeIfPresent(Time.self, forKey: .time)) ?? nil
                startedAt = time?.start.map { Date(timeIntervalSince1970: $0 / 1000) }
                finishedAt = time?.end.map { Date(timeIntervalSince1970: $0 / 1000) }
            }
        }
    }

    /// `metadata: { agent, model, parts, error, memoryRefs }` — `parts` and
    /// `memoryRefs` are decoded; `agent`/`model`/`error` are `unknown`-typed
    /// in TS and skipped. A bad `memoryRefs` entry is dropped, never the parts.
    struct Metadata: Decodable, Sendable {
        let parts: [Part]?
        /// Memories the reply used or changed (≤ 10, from memory tool results).
        let memoryRefs: [MemoryRef]?

        private enum CodingKeys: String, CodingKey { case parts, memoryRefs }

        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            parts = try c.decodeIfPresent([Part].self, forKey: .parts)
            memoryRefs = (try? c.decodeIfPresent(LossyArray<MemoryRef>.self, forKey: .memoryRefs))??.elements
        }
    }

    let id: String
    let role: String // "user" | "assistant" (Gizzi role strings)
    let content: String
    let thinking: String?
    let timestamp: String
    let metadata: Metadata?
}

/// One memory a reply used or changed: `metadata.memoryRefs[]` on a stored
/// message (`info.memory_refs` on the runtime message).
struct MemoryRef: Decodable, Equatable, Hashable, Sendable {
    enum Action: String, Sendable { case recalled, saved, updated, deleted }

    let id: String
    let title: String?
    /// The raw action; `kind` reads it, unknown values count as recalled.
    let action: String
    /// `"past_chat"` for a past conversation; absent for a saved memory.
    let source: String?
    /// ISO date of the past chat's turn.
    let date: String?
    /// The past chat's session id (`session_id` or `sessionId`).
    let sessionId: String?

    var isPastChat: Bool { source == "past_chat" }

    var kind: Action { Action(rawValue: action) ?? .recalled }

    private enum CodingKeys: String, CodingKey {
        case id, title, action, source, date
        case sessionIdSnake = "session_id"
        case sessionIdCamel = "sessionId"
    }

    init(id: String, title: String? = nil, action: String,
         source: String? = nil, date: String? = nil, sessionId: String? = nil) {
        self.id = id
        self.title = title
        self.action = action
        self.source = source
        self.date = date
        self.sessionId = sessionId
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        title = (try? c.decodeIfPresent(String.self, forKey: .title)) ?? nil
        action = (try? c.decodeIfPresent(String.self, forKey: .action)) ?? "recalled"
        source = (try? c.decodeIfPresent(String.self, forKey: .source)) ?? nil
        date = (try? c.decodeIfPresent(String.self, forKey: .date)) ?? nil
        sessionId = ((try? c.decodeIfPresent(String.self, forKey: .sessionIdSnake)) ?? nil)
            ?? ((try? c.decodeIfPresent(String.self, forKey: .sessionIdCamel)) ?? nil)
    }
}

// MARK: - Display title (UI feedback K4)

extension AgentSession {
    /// True for a name the runtime made up ("New session - 2026-11-17T...",
    /// "New thread 2026-11-17", "Untitled"): it must never show as a title.
    /// Same rule as allternit-ai `session-display-title.ts`.
    static func isRawDefaultName(_ name: String?) -> Bool {
        let n = (name ?? "").trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        if n.isEmpty || n == "untitled" || n == "new session" || n == "new thread" || n == "new chat" { return true }
        let prefixes = ["new session", "new thread", "new chat", "untitled"]
        return prefixes.contains { n.hasPrefix($0) } && n.contains { $0.isNumber }
    }

    /// The name people see: the session's own name, else "New session · Nov 17"
    /// from its created date. (The list wire carries no first prompt.)
    var displayTitle: String {
        if !Self.isRawDefaultName(name) { return (name ?? "").trimmingCharacters(in: .whitespacesAndNewlines) }
        let date = (try? Date(createdAt, strategy: Date.ISO8601FormatStyle(includingFractionalSeconds: true)))
            ?? (try? Date(createdAt, strategy: Date.ISO8601FormatStyle()))
        guard let date else { return "New session" }
        return "New session · " + date.formatted(.dateTime.month(.abbreviated).day())
    }
}
