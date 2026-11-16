import Foundation
import SwiftUI
import AllternitFilesKit

extension Notification.Name {
    /// Posted by `FilesDriveBridge` after a sync changed the Files computers.
    static let filesDriveDidSync = Notification.Name("allternit.filesDriveDidSync")
}

/// The app's in-app Files screens (plan step 21) read through this: which
/// computers have Files (the same list the File Provider shows, kept by
/// `FilesDriveBridge` in `FilesSharedStore`), one relay `FilesClient` per
/// computer, and what an Intent asked the app to open.
@MainActor
final class FilesDrive: ObservableObject {
    static let shared = FilesDrive()

    enum Launch: Equatable, Identifiable {
        case browse(ComputerId)
        case scan(ComputerId)
        var computerId: ComputerId {
            switch self { case .browse(let id), .scan(let id): return id }
        }
        var id: String {
            switch self { case .browse(let id): return "browse/\(id)"; case .scan(let id): return "scan/\(id)" }
        }
    }

    @Published private(set) var computers: [FilesSharedComputer] = []
    /// Set by the Files shortcuts; `FilesLaunchHost` opens it and clears it.
    @Published var launch: Launch?

    var store: FilesSharedStore { FilesDriveBridge.shared.store }
    private var clients: [ComputerId: FilesClient] = [:]
    private var observer: NSObjectProtocol?

    private init() {
        reload()
        observer = NotificationCenter.default.addObserver(forName: .filesDriveDidSync, object: nil, queue: .main) { _ in
            Task { @MainActor in FilesDrive.shared.reload() }
        }
    }

    /// Files is on when the bridge set this phone up (flag on, a Files key
    /// minted, at least one computer). Off: no Files row, nothing changes.
    var isEnabled: Bool { !computers.isEmpty && store.token() != nil }

    func reload() {
        computers = store.computers
        clients = clients.filter { id, _ in computers.contains { $0.id == id } }
    }

    func computer(_ id: ComputerId) -> FilesSharedComputer? { computers.first { $0.id == id } }

    func hasFiles(_ id: ComputerId) -> Bool { isEnabled && computer(id) != nil }

    /// A relay client for `computerId`, the same transport the extension uses.
    func client(for computerId: ComputerId) -> FilesClient {
        if let client = clients[computerId] { return client }
        let client = store.makeClient(computerId: computerId)
        clients[computerId] = client
        return client
    }

    func name(of computerId: ComputerId) -> String { computer(computerId)?.name ?? "Computer" }
}

/// cloud-api's Files routes that act for the signed-in user (SPEC section 2,
/// "cloud-api"): wake settings, shares, share access (audit) and storage.
/// They carry the Clerk session like the rest of the app, not the Files key.
enum FilesCloudAPI {
    private static func url(_ path: String) -> URL {
        AppConfig.cloudAPIBaseURL.appendingPathComponent("api/v1/computer-files").appendingPathComponent(path)
    }

    private static func send(_ method: String, _ path: String, body: Data? = nil) async throws -> Data {
        let client = APIClient.shared
        var request = try await client.authorizedRequest(url: url(path), method: method)
        request.httpBody = body
        let (data, response) = try await client.session.data(for: request)
        try client.validate(response, data: data, method: method)
        return data
    }

    private static func decode<T: Decodable>(_ data: Data) throws -> T {
        try JSONDecoder().decode(T.self, from: data)
    }

    static func wakeSetting(_ computerId: ComputerId) async throws -> WakeSetting {
        let settings: WakeSettings = try decode(try await send("GET", "\(computerId)/wake-settings"))
        return settings.wake
    }

    @discardableResult
    static func setWakeSetting(_ wake: WakeSetting, for computerId: ComputerId) async throws -> WakeSetting {
        let body = try JSONEncoder().encode(UpdateWakeSettingsRequest(wake: wake))
        let settings: WakeSettings = try decode(try await send("PUT", "\(computerId)/wake-settings", body: body))
        return settings.wake
    }

    static func shares(_ computerId: ComputerId) async throws -> [Share] {
        let response: SharesResponse = try decode(try await send("GET", "\(computerId)/shares"))
        return response.shares
    }

    static func revokeShare(_ shareId: String) async throws {
        _ = try await send("DELETE", "shares/\(shareId)")
    }

    /// `GET /shares/:id/access`: a bare `ShareAccess[]` (SPEC); an
    /// `{access: […]}` wrapper is read too.
    static func access(_ shareId: String) async throws -> [ShareAccess] {
        struct Wrapped: Decodable { let access: [ShareAccess] }
        let data = try await send("GET", "shares/\(shareId)/access")
        if let list = try? JSONDecoder().decode([ShareAccess].self, from: data) { return list }
        return try JSONDecoder().decode(Wrapped.self, from: data).access
    }

    static func storage(_ computerId: ComputerId) async throws -> StorageDiscovery {
        try decode(try await send("GET", "\(computerId)/storage"))
    }
}

enum FilesFormat {
    static func bytes(_ value: UInt64) -> String {
        ByteCountFormatter.string(fromByteCount: Int64(clamping: value), countStyle: .file)
    }

    static func date(_ raw: String?) -> String {
        guard let raw, let date = parse(raw) else { return "" }
        if Calendar.current.isDateInToday(date) { return "Today " + date.formatted(date: .omitted, time: .shortened) }
        if Calendar.current.isDateInYesterday(date) { return "Yesterday" }
        return date.formatted(.dateTime.month(.abbreviated).day())
    }

    static func time(_ raw: String?) -> String {
        guard let raw, let date = parse(raw) else { return "" }
        return date.formatted(date: .omitted, time: .shortened)
    }

    static func parse(_ raw: String) -> Date? {
        let fractional = ISO8601DateFormatter()
        fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return fractional.date(from: raw) ?? ISO8601DateFormatter().date(from: raw)
    }

    static func wakeLabel(_ wake: WakeSetting) -> String {
        switch wake {
        case .ask: return "Ask first"
        case .auto: return "Automatically"
        case .never: return "Never"
        }
    }

    static func roleLabel(_ role: ShareRole) -> String {
        switch role {
        case .viewDownload: return "View and download"
        case .uploadOnly: return "Upload only"
        case .edit: return "Can edit"
        case .comment: return "Can comment"
        }
    }

    /// SF Symbol per root kind.
    static func symbol(_ kind: RootKind) -> String {
        switch kind {
        case .allternitHome: return "house"
        case .inbox: return "tray"
        case .shared: return "person.2"
        case .agentOutput: return "sparkles"
        case .downloads: return "arrow.down.circle"
        case .meetings: return "video"
        case .scans: return "doc.viewfinder"
        case .memoryDrive: return "brain"
        case .code: return "chevron.left.forwardslash.chevron.right"
        case .models: return "cpu"
        case .userAdded: return "folder"
        }
    }

    /// The order the design lists roots in.
    static func order(_ kind: RootKind) -> Int {
        let all: [RootKind] = [.inbox, .shared, .agentOutput, .downloads, .meetings, .scans, .allternitHome,
                               .memoryDrive, .code, .models, .userAdded]
        return all.firstIndex(of: kind) ?? all.count
    }
}
