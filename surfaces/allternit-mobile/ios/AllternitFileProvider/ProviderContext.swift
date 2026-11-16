import FileProvider
import Foundation
import AllternitFilesKit

/// What the extension and its enumerators share for one domain (one
/// computer): the relay-only client, the roots catalog, the shared store
/// and the last sync anchor handed to the system.
final class ProviderContext: @unchecked Sendable {
    let computerId: ComputerId
    let store: FilesSharedStore
    let client: FilesClient
    let catalog: RootCatalog
    let manager: NSFileProviderManager?
    private let fallbackName: String

    private let lock = NSLock()
    private var lastAnchor: FilesSyncAnchor?

    init(domain: NSFileProviderDomain) {
        store = FilesSharedStore.fromMainBundle()
        computerId = FilesDomains.computerId(for: domain.identifier) ?? domain.identifier.rawValue
        fallbackName = domain.displayName
        manager = NSFileProviderManager(for: domain)
        // Bulk bytes go through a background session, so a download or
        // upload keeps going while iOS suspends the extension. One session
        // id per computer: each domain runs its own extension process.
        let transfer = BackgroundFilesHTTP.shared(
            identifier: "com.allternit.mobile.files." + FilesDomains.safe(computerId),
            sharedContainerIdentifier: store.appGroup,
            downloadDirectory: store.scratchDirectory(for: computerId).appendingPathComponent("incoming", isDirectory: true)
        )
        client = store.makeClient(computerId: computerId, transferHTTP: transfer)
        catalog = RootCatalog(client: client)
    }

    var computerName: String { store.computer(computerId)?.name ?? fallbackName }

    /// Items kept offline on this device, for this computer.
    var pinnedIds: Set<ItemId> {
        let prefix = computerId + "/"
        return Set(store.pins.filter { $0.hasPrefix(prefix) }.map { String($0.dropFirst(prefix.count)) })
    }

    func mapper(roots: [Root], freshness: Freshness?) -> ItemMapper {
        ItemMapper(computerName: computerName, roots: roots, freshness: freshness, pinnedIds: pinnedIds)
    }

    /// A mapper from the cached roots (listing them first if needed).
    func mapper() async throws -> ItemMapper {
        let (roots, freshness) = try await catalog.load()
        return mapper(roots: roots, freshness: freshness)
    }

    var anchor: FilesSyncAnchor? {
        get { lock.withLock { lastAnchor } }
        set { lock.withLock { lastAnchor = newValue } }
    }

    func error(_ error: Error) -> Error {
        FileProviderErrors.map(error, computerName: computerName, store: store)
    }

    func signalWorkingSet() {
        manager?.signalEnumerator(for: .workingSet) { _ in }
    }
}

/// The computer's roots, and which root each item the extension has handed
/// out belongs to (`ItemId`s are opaque, and list and upload calls need the
/// root).
actor RootCatalog {
    private let client: FilesClient
    private var roots: [Root] = []
    private var freshness: Freshness?
    private var loadedAt: Date?
    private var rootOfItem: [ItemId: RootId] = [:]

    private static let ttl: TimeInterval = 60
    /// The item map is dropped past this size (ids are re-learned by stat).
    private static let maxRemembered = 20_000

    init(client: FilesClient) {
        self.client = client
    }

    /// The roots Files shows: readable ones whose provider is ready. While
    /// the computer sleeps this comes from the asleep snapshot.
    func load(refresh: Bool = false) async throws -> (roots: [Root], freshness: Freshness) {
        if !refresh, let loadedAt, let freshness, Date().timeIntervalSince(loadedAt) < Self.ttl {
            return (roots, freshness)
        }
        let response = try await client.roots()
        roots = response.roots.filter { $0.policy.read && $0.providerReady }
        freshness = response.freshness
        loadedAt = Date()
        for root in roots { rootOfItem[root.rootItemId] = root.id }
        return (roots, response.freshness)
    }

    func root(forRootItem itemId: ItemId) async throws -> Root? {
        try await load().roots.first { $0.rootItemId == itemId }
    }

    func remember(_ items: [Item]) {
        if rootOfItem.count + items.count > Self.maxRemembered {
            rootOfItem = Dictionary(roots.map { ($0.rootItemId, $0.id) }, uniquingKeysWith: { first, _ in first })
        }
        for item in items { rootOfItem[item.id] = item.rootId }
    }

    /// The root `itemId` lives in: from items already seen, else one stat.
    func rootId(for itemId: ItemId) async throws -> RootId {
        if let known = rootOfItem[itemId] { return known }
        _ = try await load()
        if let known = rootOfItem[itemId] { return known }
        let stat = try await client.stat(itemId)
        remember([stat.item])
        return stat.item.rootId
    }
}

/// `files/2` failures as the errors the File Provider framework expects, so
/// Files shows the right state (asleep, offline, sign in) instead of a
/// generic failure.
enum FileProviderErrors {
    static func map(_ error: Error, computerName: String, store: FilesSharedStore?) -> Error {
        if error is CancellationError { return NSError(domain: NSCocoaErrorDomain, code: NSUserCancelledError) }
        if error is NSFileProviderError { return error }
        if let urlError = error as? URLError {
            if urlError.code == .cancelled { return NSError(domain: NSCocoaErrorDomain, code: NSUserCancelledError) }
            return unreachable("\(computerName) isn't reachable right now. Check your connection.")
        }
        guard let files = error as? FilesError else { return error }
        if files.isAuthFailure {
            // The app mints a new Files token the next time it opens.
            if case .signedOut = files {} else { store?.tokenRejected = true }
            return NSFileProviderError(.notAuthenticated)
        }
        switch files {
        case .wakeConsentRequired(let hint, _):
            // Plan step 20 turns this into the wake-consent sheet (File
            // Provider UI, `.notAuthenticated` + the pending wake the client
            // recorded). Until then Files shows this text.
            if hint?.setting == .never {
                return unreachable("\(computerName) is asleep, and waking it from Files is turned off.")
            }
            return unreachable("\(computerName) is asleep. Open the Allternit app to wake it.")
        case .unreachable:
            return unreachable("\(computerName) isn't responding.")
        case .warming:
            return unreachable("\(computerName) is waking up. Try again in a moment.")
        case .noTransport:
            return unreachable("\(computerName) can only be reached from the Allternit app.")
        case .api(_, let body):
            switch body.code {
            case .notFound, .rootNotFound:
                return NSFileProviderError(.noSuchItem)
            case .alreadyExists:
                return NSFileProviderError(.filenameCollision)
            case .quotaExceeded:
                return NSFileProviderError(.insufficientQuota)
            case .forbidden, .scopeViolation, .protectedRootLocked:
                return NSError(domain: NSCocoaErrorDomain, code: NSFileWriteNoPermissionError,
                               userInfo: [NSLocalizedDescriptionKey: body.message])
            case .asleep, .waking, .offline:
                return unreachable("\(computerName) is asleep or offline.")
            case .wakeDenied:
                return unreachable("Waking \(computerName) is turned off.")
            case .featureDisabled:
                return unreachable("Files isn't turned on for this account.")
            default:
                return cannotSync(body.message)
            }
        default:
            return cannotSync(files.localizedDescription)
        }
    }

    private static func unreachable(_ message: String) -> Error {
        NSFileProviderError(.serverUnreachable, userInfo: [NSLocalizedDescriptionKey: message])
    }

    private static func cannotSync(_ message: String) -> Error {
        NSFileProviderError(.cannotSynchronize, userInfo: [NSLocalizedDescriptionKey: message])
    }
}
