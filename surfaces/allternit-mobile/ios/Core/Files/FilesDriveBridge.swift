import Foundation
import UIKit
import AllternitFilesKit

/// App side of the Files drive File Provider (plan step 19). The extension
/// can't sign in (no Clerk in an extension, and Clerk session tokens last a
/// minute), list computers or run a mesh node, so the app does it and
/// leaves the result in the App Group + shared keychain (`FilesSharedStore`):
///
/// 1. reads the account's `files_drive` flag (`GET /me/flags`); with it off
///    (or unset) every Files domain is removed and nothing else happens;
/// 2. makes sure there is a Files token: a `compute`-scoped API key minted
///    for this phone (`POST /api/v1/api-keys`, "Files on <device>"), the
///    Bearer the extension sends to cloud-api's relay and computer-files
///    routes; minted again when the extension saw a 401;
/// 3. lists the user's computers (`GET /runtime-devices`, phones left out)
///    and makes the File Provider domains match: one location per computer;
/// 4. signals each domain, so Files picks up changes made on the computers;
/// 5. re-reads files pinned "Keep offline" that iOS evicted;
/// 6. picks up the Files key envelope once another device approved this one
///    (Settings > Files > Devices, `FilesKeysSection`).
///
/// Runs on every foreground (AppDelegate). Sign-out revokes the key and
/// removes the domains, which deletes every file iOS kept for them.
@MainActor
final class FilesDriveBridge {
    static let shared = FilesDriveBridge()

    let store = FilesSharedStore(appGroup: AppGroup.identifier, keychainGroup: SharedAccess.accessGroup)
    private var running = false

    private init() {}

    func sync() async {
        guard !running else { return }
        running = true
        defer { running = false }
        if SampleWorkspace.isEnabled { return }
        guard AuthManager.shared.isSignedIn else { return }
        // Unreachable: keep whatever was set up last time.
        guard let enabled = await readFlag() else { return }
        guard enabled else {
            if !store.computers.isEmpty || store.token() != nil { await turnOff() }
            return
        }

        store.cloudBaseURL = AppConfig.cloudAPIBaseURL
        if store.deviceId == nil { store.deviceId = Self.makeDeviceId() }
        store.deviceName = UIDevice.current.name
        do {
            try await ensureToken()
            let computers = try await RuntimeRelayClient.shared.fetchDevices()
                .filter { !Self.phoneRuntimeTypes.contains($0.runtimeType) }
                .map { FilesSharedComputer(id: $0.id, name: $0.displayName, runtimeType: $0.runtimeType) }
            store.computers = computers
            NotificationCenter.default.post(name: .filesDriveDidSync, object: nil)
            try await FilesDomains.sync(computers)
            await FilesPinKeeper.materialize(store: store)
            // Picks up a Files key another device approved (or a rotated
            // one), so the extension can open replicas without this screen
            // being opened. Never makes a key or sends a request by itself.
            _ = try? await FilesKeyService(store: store).state()
        } catch {
            // Next foreground tries again; the extension keeps what it had.
        }
    }

    /// Before Clerk signs out (the revoke needs the session).
    func willSignOut() async {
        guard let keyId = store.tokenKeyId() else { return }
        try? await ACIComputersClient.shared.revokeOperatorKey(id: keyId)
    }

    /// After sign-out: nothing of the account stays in Files.
    func clearLocal() {
        let store = store
        store.clearAccount()
        NotificationCenter.default.post(name: .filesDriveDidSync, object: nil)
        Task { await FilesDomains.removeAll() }
    }

    // MARK: Internals

    /// Runtime devices that are phones (this one included), not computers.
    private static let phoneRuntimeTypes: Set<String> = ["ios", "android"]

    private func turnOff() async {
        await willSignOut()
        store.clearAccount()
        NotificationCenter.default.post(name: .filesDriveDidSync, object: nil)
        await FilesDomains.removeAll()
    }

    private func ensureToken() async throws {
        if store.token() != nil && !store.tokenRejected { return }
        if let old = store.tokenKeyId() { try? await ACIComputersClient.shared.revokeOperatorKey(id: old) }
        let key = try await ACIComputersClient.shared.mintOperatorKey(name: "Files on \(store.deviceName)", canRunModels: false)
        guard let token = key.token, !token.isEmpty else {
            throw ACIComputersClient.Failure.server("The Files key response had no token.")
        }
        store.setToken(token, keyId: key.id)
    }

    /// `files_drive` from `GET /me/flags`; nil when it can't be read.
    private func readFlag() async -> Bool? {
        #if DEBUG
        if launchArgumentEnabled("files-drive") { return true }
        #endif
        do {
            let client = APIClient.shared
            let request = try await client.authorizedRequest(url: AppConfig.cloudAPIBaseURL.appendingPathComponent("api/v1/me/flags"))
            let (data, response) = try await client.session.data(for: request)
            try client.validate(response, data: data)
            let object = try JSONSerialization.jsonObject(with: data) as? [String: Any]
            return ((object?["flags"] as? [String: Any])?["files_drive"] as? Bool) ?? false
        } catch {
            return nil
        }
    }

    /// `dev_ios_<vendor id>`, the contract's `DeviceId` (`^dev_[A-Za-z0-9_-]{4,64}$`).
    private static func makeDeviceId() -> String {
        let vendor = UIDevice.current.identifierForVendor?.uuidString ?? UUID().uuidString
        return "dev_ios_" + vendor.replacingOccurrences(of: "-", with: "").lowercased()
    }
}
