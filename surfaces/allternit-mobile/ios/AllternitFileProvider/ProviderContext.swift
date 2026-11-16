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
        shares = ShareCatalog(client: client)
    }

    /// Items with a live share link (the Shared badge).
    let shares: ShareCatalog

    var computerName: String { store.computer(computerId)?.name ?? fallbackName }

    /// Items kept offline on this device, for this computer.
    var pinnedIds: Set<ItemId> {
        let prefix = computerId + "/"
        return Set(store.pins.filter { $0.hasPrefix(prefix) }.map { String($0.dropFirst(prefix.count)) })
    }

    func mapper(roots: [Root], freshness: Freshness?) -> ItemMapper {
        ItemMapper(computerName: computerName, roots: roots, freshness: freshness, pinnedIds: pinnedIds,
                   sharedIds: shares.cachedIds)
    }

    /// A mapper from the cached roots (listing them first if needed).
    func mapper() async throws -> ItemMapper {
        let (roots, freshness) = try await catalog.load()
        kickShares()
        return mapper(roots: roots, freshness: freshness)
    }

    /// Refreshes the share list in the background; a listing never waits
    /// on it (the badge shows on the next listing).
    func kickShares() {
        let shares = shares
        Task.detached(priority: .utility) { await shares.refreshIfStale() }
    }

    /// The items the system keeps on this device for this computer
    /// (`enumeratorForMaterializedItems`), at most `limit` of them, so a
    /// cursor reset can re-list every folder the user has opened.
    func materializedEntries(limit: Int = 20_000) async -> [FilesMaterializedEntry] {
        guard let manager else { return [] }
        let enumerator = manager.enumeratorForMaterializedItems()
        defer { enumerator.invalidate() }
        var entries: [FilesMaterializedEntry] = []
        var page = NSFileProviderPage(Data())
        for _ in 0..<200 {
            let collector = MaterializedCollector()
            let (items, nextPage) = await collector.run(enumerator, page: page)
            for item in items where item.itemIdentifier != .rootContainer {
                let isFolder = item.contentType?.conforms(to: .directory) ?? false
                entries.append(FilesMaterializedEntry(id: item.itemIdentifier.rawValue,
                                                      parentId: item.parentItemIdentifier.rawValue, isFolder: isFolder))
                if entries.count >= limit { return entries }
            }
            guard let nextPage else { break }
            page = nextPage
        }
        return entries
    }

    var anchor: FilesSyncAnchor? {
        get { lock.withLock { lastAnchor } }
        set { lock.withLock { lastAnchor = newValue } }
    }

    /// `root` names the protected folder in the Face ID sheet when a
    /// listing or open fails with `protected_root_locked`.
    func error(_ error: Error, root: Root? = nil) -> Error {
        FileProviderErrors.map(error, computerId: computerId, computerName: computerName, store: store, root: root)
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
            noThumbnail.removeAll()
        }
        for item in items {
            rootOfItem[item.id] = item.rootId
            // `has_thumbnail: false` = the computer indexed it and made none.
            // Absent = unknown (older runtime), so the thumbnail is asked for.
            if item.hasThumbnail == false || item.kind == .folder { noThumbnail.insert(item.id) } else { noThumbnail.remove(item.id) }
        }
    }

    /// Items known to have no thumbnail, so Files' grid doesn't ask the
    /// relay for each of them.
    private var noThumbnail: Set<ItemId> = []

    func mayHaveThumbnail(_ itemId: ItemId) -> Bool {
        !noThumbnail.contains(itemId) && !roots.contains { $0.rootItemId == itemId }
    }

    /// The root `itemId` belongs to, when the catalog already knows it.
    func knownRoot(of itemId: ItemId) -> Root? {
        guard let rootId = rootOfItem[itemId] else { return nil }
        return roots.first { $0.id == rootId }
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

/// Collects one page of the materialized set.
private final class MaterializedCollector: NSObject, NSFileProviderEnumerationObserver, @unchecked Sendable {
    private var items: [NSFileProviderItem] = []
    private var continuation: CheckedContinuation<([NSFileProviderItem], NSFileProviderPage?), Never>?

    func run(_ enumerator: NSFileProviderEnumerator, page: NSFileProviderPage) async -> ([NSFileProviderItem], NSFileProviderPage?) {
        await withCheckedContinuation { continuation in
            self.continuation = continuation
            enumerator.enumerateItems(for: self, startingAt: page)
        }
    }

    func didEnumerate(_ updatedItems: [NSFileProviderItem]) {
        items += updatedItems
    }

    func finishEnumerating(upTo nextPage: NSFileProviderPage?) {
        continuation?.resume(returning: (items, nextPage))
        continuation = nil
    }

    func finishEnumeratingWithError(_ error: Error) {
        continuation?.resume(returning: (items, nil))
        continuation = nil
    }
}

/// Item ids with a live (not revoked, not expired) share link on this
/// computer, from cloud-api's share list. Refreshed at most every two
/// minutes; on any failure the last answer stays (no badge is better than
/// a wrong one, and listing never waits on it).
final class ShareCatalog: @unchecked Sendable {
    private let client: FilesClient
    private let lock = NSLock()
    private var ids: Set<ItemId> = []
    private var loadedAt: Date?
    private var loading = false

    private static let ttl: TimeInterval = 120

    init(client: FilesClient) {
        self.client = client
    }

    var cachedIds: Set<ItemId> { lock.withLock { ids } }

    func refreshIfStale() async {
        let start: Bool = lock.withLock {
            if loading { return false }
            if let loadedAt, Date().timeIntervalSince(loadedAt) < Self.ttl { return false }
            loading = true
            return true
        }
        guard start else { return }
        let fresh = try? await client.shares().shares
        let now = Date()
        lock.withLock {
            loading = false
            loadedAt = now
            if let fresh {
                ids = Set(fresh.filter { share in
                    share.revokedAt == nil && (share.expiresAt.flatMap(ProviderDates.parse).map { $0 > now } ?? true)
                }.map(\.itemId))
            }
        }
    }
}

/// `files/2` failures as the errors the File Provider framework expects, so
/// Files shows the right state (asleep, offline, sign in) instead of a
/// generic failure.
enum FileProviderErrors {
    static func map(_ error: Error, computerId: ComputerId, computerName: String, store: FilesSharedStore?,
                    root: Root? = nil) -> Error {
        if error is CancellationError { return NSError(domain: NSCocoaErrorDomain, code: NSUserCancelledError) }
        if error is NSFileProviderError { return error }
        if let urlError = error as? URLError {
            if urlError.code == .cancelled { return NSError(domain: NSCocoaErrorDomain, code: NSUserCancelledError) }
            return unreachable("\(computerName) isn't reachable right now. Check your connection.")
        }
        guard let files = error as? FilesError else { return error }
        if files.isAuthFailure {
            // The app mints a new Files token the next time it opens; the
            // sign-in sheet (File Provider UI) sends the user there.
            if case .signedOut = files {} else { store?.tokenRejected = true }
            return prompt(.signIn, "Open Allternit to reconnect \(computerName).")
        }
        switch files {
        case .wakeConsentRequired(let hint, _):
            if hint?.setting == .never {
                return unreachable("\(computerName) is asleep, and waking it from Files is turned off.")
            }
            // The client already left the pending wake (with its hint) in
            // the shared store; the wake sheet reads it.
            return prompt(.wake, "\(computerName) is asleep. Tap to wake it.")
        case .api(_, let body) where body.code == .protectedRootLocked:
            store?.addPendingUnlock(FilesPendingUnlock(computerId: computerId, rootId: root?.id, rootName: root?.name))
            return prompt(.unlock, "\(root?.name ?? "This folder") is protected. Unlock it with Face ID.")
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
            case .forbidden, .scopeViolation:
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

    /// `.notAuthenticated` is the one error that makes Files offer the
    /// File Provider UI sheet (`prepare(forError:)`).
    private static func prompt(_ kind: FilesPromptKind, _ message: String) -> Error {
        NSFileProviderError(.notAuthenticated, userInfo: [NSLocalizedDescriptionKey: message,
                                                         FilesPromptKind.userInfoKey: kind.rawValue])
    }

    private static func unreachable(_ message: String) -> Error {
        NSFileProviderError(.serverUnreachable, userInfo: [NSLocalizedDescriptionKey: message])
    }

    private static func cannotSync(_ message: String) -> Error {
        NSFileProviderError(.cannotSynchronize, userInfo: [NSLocalizedDescriptionKey: message])
    }
}
