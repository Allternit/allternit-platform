import Foundation

// Start anywhere (docs/contracts/session-start): pick a computer and a
// folder, and the chat's first message starts a real session there. Same
// contract and names as the web client (allternit-ai src/lib/session-start).
// Device routes go through the cloud relay to the picked runtime, the way
// `TerminalServiceClient` / `RuntimeRaw.send` reach it; the folder snapshots
// (`GET /api/v1/start-options`) come from cloud-api itself.

/// Permission ids shared with the app (`ChatPermissionMode` raw values) plus `bypass`.
enum StartMode: String, Codable, CaseIterable, Sendable {
    case auto
    case `default`
    case plan
    case bypass
}

struct StartFolderGit: Codable, Equatable, Sendable {
    let remote: String
    let branch: String
    var head: String? = nil
    var dirty: Bool? = nil
    var ahead: Int? = nil
}

/// One recent folder on a computer (max 30 per device).
struct StartFolder: Codable, Equatable, Sendable, Identifiable {
    let path: String
    let name: String
    var lastUsedAt: String? = nil
    var git: StartFolderGit? = nil

    var id: String { path }
}

/// A runtime's permission default and the modes it supports (`modes.<runtimeId>`).
struct StartModeInfo: Codable, Equatable, Sendable {
    let `default`: String
    let supported: [String]
}

/// `GET /api/v1/session-start/options` on the device.
struct StartOptions: Codable, Equatable, Sendable {
    struct Device: Codable, Equatable, Sendable {
        let runtimeId: String
        let name: String
        var os: String? = nil
        let remoteStart: Bool
    }

    let device: Device
    let folders: [StartFolder]
    let modes: [String: StartModeInfo]
    let defaultRuntime: String

    /// The computer's own default mode for `runtime` (else its default runtime's).
    func defaultMode(for runtime: String? = nil) -> String? {
        if let runtime, let info = modes[runtime] { return info.default }
        return modes[defaultRuntime]?.default
    }
}

/// Where the session runs: a folder on that computer, or a git folder a
/// cloud computer clones (D7).
enum StartFolderPick: Codable, Equatable, Sendable {
    case path(String)
    case git(remote: String, branch: String, head: String?)

    private enum Keys: String, CodingKey { case path, git }
    private struct Git: Codable { let remote: String; let branch: String; let head: String? }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Keys.self)
        if let path = try c.decodeIfPresent(String.self, forKey: .path) {
            self = .path(path)
        } else {
            let g = try c.decode(Git.self, forKey: .git)
            self = .git(remote: g.remote, branch: g.branch, head: g.head)
        }
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: Keys.self)
        switch self {
        case .path(let path): try c.encode(path, forKey: .path)
        case .git(let remote, let branch, let head): try c.encode(Git(remote: remote, branch: branch, head: head), forKey: .git)
        }
    }

    /// The short name on the + sheet's Folder row (web `pendingFolderName`).
    var displayName: String {
        switch self {
        case .path(let path):
            let trimmed = path.hasSuffix("/") ? String(path.dropLast()) : path
            return trimmed.split(separator: "/").last.map(String.init) ?? path
        case .git(let remote, _, _):
            let base = remote.hasSuffix(".git") ? String(remote.dropLast(4)) : remote
            return base.split(whereSeparator: { $0 == "/" || $0 == ":" }).last.map(String.init) ?? remote
        }
    }

    /// The folder text stored on the binding.
    var bindingText: String {
        switch self {
        case .path(let path): return path
        case .git(let remote, let branch, _): return "\(remote)#\(branch)"
        }
    }
}

struct StartOrigin: Codable, Equatable, Sendable {
    let surface: String
    let deviceName: String
    var parentSessionId: String? = nil
    var depth: Int? = nil
}

/// `POST /api/v1/session-start` body.
struct StartRequest: Codable, Equatable, Sendable {
    let folder: StartFolderPick
    let runtime: String
    var mode: String? = nil
    var confirmBypass: Bool? = nil
    var prompt: String? = nil
    let origin: StartOrigin
}

/// 201 from a start: an engine session (Gizzi) or a pane terminal (any other CLI).
enum StartResult: Decodable, Equatable, Sendable {
    case engine(sessionId: String, directory: String)
    case terminal(terminalId: String, cwd: String, agent: String)

    private enum Keys: String, CodingKey { case kind, sessionId, directory, terminalId, cwd, agent }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Keys.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "engine":
            self = .engine(sessionId: try c.decode(String.self, forKey: .sessionId),
                           directory: (try? c.decode(String.self, forKey: .directory)) ?? "")
        case "terminal":
            self = .terminal(terminalId: try c.decode(String.self, forKey: .terminalId),
                             cwd: (try? c.decode(String.self, forKey: .cwd)) ?? "",
                             agent: (try? c.decode(String.self, forKey: .agent)) ?? "")
        case let other:
            throw DecodingError.dataCorruptedError(forKey: .kind, in: c, debugDescription: "Unknown start kind \(other)")
        }
    }
}

/// An error body `{error, message, action}` (errors.json carries `status` too).
struct StartErrorBody: Decodable, Equatable, Sendable {
    var status: Int? = nil
    let error: String
    let message: String
    var action: String? = nil
}

/// A refused start, with the computer's own words and what to do next.
struct StartSessionError: LocalizedError, Equatable, Sendable {
    let status: Int
    let code: String
    let message: String
    let action: String?

    var errorDescription: String? { inline }

    /// Message, then action, for an inline error line.
    var inline: String {
        guard let action, !action.isEmpty else { return message }
        return "\(message) \(action)"
    }

    static func from(status: Int, data: Data) -> StartSessionError {
        if let body = try? JSONDecoder().decode(StartErrorBody.self, from: data) {
            return StartSessionError(status: status, code: body.error, message: body.message, action: body.action)
        }
        return StartSessionError(status: status, code: "unknown",
                                 message: plainErrorText(message: nil, status: status),
                                 action: nil)
    }
}

/// One device in `GET /api/v1/start-options` (the cloud snapshot, readable while it sleeps).
struct StartSnapshotDevice: Codable, Equatable, Sendable {
    let runtimeId: String
    let name: String
    var kind: String? = nil
    var remoteStart: Bool? = nil
    var updatedAt: String? = nil
    var folders: [StartFolder] = []
    var modes: [String: StartModeInfo]? = nil
    var defaultRuntime: String? = nil

    private enum CodingKeys: String, CodingKey { case runtimeId, name, kind, remoteStart, updatedAt, folders, modes, defaultRuntime }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        runtimeId = try c.decode(String.self, forKey: .runtimeId)
        name = (try? c.decodeIfPresent(String.self, forKey: .name)) ?? ""
        kind = try? c.decodeIfPresent(String.self, forKey: .kind)
        remoteStart = try? c.decodeIfPresent(Bool.self, forKey: .remoteStart)
        updatedAt = try? c.decodeIfPresent(String.self, forKey: .updatedAt)
        folders = (try? c.decodeIfPresent([StartFolder].self, forKey: .folders)) ?? []
        modes = try? c.decodeIfPresent([String: StartModeInfo].self, forKey: .modes)
        defaultRuntime = try? c.decodeIfPresent(String.self, forKey: .defaultRuntime)
    }
}

struct StartSnapshots: Codable, Equatable, Sendable {
    let devices: [StartSnapshotDevice]
}

/// `GET /api/v1/runtime-presence` row with the start-anywhere fields.
struct StartPresence: Decodable, Equatable, Sendable {
    let runtimeId: String
    let online: Bool
    var lastSeen: String? = nil
    var remoteStart: Bool? = nil
    /// desktop | cloud | server
    var kind: String? = nil
}

/// Where a chat thread runs, stored on the session as `metadata.startBinding`
/// (web binding.ts, same keys).
struct StartBinding: Codable, Equatable, Sendable {
    enum Kind: String, Codable, Sendable { case engine, terminal }

    let runtimeId: String
    let deviceName: String
    let folder: String
    let runtime: String
    let mode: String
    let kind: Kind
    var sessionId: String? = nil
    var terminalId: String? = nil
    let origin: StartOrigin
}

/// The device routes for one runtime, through the relay (a sleeping cloud
/// computer wakes: `RuntimeRelayClient.send` waits out `runtime_warming`).
final class SessionStartClient: @unchecked Sendable {
    let runtimeId: String

    init(runtimeId: String) {
        self.runtimeId = runtimeId
    }

    private var api: APIClient {
        APIClient(baseURL: RuntimeRelayClient.relayBaseURL(runtimeId: runtimeId),
                  tokenProvider: { try await AuthManager.shared.getToken() })
    }

    /// Folders + per-runtime permission defaults, in one relay round trip.
    func options() async throws -> StartOptions {
        let (data, status) = try await RuntimeRaw.send(api, path: "api/v1/session-start/options")
        guard (200...299).contains(status) else { throw StartSessionError.from(status: status, data: data) }
        do {
            return try JSONDecoder().decode(StartOptions.self, from: data)
        } catch {
            throw APIError.decoding(error)
        }
    }

    /// Starts the session. The same `idempotencyKey` on a retry returns the
    /// first start instead of a second one.
    func start(_ body: StartRequest, idempotencyKey: String) async throws -> StartResult {
        let api = self.api
        var request = try await api.authorizedRequest(path: "api/v1/session-start", method: "POST")
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue(idempotencyKey, forHTTPHeaderField: "Idempotency-Key")
        request.httpBody = try JSONEncoder().encode(body)
        let (data, response) = try await api.send(request)
        guard let http = response as? HTTPURLResponse else { throw APIError.invalidResponse }
        guard (200...299).contains(http.statusCode) else { throw StartSessionError.from(status: http.statusCode, data: data) }
        do {
            return try JSONDecoder().decode(StartResult.self, from: data)
        } catch {
            throw APIError.decoding(error)
        }
    }

    /// Every device's last folder snapshot, from cloud-api (works while a Mac sleeps).
    static func snapshots() async throws -> StartSnapshots {
        try await APIClient.shared.get(path: "start-options")
    }

    /// Presence with `remoteStart` and `kind`.
    static func presence() async throws -> [StartPresence] {
        try await APIClient.shared.get(path: "runtime-presence")
    }

    /// The contract's runtime id for a chip runtime id: `allternit` is the
    /// Gizzi engine; vendor CLIs use their short id (`claude-code` → `claude`).
    static func contractRuntime(_ chipRuntimeId: String) -> String {
        switch chipRuntimeId {
        case "allternit", "gizzi": return "allternit"
        case "claude-code", "claude-cli": return "claude"
        case "codex-cli": return "codex"
        default:
            for suffix in ["-cli", "-code"] where chipRuntimeId.hasSuffix(suffix) {
                return String(chipRuntimeId.dropLast(suffix.count))
            }
            return chipRuntimeId
        }
    }
}
