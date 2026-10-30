import Foundation

/// Shared "resolve a base URL for the paired gizzi-code instance" helper —
/// factored out of `CodeThreadChatView`'s former `makeTerminalSession`/
/// `resolveMeshProxyBaseURL` so every gizzi-code REST client (pty, file
/// browser, session diff, …) attaches through the same mesh-aware path
/// instead of duplicating the mesh-proxy dance.
@MainActor
enum InstanceConnection {
    /// How a resolved target is reached.
    enum Route: Sendable, Equatable {
        /// An explicit gizzi URL (DEBUG `-gizzi-url`, a baked build setting)
        /// or a registered instance's public tunnel URL.
        case direct
        /// A registered mesh instance through the embedded tailnet proxy.
        case mesh
        /// The user's computer through the cloud-api runtime relay.
        case relay(runtimeId: String)
    }

    /// A resolved connection target: the base URL to talk to (already
    /// mesh-proxied if needed; an `allternit-relay://<runtimeId>`
    /// placeholder for the relay, which `APIClient` and the helpers below
    /// recognise) plus the display name of the instance or computer it
    /// came from.
    struct Resolved: Sendable {
        let baseURL: URL
        let instanceName: String?
        let route: Route

        init(baseURL: URL, instanceName: String?, route: Route = .direct) {
            self.baseURL = baseURL
            self.instanceName = instanceName
            self.route = route
        }

        var isRelay: Bool {
            if case .relay = route { return true }
            return false
        }
    }

    /// Resolves where gizzi-code calls go. Order:
    ///
    /// 1. DEBUG: an explicit `-gizzi-url` launch argument.
    /// 2. The instance the user pinned in the code thread's instance menu
    ///    (brought up over the mesh when it is a mesh address).
    /// 3. A registered mesh instance while the mesh node is already up.
    /// 4. An online registered instance with a public (non-mesh) URL.
    /// 5. The relay to the user's active computer (`RuntimeRelayStore`) —
    ///    the Release default, the way m.allternit.com reaches the runtime.
    /// 6. A baked `ALLTERNIT_GIZZI_CODE_URL` (DEBUG builds fall back to the
    ///    local dev server here).
    /// 7. A registered mesh instance, starting the mesh for it (up to ~60 s).
    ///
    /// nil when nothing is reachable: callers show `ConnectComputerView`.
    static func resolve(from store: InstanceStore) async -> Resolved? {
        #if DEBUG
        if let override = UserDefaults.standard.string(forKey: "gizzi-url"),
           let url = URL(string: override) {
            return Resolved(baseURL: url, instanceName: nil, route: .direct)
        }
        #endif

        if let pinnedID = store.selectedInstanceID,
           let pinned = store.instances.first(where: { $0.id == pinnedID }),
           let url = pinned.instanceURL {
            if !url.isMeshAddress {
                return Resolved(baseURL: url, instanceName: pinned.name, route: .direct)
            }
            if let proxyURL = await resolveMeshProxyBaseURL(for: url) {
                return Resolved(baseURL: proxyURL, instanceName: pinned.name, route: .mesh)
            }
            // A pinned mesh instance that can't come up: don't silently
            // switch to another target the user didn't choose.
            return nil
        }

        if MeshClient.shared.state.isUp,
           let mesh = store.instances.first(where: { $0.isOnline && $0.isMeshInstance }),
           let url = mesh.instanceURL,
           let proxyURL = try? await MeshClient.shared.localProxyBaseURL(forMeshURL: url) {
            return Resolved(baseURL: proxyURL, instanceName: mesh.name, route: .mesh)
        }

        if let tunnel = store.instances.first(where: { $0.isOnline && !$0.isMeshInstance }),
           let url = tunnel.instanceURL {
            return Resolved(baseURL: url, instanceName: tunnel.name, route: .direct)
        }

        let relay = RuntimeRelayStore.shared
        if let runtimeId = await relay.resolveActiveRuntimeId() {
            return Resolved(
                baseURL: RuntimeRelayClient.relayBaseURL(runtimeId: runtimeId),
                instanceName: relay.devices.first(where: { $0.id == runtimeId })?.displayName,
                route: .relay(runtimeId: runtimeId)
            )
        }

        if AppConfig.hasUsableGizziCodeURL {
            return Resolved(baseURL: AppConfig.gizziCodeBaseURL, instanceName: nil, route: .direct)
        }

        if let mesh = store.instances.first(where: { $0.isOnline && $0.isMeshInstance })
            ?? store.instances.first(where: { $0.isMeshInstance }),
           let url = mesh.instanceURL,
           let proxyURL = await resolveMeshProxyBaseURL(for: url) {
            return Resolved(baseURL: proxyURL, instanceName: mesh.name, route: .mesh)
        }
        return nil
    }

    /// Convenience resolver using the shared instance store.
    static func resolve() async -> Resolved? {
        await resolve(from: InstanceStore.shared)
    }

    /// The gizzi-code base URL a client built with `baseURL` should call
    /// now. A client constructed with the static default
    /// (`AppConfig.gizziCodeBaseURL`, i.e. every `.shared` client) resolves
    /// per call through `resolve()`; an explicit base URL is used as given.
    /// Throws `RuntimeRelayError.noComputer` when nothing is reachable.
    nonisolated static func gizziBaseURL(for baseURL: URL) async throws -> URL {
        guard baseURL == AppConfig.gizziCodeBaseURL else { return baseURL }
        guard let resolved = await resolve() else { throw RuntimeRelayError.noComputer }
        return resolved.baseURL
    }

    /// An `APIClient` for gizzi-code calls from a client built with
    /// `baseURL` (see `gizziBaseURL(for:)`). Relay placeholders are sent
    /// through `RuntimeRelayClient` by `APIClient.send` itself.
    nonisolated static func gizziClient(for baseURL: URL) async throws -> APIClient {
        APIClient(
            baseURL: try await gizziBaseURL(for: baseURL),
            tokenProvider: { try await AuthManager.shared.getToken() }
        )
    }

    // MARK: - One transport for runtime calls

    /// Sends `request`: through the relay when its URL is an
    /// `allternit-relay://` placeholder, otherwise straight out.
    nonisolated static func data(for request: URLRequest, session: URLSession = .shared) async throws -> (Data, URLResponse) {
        if let runtimeId = RuntimeRelayClient.runtimeId(in: request.url) {
            return try await RuntimeRelayClient.shared.send(request, runtimeId: runtimeId)
        }
        return try await session.data(for: request)
    }

    /// Streaming (SSE) variant of `data(for:)`.
    nonisolated static func bytes(for request: URLRequest, session: URLSession = .shared) async throws -> (URLSession.AsyncBytes, URLResponse) {
        if let runtimeId = RuntimeRelayClient.runtimeId(in: request.url) {
            return try await RuntimeRelayClient.shared.stream(request, runtimeId: runtimeId)
        }
        return try await session.bytes(for: request)
    }

    /// A WebSocket for `request` (an `http(s)`/`ws(s)` URL, or a relay
    /// placeholder). Relayed sockets come back resumed and past the tunnel
    /// handshake; direct ones are returned unresumed. Either way the caller
    /// calls `resume()` (a no-op on a running task).
    nonisolated static func webSocketTask(for request: URLRequest, session: URLSession = .shared) async throws -> URLSessionWebSocketTask {
        if let runtimeId = RuntimeRelayClient.runtimeId(in: request.url) {
            var path = "/"
            if let url = request.url, let components = URLComponents(url: url, resolvingAgainstBaseURL: false) {
                path = components.percentEncodedPath.isEmpty ? "/" : components.percentEncodedPath
                if let query = components.percentEncodedQuery, !query.isEmpty { path += "?\(query)" }
            }
            return try await RuntimeRelayClient.shared.webSocket(path: path, runtimeId: runtimeId)
        }
        var direct = request
        if let url = request.url, var components = URLComponents(url: url, resolvingAgainstBaseURL: false) {
            if components.scheme == "https" { components.scheme = "wss" }
            if components.scheme == "http" { components.scheme = "ws" }
            direct.url = components.url ?? url
        }
        return session.webSocketTask(with: direct)
    }

    // MARK: - Runtime API (allternit-api on the user's computer)

    /// Sends a request for an allternit-api route that lives on the user's
    /// computer, not on cloud-api — `/api/aci/*`, `/api/factory/*`,
    /// `/api/v1/cowork/sessions` — the way the PWA's fetch interceptor
    /// relays them (they are not cloud control-plane paths,
    /// cloud-control-plane-paths.ts):
    ///
    /// - Environment paired to cloud → the paired runtime's relay.
    /// - `apiBaseURL` is the cloud host (Release) → the active computer's
    ///   relay (`RuntimeRelayStore`); `RuntimeRelayError.noComputer` when
    ///   none.
    /// - Otherwise (DEBUG against a local allternit-api) → direct.
    nonisolated static func runtimeAPIData(for request: URLRequest, session: URLSession = .shared) async throws -> (Data, URLResponse) {
        if let runtimeId = try await runtimeAPIRelayTarget() {
            return try await RuntimeRelayClient.shared.send(request, runtimeId: runtimeId)
        }
        return try await session.data(for: request)
    }

    /// Streaming variant of `runtimeAPIData(for:)`.
    nonisolated static func runtimeAPIBytes(for request: URLRequest, session: URLSession = .shared) async throws -> (URLSession.AsyncBytes, URLResponse) {
        if let runtimeId = try await runtimeAPIRelayTarget() {
            return try await RuntimeRelayClient.shared.stream(request, runtimeId: runtimeId)
        }
        return try await session.bytes(for: request)
    }

    /// The runtime id runtime-API calls relay through, nil for direct.
    private nonisolated static func runtimeAPIRelayTarget() async throws -> String? {
        let (environment, paired) = await MainActor.run {
            (EnvironmentStore.shared.environment, EnvironmentStore.shared.pairedRuntimeId)
        }
        if environment == .cloud {
            guard let paired, !paired.isEmpty else { throw RuntimeRelayError.noComputer }
            return paired
        }
        guard AppConfig.apiBaseURL.host == AppConfig.cloudAPIBaseURL.host else { return nil }
        guard let runtimeId = await RuntimeRelayStore.shared.resolveActiveRuntimeId() else {
            throw RuntimeRelayError.noComputer
        }
        return runtimeId
    }

    /// Tunnels a localhost URL (e.g. `http://localhost:3000`) to the active
    /// mesh instance's loopback port (e.g. target `100.64.0.2:3000`) and
    /// returns the rewritten local proxy URL (e.g. `http://127.0.0.1:51234`).
    /// Returns the original URL if the host is not localhost or the mesh is not up.
    static func tunnelLocalhost(url: URL) async -> URL {
        guard let host = url.host, (host == "localhost" || host == "127.0.0.1") else {
            return url
        }
        guard let resolved = await resolve() else {
            return url
        }
        let targetInstanceURL = resolved.baseURL

        // Check if the resolved base URL is a mesh address, or if we need to retrieve
        // the original mesh host from the preferredInstance (if already loopback proxied).
        let meshHost: String
        if targetInstanceURL.isMeshAddress {
            guard let h = targetInstanceURL.host else { return url }
            meshHost = h
        } else if let host = targetInstanceURL.host, (host == "127.0.0.1" || host == "localhost"),
                  let preferred = InstanceStore.shared.preferredInstance,
                  let prefURL = preferred.instanceURL,
                  prefURL.isMeshAddress,
                  let h = prefURL.host {
            meshHost = h
        } else {
            return url
        }

        let port = url.port ?? 80
        guard let meshTargetURL = URL(string: "http://\(meshHost):\(port)") else {
            return url
        }
        do {
            let localPort = try await MeshClient.shared.proxyPort(forMeshURL: meshTargetURL)
            var components = URLComponents(url: url, resolvingAgainstBaseURL: false)
            components?.host = "127.0.0.1"
            components?.port = localPort
            return components?.url ?? url
        } catch {
            print("Localhost tunneling failed: \(error.localizedDescription)")
            return url
        }
    }

    /// Convenience for callers that only need the base URL (file browser,
    /// diff viewer REST clients — no display-name requirement).
    static func resolveBaseURL(from store: InstanceStore) async -> URL? {
        await resolve(from: store)?.baseURL
    }

    /// Resolves the loopback base URL (`http://127.0.0.1:<port>`) for a mesh
    /// instance: with the node already up, straight to the proxy; otherwise
    /// starts it via platform enrollment (a DEBUG auth key takes precedence
    /// inside MeshClient) and waits for the join (enroll plus the Go-side
    /// start, which blocks up to ~60s — callers should show a "Connecting…"
    /// state while this runs). nil when the mesh can't be used: a failed
    /// enroll or start leaves state `.failed`.
    private static func resolveMeshProxyBaseURL(for url: URL) async -> URL? {
        let mesh = MeshClient.shared
        if !mesh.state.isUp {
            await mesh.startWithPlatformAuth()
            // start() is fire-and-forget; poll the published state until
            // the join settles.
            while mesh.state == .starting {
                try? await Task.sleep(for: .milliseconds(250))
                if Task.isCancelled { return nil }
            }
            guard mesh.state.isUp else { return nil }
        }
        return try? await mesh.localProxyBaseURL(forMeshURL: url)
    }
}
