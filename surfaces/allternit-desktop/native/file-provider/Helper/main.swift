import FileProvider
import Foundation
import AllternitFilesKit

// allternit-files-domains: the Desktop app's side of the macOS File
// Provider (plan step 23). Electron main can't call FileProvider, so it runs
// this helper from `Contents/MacOS/` (inside the app bundle, so the domains
// it adds belong to Allternit Desktop's extension) and reads one JSON line
// back. See src/main/files-drive-manager.ts.
//
//   allternit-files-domains sync        stdin: SyncInput JSON
//   allternit-files-domains status
//   allternit-files-domains reenable <computer id>...
//   allternit-files-domains signal [<computer id>...]
//   allternit-files-domains url <computer id>
//   allternit-files-domains remove-all
//
// Every command prints a `Status` (or `{"url": ...}`) on stdout and exits 0,
// or prints `{"error": "..."}` and exits 1.

struct SyncInput: Decodable {
    struct Token: Decodable {
        let value: String
        let keyId: String?
    }

    let cloudBaseURL: String
    let deviceId: String
    let deviceName: String
    let computers: [FilesSharedComputer]
    /// Set only when main minted a new Files key; otherwise the stored one stays.
    let token: Token?
    /// `"<mesh ip>:<port>"` to `"127.0.0.1:<port>"`.
    let meshProxies: [String: String]
}

struct DomainStatus: Encodable {
    let computerId: String
    let name: String
    /// False until the user turns Allternit on under System Settings >
    /// General > Login Items & Extensions > File Providers.
    let userEnabled: Bool
}

struct Status: Encodable {
    let appGroup: String
    let containerReady: Bool
    let hasToken: Bool
    let tokenRejected: Bool
    let tokenKeyId: String?
    let domains: [DomainStatus]
    /// Computers that got a new domain in this call (`sync` only).
    var added: [String] = []
}

enum HelperError: Error, CustomStringConvertible {
    case usage(String)
    case noExtension
    case noDomain(String)

    var description: String {
        switch self {
        case .usage(let message): return message
        case .noExtension: return "The Allternit File Provider extension is not in this app bundle."
        case .noDomain(let id): return "No Files location for computer \(id)."
        }
    }
}

@main
struct FilesDomainsHelper {
    static func main() async {
        let args = Array(CommandLine.arguments.dropFirst())
        do {
            let store = try makeStore()
            let output: Data
            switch args.first {
            case "sync":
                output = try encode(try await sync(store: store, input: readInput()))
            case "status", nil:
                output = try encode(try await status(store: store))
            case "reenable":
                try await reenable(Array(args.dropFirst()), store: store)
                output = try encode(try await status(store: store))
            case "signal":
                let ids = args.count > 1 ? Array(args.dropFirst()) : store.computers.map(\.id)
                for id in ids { await FilesDomains.signalChanges(id) }
                output = try encode(try await status(store: store))
            case "url":
                guard args.count == 2 else { throw HelperError.usage("usage: url <computer id>") }
                output = try encode(["url": try await rootURL(args[1]).path])
            case "remove-all":
                store.clearAccount()
                await FilesDomains.removeAll()
                output = try encode(try await status(store: store))
            default:
                throw HelperError.usage("unknown command \(args[0])")
            }
            FileHandle.standardOutput.write(output)
            FileHandle.standardOutput.write(Data("\n".utf8))
            exit(0)
        } catch {
            let message = (error as? HelperError)?.description ?? "\(error)"
            let body = (try? JSONEncoder().encode(["error": message])) ?? Data("{\"error\":\"failed\"}".utf8)
            FileHandle.standardOutput.write(body)
            FileHandle.standardOutput.write(Data("\n".utf8))
            exit(1)
        }
    }

    /// The store the extension reads: the App Group comes from the
    /// extension's Info.plist (`AllternitAppGroup`, set by the build with the
    /// signing team's prefix), so app and extension can never disagree.
    static func makeStore() throws -> FilesSharedStore {
        if let group = ProcessInfo.processInfo.environment["ALLTERNIT_FILES_APP_GROUP"], !group.isEmpty {
            return FilesSharedStore(appGroup: group, keychainGroup: nil)
        }
        guard let plugIns = Bundle.main.builtInPlugInsURL else { throw HelperError.noExtension }
        let info = plugIns.appendingPathComponent("AllternitFileProvider.appex/Contents/Info.plist")
        guard let plist = NSDictionary(contentsOf: info), let group = plist["AllternitAppGroup"] as? String, !group.isEmpty else {
            throw HelperError.noExtension
        }
        return FilesSharedStore(appGroup: group, keychainGroup: nil)
    }

    static func readInput() throws -> SyncInput {
        let data = FileHandle.standardInput.readDataToEndOfFile()
        return try JSONDecoder().decode(SyncInput.self, from: data)
    }

    static func sync(store: FilesSharedStore, input: SyncInput) async throws -> Status {
        if let url = URL(string: input.cloudBaseURL) { store.cloudBaseURL = url }
        store.deviceId = input.deviceId
        store.deviceName = input.deviceName
        if let token = input.token { store.setToken(token.value, keyId: token.keyId) }
        store.meshProxies = input.meshProxies
        store.computers = input.computers
        let before = Set(try await NSFileProviderManager.domains().compactMap { FilesDomains.computerId(for: $0.identifier) })
        try await FilesDomains.sync(input.computers)
        var result = try await status(store: store)
        result.added = input.computers.map(\.id).filter { !before.contains($0) }
        return result
    }

    static func status(store: FilesSharedStore) async throws -> Status {
        let domains = try await NSFileProviderManager.domains().compactMap { domain -> DomainStatus? in
            guard let id = FilesDomains.computerId(for: domain.identifier) else { return nil }
            return DomainStatus(computerId: id, name: domain.displayName, userEnabled: domain.userEnabled)
        }
        return Status(appGroup: store.appGroup,
                      containerReady: store.containerURL != nil,
                      hasToken: store.token() != nil,
                      tokenRejected: store.tokenRejected,
                      tokenKeyId: store.tokenKeyId(),
                      domains: domains.sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending })
    }

    /// After the user turns the provider on, a domain added while it was off
    /// stays empty until it is added again (spike 6, macOS 26). It held no
    /// files while it was off, so removing it loses nothing.
    static func reenable(_ ids: [String], store: FilesSharedStore) async throws {
        let existing = try await NSFileProviderManager.domains()
        for id in ids {
            let identifier = FilesDomains.identifier(for: id)
            let name = store.computer(id)?.name ?? existing.first { $0.identifier == identifier }?.displayName ?? id
            if let domain = existing.first(where: { $0.identifier == identifier }) {
                try? await NSFileProviderManager.remove(domain)
            }
            try await NSFileProviderManager.add(NSFileProviderDomain(identifier: identifier, displayName: name))
            await FilesDomains.signalChanges(id)
        }
    }

    /// The folder Finder shows for a computer, for "Show in Finder".
    static func rootURL(_ id: String) async throws -> URL {
        guard let manager = await FilesDomains.manager(for: id) else { throw HelperError.noDomain(id) }
        return try await manager.getUserVisibleURL(for: .rootContainer)
    }

    static func encode<T: Encodable>(_ value: T) throws -> Data {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        return try encoder.encode(value)
    }
}
