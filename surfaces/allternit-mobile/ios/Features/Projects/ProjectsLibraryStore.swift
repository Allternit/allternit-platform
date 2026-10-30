import Foundation

/// One project on the Projects screen (the web's UnifiedProject, limited to
/// the kinds iOS has): cowork chat projects (`/api/v1/cowork/projects`) and
/// bot projects (`/api/v1/bot-projects`, cowork rows with a bot team).
struct LibraryProject: Identifiable, Hashable {
    enum Mode: String, CaseIterable {
        case chat, bot
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

    var id: String { "\(mode.rawValue):\(nativeId)" }
}

/// Loads both project kinds and keeps their Pin / Archive flags. Bot
/// projects keep the flags in the server project metadata, as the web's
/// setBotProjectFlags does (read the record, merge the two keys, PATCH the
/// whole blob back). Chat projects keep them on the device, as the web's
/// chat store does in the browser.
@MainActor
final class ProjectsLibraryStore: ObservableObject {
    static let shared = ProjectsLibraryStore()

    @Published private(set) var botProjects: [BotProjectSummary] = []
    @Published private(set) var botProjectsError: String? = nil
    @Published private(set) var threadCounts: [String: Int] = [:]
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

    /// Chats per project, counted client-side from the session list (the
    /// old backend ignores the `project_id` filter).
    private func loadThreadCounts() async {
        guard let envelope: AgentSessionListResponse = try? await APIClient.shared.get(path: "agent-sessions") else { return }
        var counts: [String: Int] = [:]
        for session in envelope.sessions {
            if let projectId = session.projectId { counts[projectId, default: 0] += 1 }
        }
        threadCounts = counts
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
        return chats + bots
    }

    private static func count(_ n: Int, _ word: String) -> String {
        "\(n) \(n == 1 ? word : "\(word)s")"
    }

    // MARK: Flags

    func toggleFavorite(_ project: LibraryProject) {
        switch project.mode {
        case .chat:
            toggle(project.nativeId, in: &localFavorites, key: Self.favoritesKey)
        case .bot:
            Task { await setBotFlags(project.nativeId, favorite: !project.isFavorite, archived: nil) }
        }
    }

    func toggleArchive(_ project: LibraryProject) {
        switch project.mode {
        case .chat:
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

    /// Edit details: title and description in one PATCH (both kinds are
    /// cowork project rows on the server).
    func updateDetails(_ project: LibraryProject, title: String, description: String) async throws {
        let trimmed = description.trimmingCharacters(in: .whitespacesAndNewlines)
        try await APIClient.shared.patch(
            path: "cowork/projects/\(BotModeClient.escape(project.nativeId))",
            body: DetailsBody(title: title, description: trimmed.isEmpty ? nil : description)
        )
        ProjectStore.shared.fetchProjectsIfNeeded(force: true)
        if project.mode == .bot { await loadBotProjects() }
    }

    func delete(_ project: LibraryProject) async throws {
        try await ProjectStore.shared.deleteProject(id: project.nativeId)
        if project.mode == .bot {
            botProjects.removeAll { $0.id == project.nativeId }
            await BotModeStore.shared.refreshProjects()
        }
    }
}
