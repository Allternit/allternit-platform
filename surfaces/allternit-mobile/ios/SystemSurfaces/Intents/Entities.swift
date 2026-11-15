import AppIntents
import Foundation

/// Directory reads for entity pickers. The registered service answers in the
/// app; in an extension the App Group snapshot lists bots, approvals and the
/// last routine so pickers still work.
private enum IntentDirectory {
    static func bots() async -> [BotInfo] {
        if let svc = IntentServiceRegistry.current, let list = try? await svc.listBots(), !list.isEmpty { return list }
        return (AppGroupStore.snapshot?.bots ?? []).map { BotInfo(id: $0.botId, name: $0.name, status: $0.status, step: $0.step) }
    }

    static func approvals() async -> [ApprovalInfo] {
        if let svc = IntentServiceRegistry.current, let list = try? await svc.listApprovals() { return list.items }
        return (AppGroupStore.snapshot?.needsYouRows ?? []).map {
            ApprovalInfo(id: $0.id, kind: $0.kind, botId: "", botName: $0.botName, summary: $0.line,
                         fullText: $0.line, canApprove: false)
        }
    }

    static func routines() async -> [RoutineInfo] {
        if let svc = IntentServiceRegistry.current, let list = try? await svc.listRoutines(), !list.isEmpty { return list }
        if let r = AppGroupStore.snapshot?.lastRoutine, !r.routineId.isEmpty {
            return [RoutineInfo(id: r.routineId, name: r.title, state: r.status)]
        }
        return []
    }

    static func matches(_ haystack: [String], _ needle: String) -> Bool {
        let n = needle.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !n.isEmpty else { return true }
        return haystack.contains { $0.lowercased().contains(n) }
    }

    /// Exact id first, else a spoken name (case-insensitive, whole then partial).
    static func pick<T>(_ items: [T], id: String, name: (T) -> String, itemId: (T) -> String) -> T? {
        if let hit = items.first(where: { itemId($0) == id }) { return hit }
        let n = id.lowercased()
        return items.first { name($0).lowercased() == n } ?? items.first { name($0).lowercased().contains(n) }
    }
}

// MARK: Bot

struct BotEntity: AppEntity, Sendable {
    static let typeDisplayRepresentation: TypeDisplayRepresentation = "Bot"
    static let defaultQuery = BotQuery()

    var id: String
    var name: String
    var status: String

    init(_ info: BotInfo) { id = info.id; name = info.name; status = info.status }

    var info: BotInfo { BotInfo(id: id, name: name, status: status) }
    var displayRepresentation: DisplayRepresentation {
        DisplayRepresentation(title: "\(name)", subtitle: status.isEmpty ? nil : "\(status)")
    }
}

struct BotQuery: EntityStringQuery {
    init() {}
    /// By id, and by spoken name when the id is not a known bot id.
    func entities(for identifiers: [String]) async throws -> [BotEntity] {
        let bots = await IntentDirectory.bots()
        return identifiers.compactMap { ident in
            IntentDirectory.pick(bots, id: ident, name: \.name, itemId: \.id).map(BotEntity.init)
        }
    }
    func entities(matching string: String) async throws -> [BotEntity] {
        await IntentDirectory.bots().filter { IntentDirectory.matches([$0.name], string) }.map(BotEntity.init)
    }
    func suggestedEntities() async throws -> [BotEntity] { await IntentDirectory.bots().map(BotEntity.init) }
}

// MARK: Conversation

struct ConversationEntity: AppEntity, Sendable {
    static let typeDisplayRepresentation: TypeDisplayRepresentation = "Conversation"
    static let defaultQuery = ConversationQuery()

    var id: String
    var title: String
    var sessionId: String?
    var botId: String?

    init(_ info: ConversationInfo) { id = info.id; title = info.title; sessionId = info.sessionId; botId = info.botId }

    var info: ConversationInfo { ConversationInfo(id: id, sessionId: sessionId, title: title, botId: botId) }
    var displayRepresentation: DisplayRepresentation { DisplayRepresentation(title: "\(title)") }
}

struct ConversationQuery: EntityStringQuery {
    init() {}
    private func all(_ q: String?) async -> [ConversationEntity] {
        guard let svc = IntentServiceRegistry.current,
              let list = try? await svc.listConversations(matching: q) else { return [] }
        return list.map(ConversationEntity.init)
    }
    func entities(for identifiers: [String]) async throws -> [ConversationEntity] {
        let list = await all(nil)
        return identifiers.compactMap { id in list.first { $0.id == id || $0.sessionId == id } }
    }
    func entities(matching string: String) async throws -> [ConversationEntity] { await all(string) }
    func suggestedEntities() async throws -> [ConversationEntity] { Array(await all(nil).prefix(10)) }
}

// MARK: Routine

struct RoutineEntity: AppEntity, Sendable {
    static let typeDisplayRepresentation: TypeDisplayRepresentation = "Routine"
    static let defaultQuery = RoutineQuery()

    var id: String
    var name: String
    var state: String

    init(_ info: RoutineInfo) { id = info.id; name = info.name; state = info.state }

    var info: RoutineInfo { RoutineInfo(id: id, name: name, state: state) }
    var displayRepresentation: DisplayRepresentation {
        DisplayRepresentation(title: "\(name)", subtitle: state.isEmpty ? nil : "\(state)")
    }
}

struct RoutineQuery: EntityStringQuery {
    init() {}
    func entities(for identifiers: [String]) async throws -> [RoutineEntity] {
        let list = await IntentDirectory.routines()
        return identifiers.compactMap { id in
            IntentDirectory.pick(list, id: id, name: \.name, itemId: \.id).map(RoutineEntity.init)
        }
    }
    func entities(matching string: String) async throws -> [RoutineEntity] {
        await IntentDirectory.routines().filter { IntentDirectory.matches([$0.name], string) }.map(RoutineEntity.init)
    }
    func suggestedEntities() async throws -> [RoutineEntity] { await IntentDirectory.routines().map(RoutineEntity.init) }
}

// MARK: Approval (inbox item). Never indexed in Spotlight.

struct ApprovalEntity: AppEntity, Sendable {
    static let typeDisplayRepresentation: TypeDisplayRepresentation = "Approval"
    static let defaultQuery = ApprovalQuery()

    var id: String
    var botName: String
    var summary: String
    var info: ApprovalInfo

    init(_ info: ApprovalInfo) { id = info.id; botName = info.botName; summary = info.summary; self.info = info }

    var displayRepresentation: DisplayRepresentation {
        DisplayRepresentation(title: "\(summary)", subtitle: "\(botName)")
    }
}

struct ApprovalQuery: EntityQuery {
    init() {}
    func entities(for identifiers: [String]) async throws -> [ApprovalEntity] {
        let list = await IntentDirectory.approvals()
        return identifiers.compactMap { id in list.first { $0.id == id }.map(ApprovalEntity.init) }
    }
    func suggestedEntities() async throws -> [ApprovalEntity] { await IntentDirectory.approvals().map(ApprovalEntity.init) }
}

// MARK: Computer

struct ComputerEntity: AppEntity, Sendable {
    static let typeDisplayRepresentation: TypeDisplayRepresentation = "Computer"
    static let defaultQuery = ComputerQuery()

    var id: String
    var name: String
    var isOnline: Bool

    init(_ info: ComputerInfo) { id = info.id; name = info.name; isOnline = info.isOnline }

    var displayRepresentation: DisplayRepresentation {
        DisplayRepresentation(title: "\(name)", subtitle: isOnline ? "Online" : "Asleep or offline")
    }
}

struct ComputerQuery: EntityStringQuery {
    init() {}
    private func all() async -> [ComputerEntity] {
        guard let svc = IntentServiceRegistry.current, let list = try? await svc.listComputers() else { return [] }
        return list.map(ComputerEntity.init)
    }
    func entities(for identifiers: [String]) async throws -> [ComputerEntity] {
        let list = await all()
        return identifiers.compactMap { IntentDirectory.pick(list, id: $0, name: \.name, itemId: \.id) }
    }
    func entities(matching string: String) async throws -> [ComputerEntity] {
        await all().filter { IntentDirectory.matches([$0.name], string) }
    }
    func suggestedEntities() async throws -> [ComputerEntity] { await all() }
}

// MARK: Model (a bot's brain)

struct ModelEntity: AppEntity, Sendable {
    static let typeDisplayRepresentation: TypeDisplayRepresentation = "Model"
    static let defaultQuery = ModelQuery()

    var id: String
    var name: String
    var provider: String

    init(_ info: ModelInfo) { id = info.id; name = info.name; provider = info.provider }

    var displayRepresentation: DisplayRepresentation {
        DisplayRepresentation(title: "\(name)", subtitle: "\(provider)")
    }
}

struct ModelQuery: EntityStringQuery {
    init() {}
    private func all() async -> [ModelEntity] {
        guard let svc = IntentServiceRegistry.current, let list = try? await svc.listModels() else { return [] }
        return list.map(ModelEntity.init)
    }
    func entities(for identifiers: [String]) async throws -> [ModelEntity] {
        let list = await all()
        return identifiers.compactMap { IntentDirectory.pick(list, id: $0, name: \.name, itemId: \.id) }
    }
    func entities(matching string: String) async throws -> [ModelEntity] {
        await all().filter { IntentDirectory.matches([$0.name, $0.provider], string) }
    }
    func suggestedEntities() async throws -> [ModelEntity] { await all() }
}

// MARK: Project

struct ProjectEntity: AppEntity, Sendable {
    static let typeDisplayRepresentation: TypeDisplayRepresentation = "Project"
    static let defaultQuery = ProjectQuery()

    var id: String
    var title: String

    init(_ info: ProjectInfo) { id = info.id; title = info.title }

    var displayRepresentation: DisplayRepresentation { DisplayRepresentation(title: "\(title)") }
}

struct ProjectQuery: EntityStringQuery {
    init() {}
    private func all() async -> [ProjectEntity] {
        guard let svc = IntentServiceRegistry.current, let list = try? await svc.listProjects() else { return [] }
        return list.map(ProjectEntity.init)
    }
    func entities(for identifiers: [String]) async throws -> [ProjectEntity] {
        let list = await all()
        return identifiers.compactMap { IntentDirectory.pick(list, id: $0, name: \.title, itemId: \.id) }
    }
    func entities(matching string: String) async throws -> [ProjectEntity] {
        await all().filter { IntentDirectory.matches([$0.title], string) }
    }
    func suggestedEntities() async throws -> [ProjectEntity] { await all() }
}
