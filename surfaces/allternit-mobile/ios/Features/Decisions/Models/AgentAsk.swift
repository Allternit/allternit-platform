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

    private enum CodingKeys: String, CodingKey { case id, sessionID, sessionId, session_id, questions, tool }
    private struct ToolRef: Decodable { let callID: String? }

    init(id: String, sessionID: String, questions: [AgentQuestionInfo], toolCallId: String? = nil) {
        self.id = id
        self.sessionID = sessionID
        self.questions = questions
        self.toolCallId = toolCallId
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

    private enum CodingKeys: String, CodingKey { case id, sessionID, sessionId, session_id, permission, patterns, tool }
    private struct ToolRef: Decodable { let callID: String? }

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
    }

    /// "Run a command", "Edit files" — what the agent wants to do.
    var actionLabel: String {
        switch permission.lowercased() {
        case "bash", "shell": return "Run a command"
        case "edit", "write", "patch", "multiedit": return "Edit files"
        case "read": return "Read files"
        case "webfetch", "fetch": return "Open a web page"
        case "external_directory": return "Work outside the project folder"
        default: return "Use \(permission)"
        }
    }

    /// The first pattern, as a short detail line ("rm -rf build", "src/App.swift").
    var detail: String? {
        patterns.first(where: { !$0.isEmpty && $0 != "*" })
    }
}
