import Foundation

/// A code or design project the server knows: a registered project workspace
/// with the key `code-<id>` or `design-<id>` (coordinator_routes.rs). The web
/// keeps these in the browser; this row is their account-level copy.
struct WorkspaceLibraryProject: Identifiable, Hashable, Sendable {
    /// The status page summary the hub shows (summarizeStatus in useUnifiedProjects).
    struct Status: Hashable, Sendable {
        let state: String
        let nextAction: String
        let closed: Bool
    }

    /// The cowork project row behind the registration.
    let id: String
    /// The web's workspace or design project id; a code session carries it as
    /// `metadata.workspaceId`.
    let nativeId: String
    let title: String
    let description: String?
    let directory: String?
    let createdAt: String?
    let updatedAt: String?
    let status: Status?
}

/// One project on the Projects screen (the web's UnifiedProject, limited to
/// the kinds iOS has): cowork chat projects (`/api/v1/cowork/projects`), bot
/// projects (`/api/v1/bot-projects`, cowork rows with a bot team) and code and
/// design projects (`/api/v1/project-workspaces`).
struct LibraryProject: Identifiable, Hashable {
    enum Mode: String, CaseIterable {
        case chat, code, design, bot
    }

    let mode: Mode
    let nativeId: String
    let title: String
    let description: String?
    let isFavorite: Bool
    let isArchived: Bool
    let updatedAt: Date?
    let createdAt: Date?
    /// "3 threads", "2 sessions" (ProjectCard projectCountLine).
    let countLine: String
    let waiting: Int
    let working: Int
    let cowork: CoworkProject?
    let bot: BotProjectSummary?
    /// Code and design projects.
    var workspace: WorkspaceLibraryProject? = nil

    var id: String { "\(mode.rawValue):\(nativeId)" }
}

/// Loads every project kind and keeps their Pin / Archive flags. Bot
/// projects keep the flags in the server project metadata, as the web's
/// setBotProjectFlags does (read the record, merge the two keys, PATCH the
/// whole blob back). Chat and code projects keep them on the device, as the
/// web's chat and code stores do in the browser.
@MainActor
final class ProjectsLibraryStore: ObservableObject {
    static let shared = ProjectsLibraryStore()

    @Published private(set) var botProjects: [BotProjectSummary] = []
    @Published private(set) var botProjectsError: String? = nil
    @Published private(set) var threadCounts: [String: Int] = [:]
    @Published private(set) var codeProjects: [WorkspaceLibraryProject] = []
    @Published private(set) var designProjects: [WorkspaceLibraryProject] = []
    @Published private(set) var workspaceProjectsError: String? = nil
    /// Open code sessions by workspace id, newest first.
    @Published private(set) var codeSessions: [String: [AgentSession]] = [:]
    @Published private var localFavorites: Set<String>
    @Published private var localArchived: Set<String>
    /// Optimistic bot flags while a write is in flight.
    @Published private var pendingBotFlags: [String: (favorite: Bool?, archived: Bool?)] = [:]

    private static let favoritesKey = "allternit-phone-projects-favorites"
    private static let archivedKey = "allternit-phone-projects-archived"
    private let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        localFavorites = Set(defaults.stringArray(forKey: Self.favoritesKey) ?? [])
        localArchived = Set(defaults.stringArray(forKey: Self.archivedKey) ?? [])
    }

    // MARK: Loading

    func load() async {
        ProjectStore.shared.fetchProjectsIfNeeded()
        await loadBotProjects()
        await loadWorkspaceProjects()
        await loadThreadCounts()
    }

    func loadBotProjects() async {
        do {
            botProjects = try await BotModeClient.shared.listBotProjects()
            botProjectsError = nil
        } catch {
            botProjectsError = "Couldn't load projects. Check the connection and try again."
        }
    }

    private struct WorkspaceList: Decodable {
        struct Row: Decodable {
            let projectId: String
            let key: String?
            let status: JSONValue?
            // Newer runtimes send the row's details here; older ones need
            // the project record.
            let title: String?
            let description: String?
            let nativeId: String?
            let directory: String?
            let createdAt: String?
            let updatedAt: String?
        }
        let workspaces: [Row]
    }

    private struct ProjectRecordEnvelope: Decodable {
        struct Record: Decodable {
            let title: String
            let description: String?
            let metadata: String?
            let createdAt: String?
            let updatedAt: String?

            enum CodingKeys: String, CodingKey {
                case title, description, metadata
                case createdAt = "created_at"
                case updatedAt = "updated_at"
            }
        }
        let project: Record
    }

    private static func status(from value: JSONValue?) -> WorkspaceLibraryProject.Status? {
        guard case .object(let page)? = value else { return nil }
        func text(_ key: String) -> String? {
            if case .string(let value)? = page[key] { return value }
            return nil
        }
        return WorkspaceLibraryProject.Status(
            state: text("state") ?? "on_track",
            nextAction: (text("next_action") ?? "").trimmingCharacters(in: .whitespacesAndNewlines),
            closed: text("lifecycle") == "closed"
        )
    }

    /// One call on a current runtime. Against an older one each row's details
    /// come from its project record; a row that fails keeps what the screen had.
    func loadWorkspaceProjects() async {
        // The sample workspace has none of these.
        guard !SampleWorkspace.isEnabled else { return }
        do {
            let list: WorkspaceList = try await APIClient.shared.get(path: "project-workspaces")
            var code: [WorkspaceLibraryProject] = []
            var design: [WorkspaceLibraryProject] = []
            for row in list.workspaces {
                guard let key = row.key else { continue }
                let prefix: String
                if key.hasPrefix("code-") { prefix = "code-" } else if key.hasPrefix("design-") { prefix = "design-" } else { continue }
                let project: WorkspaceLibraryProject?
                if let title = row.title {
                    project = WorkspaceLibraryProject(
                        id: row.projectId,
                        nativeId: row.nativeId ?? String(key.dropFirst(prefix.count)),
                        title: title,
                        description: row.description,
                        directory: row.directory,
                        createdAt: row.createdAt,
                        updatedAt: row.updatedAt,
                        status: Self.status(from: row.status)
                    )
                } else {
                    project = await legacyWorkspaceProject(row, key: key, prefix: prefix)
                }
                guard let project else { continue }
                if prefix == "code-" { code.append(project) } else { design.append(project) }
            }
            // A view that went away mid-loop must not publish a partial list.
            guard !Task.isCancelled else { return }
            codeProjects = code
            designProjects = design
            workspaceProjectsError = nil
        } catch is CancellationError {
            return
        } catch {
            if (error as? APIError)?.httpStatus == 404 {
                // A runtime older than project workspaces has none.
                codeProjects = []
                designProjects = []
                workspaceProjectsError = nil
            } else {
                workspaceProjectsError = "Couldn't load code and design projects. Check the connection and try again."
            }
        }
    }

    private func legacyWorkspaceProject(_ row: WorkspaceList.Row, key: String, prefix: String) async -> WorkspaceLibraryProject? {
        let path = "cowork/projects/\(BotModeClient.escape(row.projectId))"
        guard let envelope: ProjectRecordEnvelope = try? await APIClient.shared.get(path: path) else {
            return (codeProjects + designProjects).first { $0.id == row.projectId }
        }
        let record = envelope.project
        var workspace: [String: JSONValue] = [:]
        if let raw = record.metadata?.data(using: .utf8),
           let parsed = try? JSONDecoder().decode([String: JSONValue].self, from: raw),
           case .object(let value)? = parsed["workspace"] {
            workspace = value
        }
        var nativeId = String(key.dropFirst(prefix.count))
        if case .string(let value)? = workspace["nativeId"], !value.isEmpty { nativeId = value }
        var directory: String? = nil
        if case .string(let value)? = workspace["directory"], !value.isEmpty { directory = value }
        return WorkspaceLibraryProject(
            id: row.projectId,
            nativeId: nativeId,
            title: record.title,
            description: record.description,
            directory: directory,
            createdAt: record.createdAt,
            updatedAt: record.updatedAt,
            status: Self.status(from: row.status)
        )
    }

    /// New design project (createProjectWithDetails, mode design): the web's id
    /// shape. Its canvas record is found or made when the canvas opens.
    func createDesignProject(title: String, description: String?) async throws -> WorkspaceLibraryProject {
        let random = String(UInt64.random(in: 0..<2_176_782_336), radix: 36)
        let nativeId = "design-\(Int(Date().timeIntervalSince1970 * 1000))-\(random)"
        let rowId = try await BotProjectClient.shared.registerWorkspace(
            key: "design-\(nativeId)", nativeId: nativeId, mode: "design", title: title, description: description
        )
        let now = ISO8601DateFormatter().string(from: Date())
        let project = WorkspaceLibraryProject(
            id: rowId, nativeId: nativeId, title: title, description: description,
            directory: nil, createdAt: now, updatedAt: now, status: nil
        )
        designProjects.insert(project, at: 0)
        return project
    }

    private struct WorkspaceSettingsEnvelope: Decodable {
        let settings: [String: JSONValue]
    }

    private struct WorkspaceSaveBody: Encodable {
        let nativeId: String
        let mode: String
        let title: String
        let description: String?
        let settings: [String: JSONValue]
    }

    /// The folder a code project works in: `settings.directory`, as the web
    /// registers it. A register replaces the settings whole, so this keeps the rest.
    func setDirectory(_ path: String, for project: WorkspaceLibraryProject) async throws {
        let key = "code-\(project.nativeId)"
        let route = "project-workspaces/\(BotModeClient.escape(key))"
        let current: WorkspaceSettingsEnvelope = try await APIClient.shared.get(path: route)
        var settings = current.settings
        settings["directory"] = .string(path)
        var request = try await APIClient.shared.authorizedRequest(path: route, method: "PUT")
        request.httpBody = try JSONEncoder().encode(WorkspaceSaveBody(
            nativeId: project.nativeId, mode: "code", title: project.title,
            description: project.description, settings: settings
        ))
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        let (data, response) = try await APIClient.shared.send(request)
        try APIClient.shared.validate(response, data: data)
        if let index = codeProjects.firstIndex(where: { $0.id == project.id }) {
            codeProjects[index] = WorkspaceLibraryProject(
                id: project.id, nativeId: project.nativeId, title: project.title, description: project.description,
                directory: path, createdAt: project.createdAt, updatedAt: project.updatedAt, status: project.status
            )
        }
    }

    /// New code project (createProjectWithDetails, mode code): the web's id
    /// shape, registered so Desktop and the web pick it up from the same list.
    func createCodeProject(title: String, description: String?) async throws -> WorkspaceLibraryProject {
        let nativeId = "ws_" + String(Int(Date().timeIntervalSince1970 * 1000), radix: 36)
        let rowId = try await BotProjectClient.shared.registerWorkspace(
            key: "code-\(nativeId)", nativeId: nativeId, mode: "code", title: title, description: description
        )
        let now = ISO8601DateFormatter().string(from: Date())
        let project = WorkspaceLibraryProject(
            id: rowId, nativeId: nativeId, title: title, description: description,
            directory: nil, createdAt: now, updatedAt: now, status: nil
        )
        codeProjects.insert(project, at: 0)
        return project
    }

    /// One session with the links `AgentSession` doesn't decode.
    private struct LinkedSession: Decodable {
        let session: AgentSession
        let workspaceId: String?
        let projectRole: String?

        private enum Keys: String, CodingKey { case metadata }
        private enum MetadataKeys: String, CodingKey { case workspaceId, projectRole }

        init(from decoder: Decoder) throws {
            session = try AgentSession(from: decoder)
            let container = try decoder.container(keyedBy: Keys.self)
            let metadata = try? container.nestedContainer(keyedBy: MetadataKeys.self, forKey: .metadata)
            workspaceId = try? metadata?.decodeIfPresent(String.self, forKey: .workspaceId)
            projectRole = try? metadata?.decodeIfPresent(String.self, forKey: .projectRole)
        }
    }

    private struct LinkedSessionList: Decodable {
        let sessions: [LinkedSession]
    }

    /// Chats per project and code sessions per workspace, counted client-side
    /// from the session list (the old backend ignores the `project_id`
    /// filter). A code project's threads are its `workspaceId` sessions
    /// (isProjectThread), not the coordinator's own.
    func loadThreadCounts() async {
        guard let envelope: LinkedSessionList = try? await APIClient.shared.get(path: "agent-sessions") else { return }
        var counts: [String: Int] = [:]
        var code: [String: [AgentSession]] = [:]
        for linked in envelope.sessions {
            if let projectId = linked.session.projectId { counts[projectId, default: 0] += 1 }
            if let workspaceId = linked.workspaceId, linked.projectRole != "coordinator",
               linked.session.active, !linked.session.archived {
                code[workspaceId, default: []].append(linked.session)
            }
        }
        threadCounts = counts
        codeSessions = code.mapValues { list in
            list.sorted { (libraryParseDate($0.updatedAt) ?? .distantPast) > (libraryParseDate($1.updatedAt) ?? .distantPast) }
        }
    }

    // MARK: Unified list

    func projects(from cowork: [CoworkProject]) -> [LibraryProject] {
        let botIds = Set(botProjects.map(\.id))
        let chats = cowork.filter { !botIds.contains($0.id) }.map { project -> LibraryProject in
            let threads = threadCounts[project.id] ?? 0
            return LibraryProject(
                mode: .chat,
                nativeId: project.id,
                title: project.title,
                description: project.description,
                isFavorite: localFavorites.contains(project.id),
                isArchived: localArchived.contains(project.id),
                updatedAt: libraryParseDate(project.updatedAt),
                createdAt: libraryParseDate(project.createdAt),
                countLine: Self.count(threads, "thread"),
                waiting: 0,
                working: 0,
                cowork: project,
                bot: nil
            )
        }
        let bots = botProjects.map { project -> LibraryProject in
            let pending = pendingBotFlags[project.id]
            let sessions = project.counts.waiting + project.counts.working + project.counts.queued
                + project.counts.idle + project.counts.resolved
            let created = cowork.first { $0.id == project.id }.flatMap { libraryParseDate($0.createdAt) }
            return LibraryProject(
                mode: .bot,
                nativeId: project.id,
                title: project.title,
                description: project.description,
                isFavorite: pending?.favorite ?? project.favorite,
                isArchived: pending?.archived ?? project.archived,
                updatedAt: libraryParseDate(project.lastActivityAt),
                createdAt: created,
                countLine: "\(Self.count(project.botIds.count, "bot")) · Al coordinates",
                waiting: project.counts.waiting,
                working: project.counts.working,
                cowork: nil,
                bot: project
            )
        }
        let code = codeProjects.map { project -> LibraryProject in
            let sessions = codeSessions[project.nativeId] ?? []
            return workspaceRow(project, mode: .code,
                                lastActivity: sessions.first.flatMap { libraryParseDate($0.updatedAt) },
                                countLine: Self.count(sessions.count, "session"))
        }
        let design = designProjects.map { project -> LibraryProject in
            workspaceRow(project, mode: .design, lastActivity: nil,
                         countLine: Self.count(threadCounts[project.nativeId] ?? 0, "thread"))
        }
        return chats + code + design + bots
    }

    private func workspaceRow(_ project: WorkspaceLibraryProject, mode: LibraryProject.Mode, lastActivity: Date?, countLine: String) -> LibraryProject {
        LibraryProject(
            mode: mode,
            nativeId: project.nativeId,
            title: project.title,
            // ProjectCard: the description, else the folder.
            description: project.description?.isEmpty == false ? project.description : project.directory,
            isFavorite: localFavorites.contains(project.nativeId),
            isArchived: localArchived.contains(project.nativeId),
            updatedAt: [lastActivity, libraryParseDate(project.updatedAt)].compactMap { $0 }.max(),
            createdAt: libraryParseDate(project.createdAt),
            countLine: countLine,
            waiting: 0,
            working: 0,
            cowork: nil,
            bot: nil,
            workspace: project
        )
    }

    private static func count(_ n: Int, _ word: String) -> String {
        "\(n) \(n == 1 ? word : "\(word)s")"
    }

    // MARK: Flags

    func toggleFavorite(_ project: LibraryProject) {
        switch project.mode {
        case .chat, .code, .design:
            toggle(project.nativeId, in: &localFavorites, key: Self.favoritesKey)
        case .bot:
            Task { await setBotFlags(project.nativeId, favorite: !project.isFavorite, archived: nil) }
        }
    }

    func toggleArchive(_ project: LibraryProject) {
        switch project.mode {
        case .chat, .code, .design:
            toggle(project.nativeId, in: &localArchived, key: Self.archivedKey)
        case .bot:
            Task { await setBotFlags(project.nativeId, favorite: nil, archived: !project.isArchived) }
        }
    }

    private func toggle(_ id: String, in set: inout Set<String>, key: String) {
        if set.contains(id) { set.remove(id) } else { set.insert(id) }
        defaults.set(Array(set), forKey: key)
    }

    private struct ProjectEnvelope: Decodable {
        struct Record: Decodable { let metadata: String? }
        let project: Record
    }

    private struct MetadataBody: Encodable { let metadata: String }

    /// Read-merge-write of the project metadata JSON so migrated files,
    /// folder and guide version survive.
    private func setBotFlags(_ id: String, favorite: Bool?, archived: Bool?) async {
        pendingBotFlags[id] = (favorite, archived)
        defer { pendingBotFlags[id] = nil }
        let path = "cowork/projects/\(BotModeClient.escape(id))"
        do {
            let current: ProjectEnvelope = try await APIClient.shared.get(path: path)
            var metadata: [String: JSONValue] = [:]
            if let raw = current.project.metadata?.data(using: .utf8),
               let parsed = try? JSONDecoder().decode([String: JSONValue].self, from: raw) {
                metadata = parsed
            }
            if let favorite { metadata["favorite"] = .bool(favorite) }
            if let archived { metadata["archived"] = .bool(archived) }
            let encoded = String(decoding: try JSONEncoder().encode(metadata), as: UTF8.self)
            try await APIClient.shared.patch(path: path, body: MetadataBody(metadata: encoded))
            await loadBotProjects()
        } catch {
            botProjectsError = "Couldn't update \(botProjects.first { $0.id == id }?.title ?? "the project"). \(FriendlyErrorMessage.from(error.localizedDescription))"
        }
    }

    // MARK: Edit and delete

    private struct DetailsBody: Encodable {
        let title: String
        let description: String?

        func encode(to encoder: Encoder) throws {
            var c = encoder.container(keyedBy: CodingKeys.self)
            try c.encode(title, forKey: .title)
            // null clears the description, as the web's Edit details does.
            try c.encode(description, forKey: .description)
        }

        enum CodingKeys: String, CodingKey { case title, description }
    }

    /// Edit details: title and description in one PATCH (every kind is a
    /// cowork project row on the server; a code project's row has its own id).
    func updateDetails(_ project: LibraryProject, title: String, description: String) async throws {
        let trimmed = description.trimmingCharacters(in: .whitespacesAndNewlines)
        try await APIClient.shared.patch(
            path: "cowork/projects/\(BotModeClient.escape(project.workspace?.id ?? project.nativeId))",
            body: DetailsBody(title: title, description: trimmed.isEmpty ? nil : description)
        )
        ProjectStore.shared.fetchProjectsIfNeeded(force: true)
        if project.mode == .bot { await loadBotProjects() }
        if project.workspace != nil { await loadWorkspaceProjects() }
    }

    func delete(_ project: LibraryProject) async throws {
        try await ProjectStore.shared.deleteProject(id: project.workspace?.id ?? project.nativeId)
        if let row = project.workspace {
            codeProjects.removeAll { $0.id == row.id }
            designProjects.removeAll { $0.id == row.id }
        }
        if project.mode == .bot {
            botProjects.removeAll { $0.id == project.nativeId }
            await BotModeStore.shared.refreshProjects()
        }
    }
}
