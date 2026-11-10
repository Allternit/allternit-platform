import SwiftUI

/// Loops state: the fetched loop list backing the Automation Tasks tab's
/// Loops sub-surface.
///
/// Data source: `GET v1/automations/loops` on gizzi-code's own server
/// (LoopsClient, same host as PtyClient/PermissionClient/CronClient/
/// RoutinesClient). On failure the store keeps whatever it last had and
/// exposes `loadError` so views render an error state instead of spinning
/// forever (RoutineStore's convention).
@MainActor
final class LoopStore: ObservableObject {
    static let shared = LoopStore()

    @Published private(set) var loops: [Loop] = []
    @Published private(set) var isLoading = false
    @Published private(set) var loadError: String? = nil

    private let client: LoopsClient
    private var fetchTask: Task<Void, Never>? = nil

    init(client: LoopsClient = .shared) {
        self.client = client
    }

    func loop(withId id: String) -> Loop? {
        loops.first { $0.id == id }
    }

    // MARK: - Fetch

    /// Fetches the loop list once per launch unless forced; concurrent
    /// callers share the in-flight request (RoutineStore's
    /// `fetchRoutinesIfNeeded` pattern).
    func fetchLoopsIfNeeded(force: Bool = false) {
        guard force || loops.isEmpty, fetchTask == nil else { return }
        isLoading = true
        loadError = nil
        fetchTask = Task { [weak self] in
            guard let self else { return }
            defer {
                self.isLoading = false
                self.fetchTask = nil
            }
            do {
                self.loops = try await self.client.listLoops()
                LoopLiveActivityManager.shared.sync(with: self.loops)
            } catch is CancellationError {
                // View went away mid-flight — keep current state.
            } catch {
                self.loadError = error.localizedDescription
            }
        }
    }

    /// Unconditional refresh (pull-to-refresh, post-mutation resync). There's
    /// no `/loops/:id` endpoint, so this whole-list refetch is also how the
    /// detail view picks up server-driven `state`/`iteration_log` changes
    /// while a loop is running.
    func refresh() async {
        loadError = nil
        do {
            loops = try await client.listLoops()
            LoopLiveActivityManager.shared.sync(with: loops)
        } catch is CancellationError {
            // View went away mid-flight — keep current state.
        } catch {
            loadError = error.localizedDescription
        }
    }

    // MARK: - Mutations

    /// Creates a loop and prepends it locally, then refreshes so any
    /// server-normalized fields land. The server starts the loop the moment
    /// it's created (unlike Routines), so the returned/refreshed row is
    /// already `state: "running"`.
    @discardableResult
    func createLoop(
        command: String,
        exitCondition: String?,
        maxIterations: Int,
        metric: LoopMetric? = nil
    ) async throws -> Loop {
        let loop = try await client.createLoop(command: command, exitCondition: exitCondition, maxIterations: maxIterations, metric: metric)
        loops.insert(loop, at: 0)
        await refresh()
        return loop
    }

    /// Restarts a stopped/finished loop via `LoopEngine.startLoop`, then
    /// refreshes to pick up the new `state`/`iteration_log`.
    func runLoop(id: String) async throws {
        try await client.runLoop(id: id)
        await refresh()
    }

    /// Deletes a loop and removes it locally.
    func deleteLoop(id: String) async throws {
        try await client.deleteLoop(id: id)
        loops.removeAll { $0.id == id }
        LoopLiveActivityManager.shared.sync(with: loops)
    }
}
