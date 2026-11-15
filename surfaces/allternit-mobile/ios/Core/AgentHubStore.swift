import SwiftUI

/// Agent Hub state: the full-row agent list, the template catalog, and the
/// hub's mutations (update/delete/instantiate), over `AgentClient`.
///
/// The composer pill reads the SAME registry through `AgentModeStore`'s
/// full-row cache — the hub never maintains a competing selection, and
/// hub views call `AgentModeStore.fetchAgentsIfNeeded(force: true)` after
/// every mutation so the pill and the hub can never disagree (plan item
/// 2.4: a thin hub store reusing the existing fetch cache rather than
/// extending AgentModeStore with detail state it doesn't need).
@MainActor
final class AgentHubStore: ObservableObject {
    static let shared = AgentHubStore()

    @Published private(set) var agents: [AgentRecord] = []
    @Published private(set) var isLoading = false
    @Published private(set) var loadError: String? = nil

    @Published private(set) var templates: [AgentTemplate] = []
    @Published private(set) var isLoadingTemplates = false
    @Published private(set) var templatesError: String? = nil

    private let agentClient: AgentClient
    private var fetchTask: Task<Void, Never>? = nil
    private var templatesTask: Task<Void, Never>? = nil

    init(agentClient: AgentClient = AgentClient()) {
        self.agentClient = agentClient
    }

    func agent(withId id: String) -> AgentRecord? {
        agents.first { $0.id == id }
    }

    // MARK: - Fetch

    /// Fetches the registry once per launch unless forced; concurrent
    /// callers share the in-flight request (same idiom as
    /// AgentModeStore.fetchAgentsIfNeeded).
    func fetchAgentsIfNeeded(force: Bool = false) {
        guard force || agents.isEmpty, fetchTask == nil else { return }
        isLoading = true
        loadError = nil
        fetchTask = Task { [weak self] in
            guard let self else { return }
            defer {
                self.isLoading = false
                self.fetchTask = nil
            }
            do {
                self.agents = try await self.agentClient.listAgents()
                // The bot list changed: refresh Siri/Shortcuts phrases and Spotlight.
                AllternitShortcuts.updateAppShortcutParameters()
                await SpotlightIndexer.refresh()
            } catch is CancellationError {
                // View went away mid-flight — keep current state.
            } catch {
                self.loadError = error.localizedDescription
            }
        }
    }

    /// Fetches the template catalog once per launch unless forced.
    func fetchTemplatesIfNeeded(force: Bool = false) {
        guard force || templates.isEmpty, templatesTask == nil else { return }
        isLoadingTemplates = true
        templatesError = nil
        templatesTask = Task { [weak self] in
            guard let self else { return }
            defer {
                self.isLoadingTemplates = false
                self.templatesTask = nil
            }
            do {
                self.templates = try await self.agentClient.listTemplates()
            } catch is CancellationError {
                // View went away mid-flight — keep current state.
            } catch {
                self.templatesError = error.localizedDescription
            }
        }
    }

    // MARK: - Mutations

    /// Re-reads one agent from the backend and splices it into the cached
    /// list (the PUT returns only `{"success": true}`, so this is how the
    /// detail view reflects its own edits).
    @discardableResult
    func refreshAgent(id: String) async throws -> AgentRecord {
        let fresh = try await agentClient.getAgent(id: id)
        if let index = agents.firstIndex(where: { $0.id == id }) {
            agents[index] = fresh
        } else {
            agents.append(fresh)
        }
        return fresh
    }

    /// PUTs one or more editable fields, then refreshes the cached row.
    func updateAgent(id: String, model: String? = nil, provider: String? = nil,
                     systemPrompt: String? = nil, avatar: String? = nil,
                     config: [String: JSONValue]? = nil) async throws {
        try await agentClient.updateAgent(
            id: id, model: model, provider: provider, systemPrompt: systemPrompt,
            avatar: avatar, config: config
        )
        try await refreshAgent(id: id)
    }

    /// Deletes an agent and removes it from the cached list. Clearing the
    /// composer pill's selection if it pointed at the deleted agent is the
    /// view's job (AgentModeStore owns per-surface selections).
    func deleteAgent(id: String) async throws {
        try await agentClient.deleteAgent(id: id)
        agents.removeAll { $0.id == id }
    }

    /// Instantiates a template and returns the new orchestrator's full row
    /// (spliced into the cached list; the force refetch then picks up the
    /// crew's subagents too).
    @discardableResult
    func createFromTemplate(templateId: String) async throws -> AgentRecord {
        let response = try await agentClient.createFromTemplate(templateId: templateId)
        let fresh = try await refreshAgent(id: response.orchestrator.id)
        fetchAgentsIfNeeded(force: true)
        return fresh
    }
}
