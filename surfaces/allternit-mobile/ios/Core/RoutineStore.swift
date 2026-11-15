import SwiftUI

/// Routines state: the fetched routine list backing the Automation Tasks
/// tab's Routines sub-surface.
///
/// Data source: `GET v1/automations/routines` on gizzi-code's own server
/// (RoutinesClient, same host as PtyClient/PermissionClient/CronClient). On
/// failure the store keeps whatever it last had and exposes `loadError` so
/// views render an error state instead of spinning forever (CronJobStore's
/// convention).
@MainActor
final class RoutineStore: ObservableObject {
    static let shared = RoutineStore()

    @Published private(set) var routines: [Routine] = []
    @Published private(set) var isLoading = false
    @Published private(set) var loadError: String? = nil

    private let client: RoutinesClient
    private var fetchTask: Task<Void, Never>? = nil

    init(client: RoutinesClient = .shared) {
        self.client = client
    }

    func routine(withId id: String) -> Routine? {
        routines.first { $0.id == id }
    }

    // MARK: - Fetch

    /// Fetches the routine list once per launch unless forced; concurrent
    /// callers share the in-flight request (CronJobStore's
    /// `fetchJobsIfNeeded` pattern).
    func fetchRoutinesIfNeeded(force: Bool = false) {
        guard force || routines.isEmpty, fetchTask == nil else { return }
        isLoading = true
        loadError = nil
        fetchTask = Task { [weak self] in
            guard let self else { return }
            defer {
                self.isLoading = false
                self.fetchTask = nil
            }
            do {
                self.routines = try await self.client.listRoutines()
            } catch is CancellationError {
                // View went away mid-flight — keep current state.
            } catch {
                self.loadError = error.localizedDescription
            }
        }
    }

    /// Unconditional refresh (pull-to-refresh, post-mutation resync). There's
    /// no `/routines/:id` endpoint, so this whole-list refetch is also how
    /// the detail view picks up server-driven `state`/step changes after
    /// `run`.
    func refresh() async {
        loadError = nil
        do {
            routines = try await client.listRoutines()
            await SpotlightIndexer.refresh()
        } catch is CancellationError {
            // View went away mid-flight — keep current state.
        } catch {
            loadError = error.localizedDescription
        }
    }

    // MARK: - Mutations

    /// Creates a routine and prepends it locally, then refreshes so any
    /// server-normalized fields land.
    @discardableResult
    func createRoutine(
        name: String,
        steps: [RoutineStep],
        trigger: String?,
        schedule: String?
    ) async throws -> Routine {
        let routine = try await client.createRoutine(name: name, steps: steps, trigger: trigger, schedule: schedule)
        routines.insert(routine, at: 0)
        await refresh()
        return routine
    }

    /// Triggers `RoutineEngine.startRoutine` server-side, then refreshes to
    /// pick up the new `state`/step statuses.
    func runRoutine(id: String) async throws {
        try await client.runRoutine(id: id)
        await refresh()
    }

    /// Deletes a routine and removes it locally.
    func deleteRoutine(id: String) async throws {
        try await client.deleteRoutine(id: id)
        routines.removeAll { $0.id == id }
    }
}
