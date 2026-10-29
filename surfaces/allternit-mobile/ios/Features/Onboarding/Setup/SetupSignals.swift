import Foundation

/// Live signals behind the setup tiles (allternit-ai `setup/setupSignals.ts`).
/// Each tag on a tile comes from one of these: computers from the account's
/// runtime-device list, cloud computer specs only from the account's real
/// instance, brain rows from the runtime's provider auth status and the live
/// Allternit Cloud catalog, subscription rows from the Sessions computer.
@MainActor
final class SetupSignals: ObservableObject {
    // Paired computers
    @Published private(set) var runtimes: [RelayRuntimeDevice] = []
    @Published private(set) var runtimesSettled = false

    // Allternit Cloud computer
    struct CloudSignal: Equatable {
        var loading = true
        var signedIn = false
        var computer: SetupCloudComputer?
        var plan: SetupPlan?
        var error: String?
    }
    @Published private(set) var cloud = CloudSignal()

    // Brains
    @Published private(set) var brainsLoading = true
    @Published private(set) var authed: Set<String> = []
    @Published private(set) var catalog: [SetupModelRow] = []

    // Subscription brains (Subscription Fabric on the Sessions computer)
    @Published private(set) var subsLoading = true
    @Published private(set) var subsStatus: SetupSubscriptionStatus?
    @Published private(set) var subsAccounts: [SetupFabricAccount] = []
    @Published private(set) var subsError: String?

    private var started = false

    var onlineRuntimes: [RelayRuntimeDevice] { runtimes.filter(\.isOnline) }

    func start() {
        guard !started else { return }
        started = true
        Task { await loadRuntimes() }
        Task { await loadCloud() }
        Task { await loadBrains() }
        Task { await loadSubscriptions() }
    }

    /// The runtime list, plus a deadline: a list that never answers (401/403,
    /// offline) counts as settled after 6 s instead of holding setup.
    func loadRuntimes() async {
        let relay = RuntimeRelayStore.shared
        let deadline = Task { @MainActor [weak self] in
            try? await Task.sleep(nanoseconds: 6_000_000_000)
            self?.runtimesSettled = true
        }
        await relay.refresh()
        runtimes = relay.devices
        runtimesSettled = true
        deadline.cancel()
    }

    func loadCloud() async {
        guard AuthManager.shared.isSignedIn else {
            cloud = CloudSignal(loading: false, signedIn: false)
            return
        }
        async let computers = setupAttempt { try await SetupAPI.cloudComputers() }
        async let plan = setupAttempt { try await SetupAPI.hostedPlan() }
        let (c, p) = await (computers, plan)
        var sig = CloudSignal(loading: false, signedIn: true)
        switch c {
        case .success(let list): sig.computer = list.first
        case .failure(let e): sig.error = e.localizedDescription
        }
        if case .success(let value) = p { sig.plan = value }
        cloud = sig
    }

    func loadBrains() async {
        async let auth = setupAttempt { try await SetupAPI.providerAuthStatus() }
        async let models = SetupAPI.cloudModelCatalog()
        let (a, m) = await (auth, models)
        if case .success(let rows) = a { authed = Set(rows.filter { $0.authenticated }.map { $0.id }) }
        catalog = m
        brainsLoading = false
    }

    func loadSubscriptions() async {
        async let status = setupAttempt { try await SetupAPI.subscriptionStatus() }
        async let accounts = setupAttempt { try await SetupAPI.fabricAccounts() }
        let (s, a) = await (status, accounts)
        switch s {
        case .success(let value): subsStatus = value; subsError = nil
        case .failure(let e): subsStatus = nil; subsError = Self.describeSubsError(e)
        }
        if case .success(let list) = a { subsAccounts = list }
        subsLoading = false
    }

    /// A login created or changed by the inline connect flow, ahead of the next list refresh.
    func upsert(_ account: SetupFabricAccount) {
        subsAccounts.removeAll { $0.accountId == account.accountId }
        subsAccounts.append(account)
    }

    enum SubscriptionTileState: Equatable { case checking, ready, signin, noComputer, offline, connect, error }

    /// Tile state for one subscription provider (chatgpt | claude | kimi).
    func subscriptionState(_ provider: String) -> SubscriptionTileState {
        if subsLoading { return .checking }
        let logins = subsAccounts.filter { $0.provider == provider }
        if logins.contains(where: \.isReady) { return .ready }
        let bound = subsStatus?.bound ?? false
        if subsError != nil && !bound { return .error }
        if !bound { return .noComputer }
        if !(subsStatus?.running ?? false) { return .offline }
        if !logins.isEmpty { return .signin }
        return .connect
    }

    func terms(for provider: String) -> SetupSubscriptionStatus.Terms? {
        subsStatus?.terms.first { $0.provider == provider }
    }

    /// Plain words for a Sessions computer error (web describeSubsError, the common cases).
    static func describeSubsError(_ error: Error) -> String {
        if let e = error as? ConnectError {
            switch e.status {
            case 401, 403: return "Sign in again to check your subscriptions"
            case 404, 503: return "Your Sessions computer can’t be reached right now"
            case nil where e.code == "timeout": return "Your Sessions computer took too long to answer"
            default: return e.message
            }
        }
        return error.localizedDescription
    }
}

/// Runs one source, keeping its failure as a value so one failing source never blanks the others.
func setupAttempt<T: Sendable>(_ body: @Sendable () async throws -> T) async -> Result<T, Error> {
    do { return .success(try await body()) } catch { return .failure(error) }
}
