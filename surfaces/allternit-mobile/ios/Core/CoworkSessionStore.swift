import SwiftUI

/// Cowork session state: the list backing the Cowork workspace launchpad.
///
/// Data source: `GET /api/v1/cowork/sessions` on the gateway API
/// (CoworkSessionsClient). On failure the store keeps whatever it last had and
/// exposes `errorMessage` so views render an error state instead of spinning
/// forever (ProjectStore / CoworkTasksStore convention).
@MainActor
final class CoworkSessionStore: ObservableObject {
    static let shared = CoworkSessionStore()

    @Published private(set) var sessions: [CoworkSession] = []
    @Published private(set) var isLoading = false
    @Published private(set) var errorMessage: String? = nil

    private let client: CoworkSessionsClient
    private var fetchTask: Task<Void, Never>? = nil

    init(client: CoworkSessionsClient = CoworkSessionsClient()) {
        self.client = client
    }

    // MARK: - Fetch

    /// Fetches the session list once per launch unless forced; concurrent
    /// callers share the in-flight request (CronJobStore / CoworkTasksStore
    /// `fetch*IfNeeded` pattern).
    func fetchSessionsIfNeeded(force: Bool = false) {
        guard force || sessions.isEmpty, fetchTask == nil else { return }
        isLoading = true
        errorMessage = nil
        fetchTask = Task { [weak self] in
            guard let self else { return }
            defer {
                self.isLoading = false
                self.fetchTask = nil
            }
            do {
                self.sessions = try await self.client.listSessions()
            } catch is CancellationError {
                // View went away mid-flight — keep current state.
            } catch {
                self.errorMessage = error.localizedDescription
            }
        }
    }

    /// Unconditional refresh (pull-to-refresh, post-mutation resync).
    func refresh() async {
        errorMessage = nil
        do {
            sessions = try await client.listSessions()
        } catch is CancellationError {
            // View went away mid-flight — keep current state.
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    // MARK: - Mutations

    /// Creates a session, appends it locally, and returns the new id.
    @discardableResult
    func createSession(name: String) async throws -> String {
        let id = try await client.createSession(name: name)
        // Optimistically reflect the new session; let refresh fill details.
        let placeholder = CoworkSession(
            id: id,
            userId: "",
            projectId: nil,
            title: name,
            status: "active",
            mode: "regular",
            checkpoint: nil,
            metadata: nil,
            startedAt: nil,
            completedAt: nil,
            createdAt: ISO8601DateFormatter().string(from: Date()),
            updatedAt: ISO8601DateFormatter().string(from: Date())
        )
        sessions.insert(placeholder, at: 0)
        await refresh()
        return id
    }

    /// Deletes a session and removes it locally.
    func deleteSession(id: String) async throws {
        try await client.deleteSession(id: id)
        sessions.removeAll { $0.id == id }
    }
}
