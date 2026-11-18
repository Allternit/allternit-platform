import SwiftUI

/// Enterprise BYOC state: profile, cloud credentials, and metered usage.
///
/// Data sources: `/api/v1/me`, `/api/v1/usage/summary`, `/api/v1/cloud-credentials`
/// on the gateway (BYOCClient).
@MainActor
final class BYOCStore: ObservableObject {
    static let shared = BYOCStore()

    @Published private(set) var profile: CurrentUserProfile? = nil
    @Published private(set) var credentials: [CloudCredential] = []
    @Published private(set) var usageSummary: EnterpriseUsageSummary? = nil
    @Published private(set) var isLoadingProfile = false
    @Published private(set) var isLoadingCredentials = false
    @Published private(set) var isLoadingUsage = false
    @Published private(set) var loadError: String? = nil
    @Published private(set) var isCreating = false
    @Published private(set) var createError: String? = nil
    @Published private(set) var isRevokingId: String? = nil

    /// Views report/update create-flow errors through this setter; the
    /// property stays `private(set)` so all mutation goes through the store.
    func setCreateError(_ message: String?) {
        createError = message
    }
    @Published private(set) var isTesting = false
    @Published private(set) var testResult: CloudCredentialTestResult? = nil

    private let client: BYOCClient
    private var fetchTask: Task<Void, Never>? = nil

    init(client: BYOCClient = BYOCClient()) {
        self.client = client
    }

    // MARK: - Fetch

    func fetchIfNeeded(force: Bool = false) {
        guard force || profile == nil, fetchTask == nil else { return }
        fetchTask = Task { [weak self] in
            guard let self else { return }
            defer { self.fetchTask = nil }
            do {
                self.isLoadingProfile = true
                self.profile = try await self.client.fetchCurrentUserProfile()
                self.isLoadingProfile = false
                await self.refreshCredentials()
            } catch is CancellationError {
                // View went away mid-flight — keep current state.
            } catch {
                self.loadError = error.localizedDescription
            }
            self.isLoadingProfile = false
        }
    }

    func refreshCredentials() async {
        isLoadingCredentials = true
        defer { isLoadingCredentials = false }
        do {
            self.credentials = try await client.listCloudCredentials()
        } catch {
            loadError = error.localizedDescription
        }
    }

    func refreshUsage() async {
        guard let orgId = profile?.organizationId else { return }
        isLoadingUsage = true
        defer { isLoadingUsage = false }
        let (start, end) = Self.currentBillingPeriod()
        do {
            self.usageSummary = try await client.fetchUsageSummary(
                organizationId: orgId,
                periodStart: start,
                periodEnd: end
            )
        } catch {
            loadError = error.localizedDescription
        }
    }

    // MARK: - Actions

    func createCredential(_ request: CloudCredentialCreateRequest) async throws {
        isCreating = true
        createError = nil
        defer { isCreating = false }
        let credential = try await client.createCloudCredential(request)
        credentials.append(credential)
    }

    func revokeCredential(_ id: String) async throws {
        isRevokingId = id
        defer { isRevokingId = nil }
        try await client.revokeCloudCredential(id: id)
        credentials.removeAll { $0.id == id }
    }

    func testCredential(_ request: CloudCredentialTestRequest) async throws -> CloudCredentialTestResult {
        isTesting = true
        defer { isTesting = false }
        let result = try await client.testCloudCredential(request)
        testResult = result
        return result
    }

    func clearTestResult() {
        testResult = nil
    }

    func clearErrors() {
        loadError = nil
        createError = nil
    }

    // MARK: - Account switch

    /// Account switch (phone calm I14b, org and person): the leaving
    /// account's profile, cloud credentials and usage go. The next screen
    /// that needs them fetches with the new session (`fetchIfNeeded`).
    func reset() {
        fetchTask?.cancel()
        fetchTask = nil
        profile = nil
        credentials = []
        usageSummary = nil
        isLoadingProfile = false
        isLoadingCredentials = false
        isLoadingUsage = false
        loadError = nil
        isCreating = false
        createError = nil
        isRevokingId = nil
        isTesting = false
        testResult = nil
    }

    // MARK: - Helpers

    var canManageBilling: Bool {
        guard let role = profile?.organizationRole ?? profile?.role else { return false }
        return ["owner", "admin", "admin:billing"].contains(role.lowercased())
    }

    var isRuntimeAvailable: Bool {
        // Best-effort signal: at least one registered gizzi instance.
        !InstanceStore.shared.instances.isEmpty
    }

    var activeCredentialCount: Int {
        credentials.filter { $0.status == "active" }.count
    }

    private static func currentBillingPeriod() -> (start: String, end: String) {
        let now = Date()
        var start = DateComponents()
        start.year = Calendar.current.component(.year, from: now)
        start.month = Calendar.current.component(.month, from: now)
        start.day = 1
        start.timeZone = TimeZone(identifier: "UTC")
        let startDate = Calendar.current.date(from: start)!
        var end = start
        end.month! += 1
        let endDate = Calendar.current.date(from: end)!
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime]
        return (formatter.string(from: startDate), formatter.string(from: endDate))
    }
}
