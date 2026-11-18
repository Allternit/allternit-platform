import Foundation

// -----------------------------------------------------------------------------
// What an agent asks the person: a question (gizzi `question` tool,
// `Question.Request` in cmd/gizzi-code/src/runtime/integrations/question) or a
// permission (gizzi `PermissionNext.Request`). Both arrive on the session
// stream (`question.asked` / `permission.asked`) and from the list routes
// (`GET /api/v1/questions`, `GET /api/v1/permissions`, agent_session_routes.rs).
//
// Decoding is tolerant on purpose: older runtimes omit the decision-tree
// fields, and a malformed option must never drop the whole request.
// -----------------------------------------------------------------------------

/// One choice of a question, or of a decision-tree follow-up.
struct AgentQuestionOption: Decodable, Sendable, Equatable, Hashable {
    let label: String
    let description: String?
    let recommended: Bool
    let caution: String?
    /// Decision tree: the follow-up shown when this option is picked.
    let then: AgentQuestionFollowUp?

    private enum CodingKeys: String, CodingKey {
        case label, description, recommended, caution, then
    }

    init(label: String, description: String? = nil, recommended: Bool = false, caution: String? = nil, then: AgentQuestionFollowUp? = nil) {
        self.label = label
        self.description = description
        self.recommended = recommended
        self.caution = caution
        self.then = then
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        label = (try? c.decode(String.self, forKey: .label)) ?? ""
        description = try? c.decodeIfPresent(String.self, forKey: .description)
        recommended = ((try? c.decodeIfPresent(Bool.self, forKey: .recommended)) ?? nil) ?? false
        caution = try? c.decodeIfPresent(String.self, forKey: .caution)
        let followUp = (try? c.decodeIfPresent(AgentQuestionFollowUp.self, forKey: .then)) ?? nil
        then = (followUp?.options.isEmpty == false) ? followUp : nil
    }
}

struct AgentQuestionFollowUp: Decodable, Sendable, Equatable, Hashable {
    let question: String
    let options: [AgentQuestionOption]

    private enum CodingKeys: String, CodingKey { case question, options }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        question = (try? c.decode(String.self, forKey: .question)) ?? ""
        options = ((try? c.decodeIfPresent([AgentQuestionOption].self, forKey: .options)) ?? nil)?
            .filter { !$0.label.isEmpty } ?? []
    }
}

/// One question (gizzi `Question.Info`).
struct AgentQuestionInfo: Decodable, Sendable, Equatable, Hashable {
    let question: String
    let header: String?
    let options: [AgentQuestionOption]
    let multiple: Bool
    /// Typed answer allowed. Session question cards default to true unless
    /// the question sets `custom: false` (web `customByDefault`).
    let custom: Bool
    /// Decision tree: "low" | "medium" | "high".
    let risk: String?
    /// Decision tree: the facts behind the question.
    let context: String?

    private enum CodingKeys: String, CodingKey {
        case question, header, options, multiple, custom, risk, context
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        question = (try? c.decode(String.self, forKey: .question)) ?? ""
        header = try? c.decodeIfPresent(String.self, forKey: .header)
        options = ((try? c.decodeIfPresent([AgentQuestionOption].self, forKey: .options)) ?? nil)?
            .filter { !$0.label.isEmpty } ?? []
        multiple = ((try? c.decodeIfPresent(Bool.self, forKey: .multiple)) ?? nil) ?? false
        custom = ((try? c.decodeIfPresent(Bool.self, forKey: .custom)) ?? nil) ?? true
        risk = try? c.decodeIfPresent(String.self, forKey: .risk)
        context = try? c.decodeIfPresent(String.self, forKey: .context)
    }

    /// gizzi `Question.isTree`: a follow-up exists, or risk/context is set.
    var isTree: Bool {
        !multiple && !options.isEmpty && (options.contains { $0.then != nil } || risk != nil || context != nil)
    }

    /// The card's title: a tree shows the full question; otherwise the
    /// short header leads and the question becomes the description
    /// (web `toQuestionConfigs`).
    var title: String {
        if isTree { return question }
        if let header, !header.isEmpty { return header }
        return question
    }

    var subtitle: String? {
        guard !isTree, let header, !header.isEmpty, header != question else { return nil }
        return question
    }
}

/// A pending question request: one or more questions from one tool call.
struct AgentQuestionRequest: Decodable, Sendable, Equatable, Identifiable, Hashable {
    let id: String
    let sessionID: String
    let questions: [AgentQuestionInfo]
    /// The `question` tool call this request belongs to, when known.
    let toolCallId: String?
    /// Non-blocking ask: no turn waits on it (no `tool` field). It can arrive
    /// while the session is idle; answering starts a new turn.
    let isAsync: Bool
    /// What the agent assumed and went on with, for async asks.
    let assumption: String?
    /// `time.created`, epoch milliseconds.
    let created: Double?

    private enum CodingKeys: String, CodingKey {
        case id, sessionID, sessionId, session_id, questions, tool, time
        case isAsync = "async"
        case assumption
    }
    private struct ToolRef: Decodable { let callID: String? }
    private struct TimeRef: Decodable { let created: Double? }

    init(id: String, sessionID: String, questions: [AgentQuestionInfo], toolCallId: String? = nil,
         isAsync: Bool = false, assumption: String? = nil, created: Double? = nil) {
        self.id = id
        self.sessionID = sessionID
        self.questions = questions
        self.toolCallId = toolCallId
        self.isAsync = isAsync
        self.assumption = assumption
        self.created = created
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        sessionID = ((try? c.decodeIfPresent(String.self, forKey: .sessionID)) ?? nil)
            ?? ((try? c.decodeIfPresent(String.self, forKey: .sessionId)) ?? nil)
            ?? ((try? c.decodeIfPresent(String.self, forKey: .session_id)) ?? nil)
            ?? ""
        questions = ((try? c.decodeIfPresent([AgentQuestionInfo].self, forKey: .questions)) ?? nil)?
            .filter { !$0.question.isEmpty } ?? []
        toolCallId = ((try? c.decodeIfPresent(ToolRef.self, forKey: .tool)) ?? nil)?.callID
        isAsync = ((try? c.decodeIfPresent(Bool.self, forKey: .isAsync)) ?? nil) ?? false
        let a = ((try? c.decodeIfPresent(String.self, forKey: .assumption)) ?? nil)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        assumption = (a?.isEmpty == false) ? a : nil
        created = ((try? c.decodeIfPresent(TimeRef.self, forKey: .time)) ?? nil)?.created
    }
}

/// A pending permission ask (gizzi `PermissionNext.Request`).
struct AgentPermissionAsk: Decodable, Sendable, Equatable, Identifiable, Hashable {
    let id: String
    let sessionID: String
    /// e.g. "edit", "bash", "webfetch".
    let permission: String
    let patterns: [String]
    /// The tool call waiting on this permission — the rail draws Deny /
    /// Allow on that row (web inline-approvals.ts).
    let toolCallId: String?
    /// run_code: the Python script the person is asked to approve (the
    /// first ~4 KB, trimmed by gizzi), the language, and the attached files
    /// it can read (`metadata.code / language / files`).
    let code: String?
    let language: String?
    let files: [String]
    /// metadata.review: class hand_off | confirm_now | confirm_unless_named | free.
    let reviewClass: String?
    let reviewRisk: String?
    let reviewAuthorized: Bool?
    let reviewReason: String?
    /// computer.access (`metadata.access`, computer-toolset/index.ts): one
    /// app name and its asked tier per entry, in the same order, plus why.
    let apps: [String]
    let tiers: [String]
    let accessReason: String?
    /// folder_access (`metadata.folderAccess`, builtins/folder-access.ts):
    /// the outside folders and action (request | request_delete).
    let paths: [String]
    let folderAction: String?

    private enum CodingKeys: String, CodingKey { case id, sessionID, sessionId, session_id, permission, patterns, tool, metadata }
    private struct Review: Decodable {
        let `class`: String?
        let risk: String?
        let authorized: Bool?
        let reason: String?
    }
    /// metadata.access.entries[]: {name, bundle_id, category?, max_tier?, tier}.
    private struct AccessEntry: Decodable {
        let name: String?
        let bundle_id: String?
        let tier: String?
    }
    /// metadata.access: {entries, reason, needed_tier?, granted_tier?, expires_in_secs?}.
    private struct Access: Decodable {
        let entries: [AccessEntry]?
        let reason: String?
    }
    /// metadata.folderAccess: {action, paths, reason, scope}.
    private struct FolderAccess: Decodable {
        let action: String?
        let paths: [String]?
        let reason: String?
    }
    private struct ToolRef: Decodable { let callID: String? }
    private struct Meta: Decodable {
        let code: String?
        let language: String?
        let files: [String]?
        let review: Review?
        let access: Access?
        let folderAccess: FolderAccess?
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        sessionID = ((try? c.decodeIfPresent(String.self, forKey: .sessionID)) ?? nil)
            ?? ((try? c.decodeIfPresent(String.self, forKey: .sessionId)) ?? nil)
            ?? ((try? c.decodeIfPresent(String.self, forKey: .session_id)) ?? nil)
            ?? ""
        permission = ((try? c.decodeIfPresent(String.self, forKey: .permission)) ?? nil) ?? "tool"
        patterns = ((try? c.decodeIfPresent([String].self, forKey: .patterns)) ?? nil) ?? []
        toolCallId = ((try? c.decodeIfPresent(ToolRef.self, forKey: .tool)) ?? nil)?.callID
        let meta = (try? c.decodeIfPresent(Meta.self, forKey: .metadata)) ?? nil
        code = meta?.code
        language = meta?.language
        files = meta?.files ?? []
        reviewClass = meta?.review?.class
        reviewRisk = meta?.review?.risk
        reviewAuthorized = meta?.review?.authorized
        reviewReason = meta?.review?.reason
        let entries = (meta?.access?.entries ?? []).filter { !($0.name ?? $0.bundle_id ?? "").isEmpty }
        apps = entries.map { ($0.name?.isEmpty == false ? $0.name : $0.bundle_id) ?? "" }
        tiers = entries.map { $0.tier ?? "read" }
        accessReason = meta?.access?.reason ?? meta?.folderAccess?.reason
        paths = (meta?.folderAccess?.paths ?? []).filter { !$0.isEmpty }
        folderAction = meta?.folderAccess?.action
    }

    /// "Run a command", "Edit files" — what the agent wants to do.
    var actionLabel: String {
        switch permission.lowercased() {
        case "bash", "shell": return "Run a command"
        case "edit", "write", "patch", "multiedit": return "Edit files"
        case "read": return "Read files"
        case "webfetch", "fetch": return "Open a web page"
        case "run_code": return "Run Python code"
        case "external_directory": return "Work outside the project folder"
        case "computer.access": return "Use apps on your computer"
        case "folder_access": return folderAction == "request_delete" ? "Delete from a folder" : "Open a folder"
        default: return "Use \(permission)"
        }
    }

    /// The first pattern, as a short detail line ("rm -rf build", "src/App.swift").
    var detail: String? {
        patterns.first(where: { !$0.isEmpty && $0 != "*" })
    }

    var isComputerAccess: Bool { permission.lowercased() == "computer.access" }
    var isFolderAccess: Bool { permission.lowercased() == "folder_access" }
    /// confirm_now asks never offer "Always".
    var allowsAlways: Bool { reviewClass != "confirm_now" }

    /// One line on why this ask is shown, from metadata.review.
    var reviewLine: String? {
        guard let cls = reviewClass, cls != "free" else { return nil }
        let label: String
        switch cls {
        case "hand_off": label = "Hand-off"
        case "confirm_now": label = "Confirm now"
        case "confirm_unless_named": label = "Confirm unless named"
        default: label = cls
        }
        var parts = [label]
        if let r = reviewRisk, !r.isEmpty { parts.append("\(r) risk") }
        if reviewAuthorized == true { parts.append("authorized") }
        var line = parts.joined(separator: " · ")
        if let r = reviewReason, !r.isEmpty { line += ". " + r }
        return line
    }
}
