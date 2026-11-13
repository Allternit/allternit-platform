import Foundation

/// The self-hosted server profile: address in UserDefaults, API key in the
/// Keychain (`KeychainHelper`). Owns the connect/test flow and the status
/// shown on Settings → Self-hosted server.
@MainActor
final class SelfHostedServerStore: ObservableObject {
    static let shared = SelfHostedServerStore()

    enum Phase: Equatable {
        case notConfigured
        case connecting
        case connected
        case failed(SelfHostedError)
    }

    private static let urlKey = "selfhosted.serverURL"
    private static let keyAccount = "selfhosted.apiKey"

    @Published private(set) var phase: Phase = .notConfigured
    @Published private(set) var info: SHServerInfo?
    @Published private(set) var agents: [SHAgent] = []
    @Published private(set) var environments: [SHEnvironment] = []
    @Published private(set) var workers: [String: [SHWorker]] = [:]

    private(set) var serverURL: String
    private(set) var apiKey: String

    init() {
        serverURL = UserDefaults.standard.string(forKey: Self.urlKey) ?? ""
        apiKey = KeychainHelper.load(Self.keyAccount) ?? ""
        phase = isConfigured ? .connecting : .notConfigured
    }

    var isConfigured: Bool { !serverURL.isEmpty && !apiKey.isEmpty }
    var workersOnline: Int { environments.reduce(0) { $0 + ($1.workerCount ?? 0) } }
    var queueDepth: Int { environments.reduce(0) { $0 + ($1.queueDepth ?? 0) } }

    /// A client for the saved profile, or nil when none is saved.
    func client() -> SelfHostedClient? {
        guard let url = try? SelfHostedClient.normalize(serverURL), !apiKey.isEmpty else { return nil }
        return SelfHostedClient(baseURL: url, apiKey: apiKey)
    }

    /// Validate, test, and (only on success) save the profile.
    func connect(urlText: String, key: String) async {
        let trimmedKey = key.trimmingCharacters(in: .whitespacesAndNewlines)
        phase = .connecting
        let url: URL
        do { url = try SelfHostedClient.normalize(urlText) }
        catch { phase = .failed(error as? SelfHostedError ?? .invalidURL); return }
        guard !trimmedKey.isEmpty else { phase = .failed(.unauthorized); return }
        let candidate = SelfHostedClient(baseURL: url, apiKey: trimmedKey)
        if await refresh(using: candidate) {
            serverURL = url.absoluteString
            apiKey = trimmedKey
            UserDefaults.standard.set(serverURL, forKey: Self.urlKey)
            KeychainHelper.save(trimmedKey, for: Self.keyAccount)
        }
    }

    /// Re-test the saved profile.
    func reload() async {
        guard let client = client() else { phase = .notConfigured; return }
        phase = .connecting
        _ = await refresh(using: client)
    }

    func disconnect() {
        UserDefaults.standard.removeObject(forKey: Self.urlKey)
        KeychainHelper.delete(Self.keyAccount)
        serverURL = ""; apiKey = ""
        info = nil; agents = []; environments = []; workers = [:]
        phase = .notConfigured
    }

    private func refresh(using client: SelfHostedClient) async -> Bool {
        do {
            let info = try await client.serverInfo()
            // Server info needs no key, so the agents call is the key check.
            async let agents = client.listAgents()
            async let envs = client.listEnvironments()
            let (a, e) = try await (agents, envs)
            self.info = info; self.agents = a; self.environments = e
            var byEnv: [String: [SHWorker]] = [:]
            for env in e { byEnv[env.id] = (try? await client.listWorkers(environmentID: env.id)) ?? [] }
            workers = byEnv
            phase = .connected
            return true
        } catch {
            phase = .failed(error as? SelfHostedError ?? .unreachable(error.localizedDescription))
            return false
        }
    }
}
