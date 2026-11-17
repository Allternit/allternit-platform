import Foundation
import UIKit
import Mesh

/// Wrapper around the embedded tsnet node (Frameworks/Mesh.xcframework —
/// vendored from infrastructure/mesh/tsnet-ios). One node per app process:
/// joins the Headscale tailnet (AppConfig.meshControlURL) with a pre-auth
/// key and exposes `meshGET` for reaching tailnet-only services.
///
/// The gobind entry points (`MeshNode.start`, `get`, `close`) are ordinary
/// blocking C calls into the Go runtime — `start` blocks up to ~60s while
/// the node registers — so they always run off the main actor. tsnet stores
/// its node key in `Application Support/mesh`, so the node keeps the same
/// tailnet identity across launches (and across the background-restart
/// below) as long as the directory survives.
@MainActor
final class MeshClient: ObservableObject {
    static let shared = MeshClient()

    enum State: Equatable {
        case idle
        case starting
        case up(meshIP: String)
        case failed(String)

        /// True when the node was up (or trying) — used by the foreground
        /// restart, which should not resurrect a stopped/failed node.
        var isLive: Bool {
            switch self {
            case .starting, .up: return true
            case .idle, .failed: return false
            }
        }

        /// True only when the node is online — the loopback proxy (and
        /// `meshGET`) is usable in this state alone.
        var isUp: Bool {
            if case .up = self { return true }
            return false
        }
    }

    @Published private(set) var state: State = .idle

    // `nonisolated(unsafe)`: MeshNode is a gobind Obj-C object (not Sendable)
    // and its methods must run off the main actor — same Swift 6.0 escape
    // hatch PtySession documents for its deinit teardown. Safe by invariant:
    // the reference is only swapped on the main actor (start/stop), and every
    // gobind call just RPCs into the Go runtime, which serializes internally.
    nonisolated(unsafe) private var node: MeshNode?

    /// Last successful start parameters, so the foreground restart can rejoin
    /// without the caller re-supplying the key.
    private var lastStartParams: (controlURL: String, authKey: String)?
    /// Set when the app backgrounds while the node is live; consumed by the
    /// foreground restart.
    private var restartOnForeground = false

    /// The running loopback proxy: its tailnet target ("host:port") and the
    /// local 127.0.0.1 port it listens on. One proxy per node (the Go side
    /// enforces this) — a different target replaces it. Cleared whenever the
    /// node is torn down, because the proxy dies with it (foreground restart
    /// included); the next `proxyPort(forMeshURL:)` re-creates it lazily.
    private var activeProxy: (target: String, localPort: Int)?

    /// Sendable box for handing the (non-Sendable) MeshNode to a detached
    /// task — see the `nonisolated(unsafe)` note on `node`.
    private struct NodeBox: @unchecked Sendable {
        let node: MeshNode
    }

    // `nonisolated(unsafe)` so the (non-isolated, Swift 6.0) deinit can
    // unregister them — same constraint VoiceModeViewModel documents for its
    // interruption observer. MainActor-isolated everywhere else.
    nonisolated(unsafe) private var foregroundObserver: NSObjectProtocol?
    nonisolated(unsafe) private var backgroundObserver: NSObjectProtocol?

    private init() {
        // iOS suspends the process in the background and the tailnet sockets
        // die with it; tsnet can't revive them. On foreground, if the node
        // was live, tear it down and re-start with the same params — the
        // persisted dataDir means the rejoin reuses the same node key, so
        // the mesh IP is stable. Known limitation: anything mid-flight
        // through the old node (e.g. a meshGET) fails across the transition.
        backgroundObserver = NotificationCenter.default.addObserver(
            forName: UIApplication.didEnterBackgroundNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            Task { @MainActor in
                guard let self else { return }
                self.restartOnForeground = self.state.isLive
            }
        }
        foregroundObserver = NotificationCenter.default.addObserver(
            forName: UIApplication.willEnterForegroundNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            Task { @MainActor in
                guard let self, self.restartOnForeground,
                      let params = self.lastStartParams else { return }
                self.restartOnForeground = false
                self.start(controlURL: params.controlURL, authKey: params.authKey)
            }
        }
    }

    deinit {
        if let foregroundObserver { NotificationCenter.default.removeObserver(foregroundObserver) }
        if let backgroundObserver { NotificationCenter.default.removeObserver(backgroundObserver) }
    }

    // MARK: - Lifecycle

    /// Brings the node up. No-op while a start is already in flight; any
    /// previous node is closed first. `authKey` is a Headscale pre-auth key
    /// (from `startWithPlatformAuth()`'s enroll response, or a DEBUG manual
    /// key — see `debugAuthKey`).
    func start(controlURL: String = AppConfig.meshControlURL, authKey: String) {
        guard state != .starting else { return }

        // Box before crossing: only @unchecked-Sendable values may be
        // captured by the detached (sending) closure below.
        let oldBox = node.map { NodeBox(node: $0) }
        node = nil
        activeProxy = nil // the old node's proxy dies with it
        state = .starting
        lastStartParams = (controlURL, authKey)

        let dataDir: String
        do {
            dataDir = try Self.meshDataDir().path
        } catch {
            state = .failed("Couldn't create mesh data directory: \(error.localizedDescription)")
            return
        }

        Task.detached { [weak self] in
            if let oldBox {
                try? oldBox.node.close()
            }
            guard let newNode = MeshNewNode("allternit-ios") else {
                await self?.startDidFail("MeshNewNode returned nil")
                return
            }
            let newBox = NodeBox(node: newNode)
            do {
                // Blocks until the node is online (up to ~60s).
                try newBox.node.start(controlURL, authKey: authKey, dataDir: dataDir)
                let meshIP = newBox.node.meshIP()
                await self?.startDidSucceed(newBox, meshIP: meshIP.isEmpty ? "unknown" : meshIP)
            } catch {
                try? newBox.node.close()
                await self?.startDidFail(error.localizedDescription)
            }
        }
    }

    /// Brings the node up with platform-minted credentials: enrolls via the
    /// cloud API (`MeshEnrollClient` → `POST /api/v1/mesh/enroll`) and starts
    /// with the returned control URL + pre-auth key. No key caching — enroll
    /// mints a fresh 24h single-use key on every start (platform-side
    /// refresh cadence is a follow-up). A DEBUG `debugAuthKey` (launch arg /
    /// Settings row) always wins and skips enrollment entirely. Enrollment
    /// failures land in state: 401 → sign-in required, 503
    /// (`mesh_not_configured`) → mesh unavailable, anything else → the
    /// error's message.
    func startWithPlatformAuth() async {
        if let key = Self.debugAuthKey {
            start(authKey: key)
            return
        }
        do {
            let enrollment = try await MeshEnrollClient().enroll()
            start(controlURL: enrollment.controlUrl, authKey: enrollment.authKey)
        } catch let error as APIError {
            switch error {
            case .httpError(let statusCode, let message):
                switch statusCode {
                case 401: state = .failed("Sign in to use mesh")
                case 503: state = .failed("Mesh unavailable")
                default: state = .failed(message ?? "Enroll failed with status \(statusCode)")
                }
            default:
                state = .failed(error.localizedDescription)
            }
        } catch {
            state = .failed(error.localizedDescription)
        }
    }

    /// Completion hops from start()'s detached task. Methods (not
    /// `MainActor.run` closures) so `self` crosses as the actor receiver
    /// instead of a task-isolated capture.
    private func startDidSucceed(_ box: NodeBox, meshIP: String) {
        node = box.node
        state = .up(meshIP: meshIP)
    }

    private func startDidFail(_ message: String) {
        state = .failed(message)
    }

    /// Shuts the node down and forgets the start params (no foreground
    /// restart). The node identity on disk is kept — the next start reuses it.
    func stop() {
        let oldBox = node.map { NodeBox(node: $0) }
        node = nil
        activeProxy = nil
        lastStartParams = nil
        restartOnForeground = false
        state = .idle
        if let oldBox {
            Task.detached {
                try? oldBox.node.stopProxy()
                try? oldBox.node.close()
            }
        }
    }

    /// Person change (phone calm I14b): the tailnet node key in
    /// `Application Support/mesh` was enrolled for the leaving person, so the
    /// node stops and the key is deleted. The next start enrolls a new node
    /// for whoever is signed in. The DEBUG `mesh-auth-key` override is a
    /// developer setting and is kept.
    func forgetIdentity() {
        stop()
        guard let support = try? FileManager.default.url(for: .applicationSupportDirectory, in: .userDomainMask,
                                                         appropriateFor: nil, create: false) else { return }
        try? FileManager.default.removeItem(at: support.appendingPathComponent("mesh", isDirectory: true))
    }

    // MARK: - Traffic

    /// HTTP GET through the tailnet using the node's identity (response body
    /// capped at 4 MiB by the framework). Runs off the calling executor —
    /// the underlying gobind call blocks.
    nonisolated func meshGET(_ url: String) async throws -> String {
        guard let node else { throw MeshClientError.notUp }
        // Box before crossing into the detached (sending) closure.
        let box = NodeBox(node: node)
        return try await Task.detached {
            // `get(_:error:)` returns a nonnull NSString*, so Swift imports
            // the error as an out-parameter instead of `throws`.
            var error: NSError?
            let body = box.node.get(url, error: &error)
            if let error { throw error }
            return body
        }.value
    }

    enum MeshClientError: LocalizedError {
        case notUp
        case badMeshURL

        var errorDescription: String? {
            switch self {
            case .notUp: return "Mesh node is not up"
            case .badMeshURL: return "Instance URL has no usable host"
            }
        }
    }

    // MARK: - Loopback proxy

    /// The tailnet target (host + port, default 80) a mesh instance URL
    /// points at; nil when the URL has no usable host. `nonisolated`: a
    /// pure function — the DEBUG self-check calls it off-actor.
    nonisolated static func proxyTarget(forMeshURL url: URL) -> (host: String, port: Int)? {
        guard let host = url.host, !host.isEmpty else { return nil }
        return (host, url.port ?? 80)
    }

    /// Starts (or reuses) the loopback proxy for a mesh instance URL like
    /// `http://100.64.0.2:4096` and returns the local port: the Go node
    /// listens on 127.0.0.1:<port> and forwards each connection through the
    /// tailnet to the URL's host:port. This is the ONLY way URLSession can
    /// reach a mesh address — 100.64.0.0/10 is in-process userspace, so iOS
    /// networking can't route it. Only available while state is `.up`.
    ///
    /// Idempotent per target: a running proxy for the same host:port is
    /// returned as-is. A different target replaces the running proxy (the Go
    /// side allows one per node), which the terminal's single-instance
    /// attach makes sufficient.
    func proxyPort(forMeshURL url: URL) async throws -> Int {
        guard state.isUp, let node else { throw MeshClientError.notUp }
        guard let target = Self.proxyTarget(forMeshURL: url) else { throw MeshClientError.badMeshURL }
        let targetKey = "\(target.host):\(target.port)"
        if let activeProxy, activeProxy.target == targetKey {
            return activeProxy.localPort
        }
        // Box before crossing into the detached (sending) closure — same
        // pattern as meshGET; gobind blocks, so it runs off the main actor.
        let box = NodeBox(node: node)
        let localPort = try await Task.detached {
            // gobind maps the (int, error) return to BOOL + ret0_ out-param.
            var localPort = 0
            try box.node.startProxy(target.host, targetPort: target.port, ret0_: &localPort)
            return localPort
        }.value
        activeProxy = (targetKey, localPort)
        return localPort
    }

    /// The loopback base URL (`http://127.0.0.1:<port>`) a `PtyClient` can
    /// target to reach a mesh instance through the proxy. TCP-level
    /// forwarding preserves the loopback Host header, which the gizzi
    /// server's loopback allowlist accepts.
    func localProxyBaseURL(forMeshURL url: URL) async throws -> URL? {
        let port = try await proxyPort(forMeshURL: url)
        return URL(string: "http://127.0.0.1:\(port)")
    }

    // MARK: - DEBUG auth key

    #if DEBUG
    /// DEBUG-only self-check for the mesh proxy resolution path (run with
    /// the `-mesh-proxy-selfcheck` launch argument): 100.64.0.0/10
    /// classification, target parsing, and the loopback base-URL shape. The
    /// live proxy itself needs a joined tailnet plus a registered mesh
    /// gizzi instance, so this covers the pure parts only. Prints one line
    /// per check and returns false on any mismatch.
    @discardableResult
    static func runProxySelfCheck() -> Bool {
        var ok = true
        func check(_ label: String, _ condition: Bool) {
            if !condition { ok = false }
            print("[mesh-proxy-selfcheck] \(label): \(condition ? "ok" : "FAIL")")
        }
        check("100.64.0.2 is mesh", URL(string: "http://100.64.0.2:4096")?.isMeshAddress == true)
        check("100.127.255.254 is mesh", URL(string: "http://100.127.255.254:4096")?.isMeshAddress == true)
        check("100.63.255.255 is not mesh", URL(string: "http://100.63.255.255:4096")?.isMeshAddress == false)
        check("100.128.0.0 is not mesh", URL(string: "http://100.128.0.0:4096")?.isMeshAddress == false)
        check("tunnel URL is not mesh", URL(string: "https://xyz.trycloudflare.com")?.isMeshAddress == false)
        check("127.0.0.1 is not mesh", URL(string: "http://127.0.0.1:4096")?.isMeshAddress == false)
        check("10.0.0.5 is not mesh", URL(string: "http://10.0.0.5:4096")?.isMeshAddress == false)
        let target = URL(string: "http://100.64.0.2:4096").flatMap(proxyTarget(forMeshURL:))
        check("target parses host", target?.host == "100.64.0.2")
        check("target parses port", target?.port == 4096)
        check("default port is 80", URL(string: "http://100.64.0.2").flatMap(proxyTarget(forMeshURL:))?.port == 80)
        check("bad URL has no target", URL(string: "http://").flatMap(proxyTarget(forMeshURL:)) == nil)
        check("loopback base URL shape", URL(string: "http://127.0.0.1:51234")?.absoluteString == "http://127.0.0.1:51234")
        return ok
    }
    #endif

    /// Pre-auth key source (v1, DEBUG only): the `-mesh-auth-key <key>`
    /// launch argument, or the Settings debug row — both land in
    /// UserDefaults under `mesh-auth-key` (launch args sit in the arguments
    /// domain, so the flag wins when both are set). Release builds have no
    /// key entry at all. When set, it overrides platform enrollment
    /// (`startWithPlatformAuth`) — the debug override always wins.
    static var debugAuthKey: String? {
        #if DEBUG
        let key = UserDefaults.standard.string(forKey: "mesh-auth-key")?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return (key?.isEmpty == false) ? key : nil
        #else
        return nil
        #endif
    }

    // MARK: - Data directory

    /// `Application Support/mesh`, created on first use. Keeping a stable
    /// directory is what makes the node identity (and mesh IP) persist
    /// across launches.
    private static func meshDataDir() throws -> URL {
        let support = try FileManager.default.url(
            for: .applicationSupportDirectory,
            in: .userDomainMask,
            appropriateFor: nil,
            create: true
        )
        let dir = support.appendingPathComponent("mesh", isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }
}
