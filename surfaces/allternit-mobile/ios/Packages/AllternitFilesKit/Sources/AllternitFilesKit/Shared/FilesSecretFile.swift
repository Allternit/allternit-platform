import Foundation

/// macOS secret storage for `FilesSharedStore` (plan step 23).
///
/// Allternit Desktop is signed with a Developer ID, not through the App
/// Store, so it has no provisioning profile and no shared keychain access
/// group (`keychain-access-groups` is a restricted entitlement on macOS).
/// The app's helper and the sandboxed File Provider extension do share a
/// team-prefixed App Group, whose container macOS keeps from other apps. The
/// Files token and wake tickets go in one JSON file there, mode 0600.
///
/// Several processes write it (the helper sets the token, the extension
/// stores wake tickets), so every read-modify-write holds an exclusive
/// `flock` on a lock file beside it and the file is replaced atomically.
struct FilesSecretFile {
    let url: URL

    init(directory: URL) {
        url = directory.appendingPathComponent("secrets.json", isDirectory: false)
    }

    func read(_ account: String) -> String? {
        withLock { load()[account] }.flatMap { $0.isEmpty ? nil : $0 }
    }

    @discardableResult
    func write(_ value: String, account: String) -> Bool {
        withLock {
            var all = load()
            all[account] = value
            return save(all)
        }
    }

    func delete(_ account: String) {
        withLock {
            var all = load()
            guard all.removeValue(forKey: account) != nil else { return }
            save(all)
        }
    }

    // MARK: Internals

    private func load() -> [String: String] {
        guard let data = try? Data(contentsOf: url) else { return [:] }
        return (try? JSONDecoder().decode([String: String].self, from: data)) ?? [:]
    }

    @discardableResult
    private func save(_ all: [String: String]) -> Bool {
        let directory = url.deletingLastPathComponent()
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true,
                                                 attributes: [.posixPermissions: 0o700])
        guard let data = try? JSONEncoder().encode(all) else { return false }
        // Written to a 0600 temp file first, so the secrets are never
        // readable by others, not even for a moment, then moved over.
        let temp = directory.appendingPathComponent(".secrets-\(UUID().uuidString).tmp")
        guard FileManager.default.createFile(atPath: temp.path, contents: data, attributes: [.posixPermissions: 0o600]) else {
            return false
        }
        if rename(temp.path, url.path) != 0 {
            try? FileManager.default.removeItem(at: temp)
            return false
        }
        return true
    }

    private func withLock<T>(_ body: () -> T) -> T {
        let lockPath = url.deletingLastPathComponent().appendingPathComponent(".secrets.lock").path
        try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true,
                                                 attributes: [.posixPermissions: 0o700])
        let fd = open(lockPath, O_RDWR | O_CREAT, 0o600)
        guard fd >= 0 else { return body() }
        defer { close(fd) }
        flock(fd, LOCK_EX)
        defer { flock(fd, LOCK_UN) }
        return body()
    }
}
