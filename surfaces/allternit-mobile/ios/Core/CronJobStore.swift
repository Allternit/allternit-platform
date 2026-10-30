import SwiftUI

/// Cron jobs state: the fetched job list backing the Automation Tasks tab.
///
/// Data source: `GET v1/cron/jobs` on gizzi-code's own server (CronClient,
/// same host as PtyClient/PermissionClient). On failure the store keeps
/// whatever it last had and exposes `loadError` so views render an error
/// state instead of spinning forever (ProjectStore's convention).
@MainActor
final class CronJobStore: ObservableObject {
    static let shared = CronJobStore()

    @Published private(set) var jobs: [CronJob] = []
    @Published private(set) var isLoading = false
    @Published private(set) var loadError: String? = nil

    private let client: CronClient
    private var fetchTask: Task<Void, Never>? = nil

    init(client: CronClient = .shared) {
        self.client = client
    }

    func job(withId id: String) -> CronJob? {
        jobs.first { $0.id == id }
    }

    // MARK: - Fetch

    /// Fetches the job list once per launch unless forced; concurrent
    /// callers share the in-flight request (ProjectStore's
    /// `fetchProjectsIfNeeded` pattern).
    func fetchJobsIfNeeded(force: Bool = false) {
        guard force || jobs.isEmpty, fetchTask == nil else { return }
        isLoading = true
        loadError = nil
        fetchTask = Task { [weak self] in
            guard let self else { return }
            defer {
                self.isLoading = false
                self.fetchTask = nil
            }
            do {
                self.jobs = try await self.client.listJobs()
            } catch is CancellationError {
                // View went away mid-flight — keep current state.
            } catch {
                self.loadError = error.localizedDescription
            }
        }
    }

    /// Unconditional refresh (pull-to-refresh, post-mutation resync).
    func refresh() async {
        loadError = nil
        do {
            jobs = try await client.listJobs()
        } catch is CancellationError {
            // View went away mid-flight — keep current state.
        } catch {
            loadError = error.localizedDescription
        }
    }

    // MARK: - Mutations

    /// Creates an agent job and prepends it locally, then refreshes so
    /// server-computed fields (`nextRunAt`, normalized schedule) land.
    @discardableResult
    func createAgentJob(name: String, prompt: String, schedule: String) async throws -> CronJob {
        let job = try await client.createAgentJob(name: name, prompt: prompt, schedule: schedule)
        jobs.insert(job, at: 0)
        await refresh()
        return job
    }

    /// Pauses a job: optimistic local flip to "paused", then resynced from
    /// the server (the source of truth for `status`/`nextRunAt`).
    func pause(id: String) async throws {
        try await client.pause(id: id)
        await refresh()
    }

    /// Resumes a paused job.
    func resume(id: String) async throws {
        try await client.resume(id: id)
        await refresh()
    }

    /// Manually triggers a run. Doesn't mutate `jobs` itself — callers
    /// refresh the job's run history separately.
    func runNow(id: String) async throws {
        try await client.runNow(id: id)
        await refresh()
    }

    /// Saves a new schedule (drag on the calendar), then resyncs `nextRunAt`.
    func updateSchedule(id: String, schedule: String) async throws {
        try await client.updateSchedule(id: id, schedule: schedule)
        await refresh()
    }

    /// Deletes a job and removes it locally.
    func deleteJob(id: String) async throws {
        try await client.deleteJob(id: id)
        jobs.removeAll { $0.id == id }
    }

    /// Run history for a job's detail view — not cached on the store since
    /// only one detail view reads it at a time.
    func listRuns(jobId: String) async throws -> [CronRun] {
        try await client.listRuns(jobId: jobId)
    }
}
