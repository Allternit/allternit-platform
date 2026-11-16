import FileProvider
import Foundation
import AllternitFilesKit

/// Lists one folder: the computer (its top-level roots) or any folder in a
/// root, one `files/2` page (500 items) per File Provider page. While the
/// computer sleeps the listing comes from the asleep snapshot, and the
/// items carry no write capabilities (ItemMapper).
///
/// A replicated extension learns about changes through the working set
/// only, so this enumerator reports no changes of its own.
final class FolderEnumerator: NSObject, NSFileProviderEnumerator {
    private let container: NSFileProviderItemIdentifier
    private let context: ProviderContext
    private var task: Task<Void, Never>?

    static let pageLimit = 500

    init(container: NSFileProviderItemIdentifier, context: ProviderContext) {
        self.container = container
        self.context = context
    }

    func invalidate() {
        task?.cancel()
    }

    func enumerateItems(for observer: NSFileProviderEnumerationObserver, startingAt page: NSFileProviderPage) {
        let container = container
        let context = context
        task = Task {
            do {
                let (roots, rootsFreshness) = try await context.catalog.load()
                if container == .rootContainer {
                    observer.didEnumerate(context.mapper(roots: roots, freshness: rootsFreshness).topLevelRoots())
                    observer.finishEnumerating(upTo: nil)
                    return
                }
                let parentId = container.rawValue
                let rootId = try await context.catalog.rootId(for: parentId)
                let token = Self.pageToken(page)
                let list = try await context.client.list(rootId: rootId, parentId: parentId, pageToken: token, limit: Self.pageLimit)
                await context.catalog.remember(list.items)
                let mapper = context.mapper(roots: roots, freshness: list.freshness)
                var items = list.items.map(mapper.map)
                if token == nil, let root = roots.first(where: { $0.rootItemId == parentId }) {
                    // Sub-roots (Inbox, Shared, …) under this root, in case
                    // the listing has no link entry for one.
                    items += roots.filter { $0.parentRootId == root.id }.map(mapper.rootFolder)
                }
                observer.didEnumerate(Self.unique(items))
                observer.finishEnumerating(upTo: list.nextPageToken.map { NSFileProviderPage(Data($0.utf8)) })
            } catch {
                observer.finishEnumeratingWithError(context.error(error))
            }
        }
    }

    func enumerateChanges(for observer: NSFileProviderChangeObserver, from anchor: NSFileProviderSyncAnchor) {
        observer.finishEnumeratingChanges(upTo: anchor, moreComing: false)
    }

    /// nil for the system's initial pages, else the `page_token` we handed out.
    static func pageToken(_ page: NSFileProviderPage) -> PageToken? {
        if page == NSFileProviderPage.initialPageSortedByName as NSFileProviderPage
            || page == NSFileProviderPage.initialPageSortedByDate as NSFileProviderPage {
            return nil
        }
        let token = String(decoding: page.rawValue, as: UTF8.self)
        return token.isEmpty ? nil : token
    }

    static func unique(_ items: [FileProviderItem]) -> [FileProviderItem] {
        var seen = Set<NSFileProviderItemIdentifier>()
        return items.filter { seen.insert($0.itemIdentifier).inserted }
    }
}

/// The working set: how the system learns what changed on the computer.
///
/// - Items: every root folder (the computer's top level and the sub-roots).
///   Folders below them are listed when the user opens them.
/// - Sync anchor: every root's change cursor (`FilesSyncAnchor`).
/// - Changes: one `GET /roots/:id/changes` per root (no long poll), mapped
///   to updates and deletions. A root that appeared is reported as a new
///   folder; a root that went away, as deleted.
/// - Asleep or offline: no changes and the same anchor, so nothing is lost
///   and nothing wakes the computer. The snapshot answers while it sleeps.
/// - A cursor the server reset: that root's top folder is listed again and
///   reported as updates. Deletions under it that happened while the cursor
///   was stale show up the next time a folder is listed.
final class WorkingSetEnumerator: NSObject, NSFileProviderEnumerator {
    private let context: ProviderContext
    private var task: Task<Void, Never>?

    init(context: ProviderContext) {
        self.context = context
    }

    func invalidate() {
        task?.cancel()
    }

    func enumerateItems(for observer: NSFileProviderEnumerationObserver, startingAt page: NSFileProviderPage) {
        let context = context
        task = Task {
            do {
                let (roots, freshness) = try await context.catalog.load()
                let mapper = context.mapper(roots: roots, freshness: freshness)
                observer.didEnumerate(roots.map(mapper.rootFolder))
                observer.finishEnumerating(upTo: nil)
            } catch {
                observer.finishEnumeratingWithError(context.error(error))
            }
        }
    }

    func currentSyncAnchor(completionHandler: @escaping (NSFileProviderSyncAnchor?) -> Void) {
        let context = context
        task = Task {
            if let (roots, _) = try? await context.catalog.load(refresh: true) {
                let anchor = FilesSyncAnchor(roots: roots)
                context.anchor = anchor
                completionHandler(NSFileProviderSyncAnchor(anchor.data))
            } else {
                completionHandler(context.anchor.map { NSFileProviderSyncAnchor($0.data) })
            }
        }
    }

    func enumerateChanges(for observer: NSFileProviderChangeObserver, from anchor: NSFileProviderSyncAnchor) {
        let context = context
        task = Task {
            guard let start = FilesSyncAnchor(data: anchor.rawValue) else {
                observer.finishEnumeratingWithError(NSFileProviderError(.syncAnchorExpired))
                return
            }
            do {
                let (roots, freshness) = try await context.catalog.load(refresh: true)
                var next = start
                var updated: [FileProviderItem] = []
                var deleted: [NSFileProviderItemIdentifier] = []
                var moreComing = false
                var mapper = context.mapper(roots: roots, freshness: freshness)

                for root in roots {
                    try Task.checkCancellation()
                    guard let mark = start.roots[root.id] else {
                        updated.append(mapper.rootFolder(root))
                        next.roots[root.id] = .init(cursor: root.cursor, rootItemId: root.rootItemId)
                        continue
                    }
                    switch try await context.client.changes(rootId: root.id, cursor: mark.cursor, limit: 500) {
                    case .changes(let response):
                        mapper = context.mapper(roots: roots, freshness: response.freshness)
                        let items = response.changes.compactMap(\.item)
                        await context.catalog.remember(items)
                        for change in response.changes {
                            switch change.op {
                            case .deleted, .trashed:
                                deleted.append(NSFileProviderItemIdentifier(change.itemId))
                            default:
                                if let item = change.item { updated.append(mapper.map(item)) }
                            }
                        }
                        next.roots[root.id] = .init(cursor: response.cursor, rootItemId: root.rootItemId)
                        if response.hasMore { moreComing = true }
                    case .reset:
                        let list = try await context.client.list(rootId: root.id, parentId: root.rootItemId,
                                                                 limit: FolderEnumerator.pageLimit)
                        await context.catalog.remember(list.items)
                        updated.append(mapper.rootFolder(root))
                        updated += list.items.map(context.mapper(roots: roots, freshness: list.freshness).map)
                        next.roots[root.id] = .init(cursor: list.cursor, rootItemId: root.rootItemId)
                    }
                }
                let live = Set(roots.map(\.id))
                for (rootId, mark) in start.roots where !live.contains(rootId) {
                    deleted.append(NSFileProviderItemIdentifier(mark.rootItemId))
                    next.roots[rootId] = nil
                }

                let deletedSet = Set(deleted)
                let updates = FolderEnumerator.unique(updated).filter { !deletedSet.contains($0.itemIdentifier) }
                if !updates.isEmpty { observer.didUpdate(updates) }
                if !deleted.isEmpty { observer.didDeleteItems(withIdentifiers: deleted) }
                context.anchor = next
                observer.finishEnumeratingChanges(upTo: NSFileProviderSyncAnchor(next.data), moreComing: moreComing)
            } catch let error as FilesError where error.isComputerUnavailable || error.code == .wakeConsentRequired {
                observer.finishEnumeratingChanges(upTo: anchor, moreComing: false)
            } catch is URLError {
                observer.finishEnumeratingChanges(upTo: anchor, moreComing: false)
            } catch {
                observer.finishEnumeratingWithError(context.error(error))
            }
        }
    }
}

/// The trash is the computer's own (`/roots/:id/trash`, plan step 21), not
/// synced into Files: an empty folder.
final class EmptyEnumerator: NSObject, NSFileProviderEnumerator {
    func invalidate() {}

    func enumerateItems(for observer: NSFileProviderEnumerationObserver, startingAt page: NSFileProviderPage) {
        observer.finishEnumerating(upTo: nil)
    }

    func enumerateChanges(for observer: NSFileProviderChangeObserver, from anchor: NSFileProviderSyncAnchor) {
        observer.finishEnumeratingChanges(upTo: anchor, moreComing: false)
    }
}
