import FileProvider
import Foundation
import UniformTypeIdentifiers
import AllternitFilesKit

/// Files drive File Provider extension (plan step 19): one replicated
/// domain per computer, so each computer is its own location in Files.
///
/// RELAY ONLY. Spike 5 showed iOS kills a File Provider extension at
/// 20,481 KB and tsnet alone takes ~11 MB, so this process never starts a
/// mesh node. Every request is a cloud-api relay request (`FilesClient`
/// with `allowedTransports: [.relay]`); downloads stream to disk in 32 MiB
/// ranged segments and uploads stream from disk in 4 MiB chunks, so memory
/// stays flat whatever the file size. Bulk bytes use a background
/// `URLSession`, so transfers continue while the extension is suspended.
///
/// Asleep computers: listing and stat are answered from the asleep snapshot
/// (no wake, no bill). Opening a file needs the computer awake: with the
/// computer's wake setting `auto` the client asks for a wake ticket by
/// itself; with `ask` the open fails with `.notAuthenticated`, Files offers
/// the wake sheet (AllternitFileProviderUI, plan step 20), and the ticket it
/// gets is shared back through `FilesSharedStore`.
///
/// Protected roots (Memory Drive) answer `protected_root_locked` until the
/// user unlocks with Face ID in the same sheet; requests then carry
/// `X-Allternit-Unlock` for five minutes.
///
/// Conflicts are never resolved by overwriting: every write carries the
/// `base_rev` the system last saw, and a stale one lands as a conflict copy
/// (SPEC section 4).
final class FileProviderExtension: NSObject, NSFileProviderReplicatedExtension {
    /// Shared with the thumbnail provider (AllternitThumbnails/).
    let context: ProviderContext
    private var watcher: Task<Void, Never>?

    /// While the process is alive, how often to look for changes made on
    /// the computer (one `GET /roots` through the relay; it never wakes it).
    private static let watchInterval: UInt64 = 30

    required init(domain: NSFileProviderDomain) {
        context = ProviderContext(domain: domain)
        super.init()
        startWatching()
    }

    func invalidate() {
        watcher?.cancel()
    }

    // MARK: Items

    func item(for identifier: NSFileProviderItemIdentifier, request: NSFileProviderRequest,
              completionHandler: @escaping (NSFileProviderItem?, Error?) -> Void) -> Progress {
        let context = context
        let task = Task {
            do {
                completionHandler(try await Self.item(identifier, context: context), nil)
            } catch {
                completionHandler(nil, context.error(error))
            }
        }
        return Self.progress(cancelling: task)
    }

    private static func item(_ identifier: NSFileProviderItemIdentifier, context: ProviderContext) async throws -> FileProviderItem {
        let mapper = try await context.mapper()
        if identifier == .rootContainer { return mapper.rootContainer() }
        if identifier == .trashContainer || identifier == .workingSet { throw NSFileProviderError(.noSuchItem) }
        if let root = try await context.catalog.root(forRootItem: identifier.rawValue) {
            return mapper.rootFolder(root)
        }
        let stat = try await context.client.stat(identifier.rawValue)
        await context.catalog.remember([stat.item])
        return try await context.mapper(roots: context.catalog.load().roots, freshness: stat.freshness).map(stat.item)
    }

    // MARK: Contents

    func fetchContents(for itemIdentifier: NSFileProviderItemIdentifier, version requestedVersion: NSFileProviderItemVersion?,
                       request: NSFileProviderRequest,
                       completionHandler: @escaping (URL?, NSFileProviderItem?, Error?) -> Void) -> Progress {
        let context = context
        let progress = Progress(totalUnitCount: 100)
        let task = Task {
            do {
                let (url, item) = try await Self.fetch(itemIdentifier.rawValue, context: context, progress: progress)
                completionHandler(url, item, nil)
            } catch {
                let root = await context.catalog.knownRoot(of: itemIdentifier.rawValue)
                completionHandler(nil, nil, context.error(error, root: root))
            }
        }
        progress.cancellationHandler = { task.cancel() }
        return progress
    }

    /// Stats the item, then streams its current content to a file in the
    /// domain's temporary directory (same volume, so the system moves it in
    /// without a copy). If the file changes on the computer mid-download,
    /// it starts over once from the new revision.
    private static func fetch(_ itemId: ItemId, context: ProviderContext, progress: Progress) async throws -> (URL, FileProviderItem) {
        let directory = (try? context.manager?.temporaryDirectoryURL()) ?? FileManager.default.temporaryDirectory
        var attempt = 0
        while true {
            attempt += 1
            let stat = try await context.client.stat(itemId)
            let item = stat.item
            guard item.kind == .file else { throw NSFileProviderError(.noSuchItem) }
            progress.totalUnitCount = Int64(max(item.size, 1))
            let destination = directory.appendingPathComponent(UUID().uuidString, isDirectory: false)
            do {
                // rootId lets an asleep computer's file come from a replica
                // (sealed, opened here with this device's Files key) instead
                // of a wake.
                try await context.client.download(itemId: item.id, rev: item.rev, size: item.size,
                                                  contentHash: item.contentHash, rootId: item.rootId, to: destination,
                                                  wake: .whenAutomatic) { done, _ in
                    progress.completedUnitCount = Int64(done)
                }
            } catch FilesError.revisionChanged where attempt < 2 {
                continue
            }
            await context.catalog.remember([item])
            let mapper = try await context.mapper(roots: context.catalog.load().roots, freshness: stat.freshness)
            return (destination, mapper.map(item))
        }
    }

    // MARK: Create

    func createItem(basedOn itemTemplate: NSFileProviderItem, fields: NSFileProviderItemFields, contents url: URL?,
                    options: NSFileProviderCreateItemOptions = [], request: NSFileProviderRequest,
                    completionHandler: @escaping (NSFileProviderItem?, NSFileProviderItemFields, Bool, Error?) -> Void) -> Progress {
        let context = context
        let progress = Progress(totalUnitCount: 100)
        let task = Task {
            do {
                let item = try await Self.create(itemTemplate, contents: url, mayAlreadyExist: options.contains(.mayAlreadyExist),
                                                 context: context, progress: progress)
                completionHandler(item, [], false, nil)
            } catch {
                completionHandler(nil, [], false, context.error(error))
            }
        }
        progress.cancellationHandler = { task.cancel() }
        return progress
    }

    private static func create(_ template: NSFileProviderItem, contents url: URL?, mayAlreadyExist: Bool,
                               context: ProviderContext, progress: Progress) async throws -> FileProviderItem {
        let parent = template.parentItemIdentifier
        guard parent != .rootContainer, parent != .trashContainer else {
            // The computer's top level holds only its roots.
            throw NSError(domain: NSCocoaErrorDomain, code: NSFileWriteNoPermissionError)
        }
        let type = template.contentType ?? .data
        if type.conforms(to: .symbolicLink) || type.conforms(to: .aliasFile) {
            throw NSError(domain: NSCocoaErrorDomain, code: NSFeatureUnsupportedError)
        }
        let parentId = parent.rawValue
        let rootId = try await context.catalog.rootId(for: parentId)
        let isFolder = type.conforms(to: .directory)

        // A re-import (the system lost track of an item it already sent)
        // must not make a "(2)" copy: reuse the item with that name.
        let existing = mayAlreadyExist ? try await findChild(named: template.filename, kind: isFolder ? .folder : .file,
                                                              rootId: rootId, parentId: parentId, context: context) : nil

        let created: Item
        if isFolder {
            // Folders, and packages (`.pages`): the system creates the
            // package's files as children afterwards.
            if let existing {
                created = existing
            } else {
                created = try await context.client.mkdir(parentId: parentId, name: template.filename, onConflict: .keepBoth).item
            }
        } else {
            let source = try url ?? emptyFile(in: context)
            let target: FilesUploadTarget = existing.map {
                .replace(rootId: rootId, parentId: parentId, itemId: $0.id, name: $0.name, baseRev: $0.rev)
            } ?? .create(rootId: rootId, parentId: parentId, name: template.filename)
            let result = try await context.client.upload(fileAt: source, to: target, onConflict: .keepBoth,
                                                         modifiedAt: template.contentModificationDate ?? nil,
                                                         wake: .whenAutomatic) { done, total in
                progress.totalUnitCount = Int64(max(total, 1))
                progress.completedUnitCount = Int64(done)
            }
            if url == nil { try? FileManager.default.removeItem(at: source) }
            created = result.item
        }
        await context.catalog.remember([created])
        return try await context.mapper().map(created)
    }

    private static func findChild(named name: String, kind: ItemKind, rootId: RootId, parentId: ItemId,
                                  context: ProviderContext) async throws -> Item? {
        var token: PageToken?
        for _ in 0..<20 {
            let list = try await context.client.list(rootId: rootId, parentId: parentId, pageToken: token, limit: FolderEnumerator.pageLimit)
            if let match = list.items.first(where: { $0.name == name && $0.kind == kind }) { return match }
            guard let next = list.nextPageToken else { return nil }
            token = next
        }
        return nil
    }

    private static func emptyFile(in context: ProviderContext) throws -> URL {
        let url = context.store.scratchDirectory(for: context.computerId).appendingPathComponent("empty-\(UUID().uuidString)")
        guard FileManager.default.createFile(atPath: url.path, contents: Data()) else {
            throw NSError(domain: NSCocoaErrorDomain, code: NSFileWriteUnknownError)
        }
        return url
    }

    // MARK: Modify

    func modifyItem(_ item: NSFileProviderItem, baseVersion version: NSFileProviderItemVersion,
                    changedFields: NSFileProviderItemFields, contents newContents: URL?,
                    options: NSFileProviderModifyItemOptions = [], request: NSFileProviderRequest,
                    completionHandler: @escaping (NSFileProviderItem?, NSFileProviderItemFields, Bool, Error?) -> Void) -> Progress {
        let context = context
        let progress = Progress(totalUnitCount: 100)
        let task = Task {
            do {
                let (result, fetchAgain) = try await Self.modify(item, baseVersion: version, changedFields: changedFields,
                                                                 contents: newContents, context: context, progress: progress)
                completionHandler(result, [], fetchAgain, nil)
            } catch {
                completionHandler(nil, [], false, context.error(error))
            }
        }
        progress.cancellationHandler = { task.cancel() }
        return progress
    }

    /// Rename/move first, then new content against the rev the move
    /// returned. Metadata the computer does not keep (dates, tags, flags,
    /// extended attributes) is accepted and not stored.
    ///
    /// When the content upload lands as a conflict copy (someone changed
    /// the file on the computer since this device saw it), the original is
    /// returned with "fetch again": Files then shows the computer's version,
    /// and the edit made here appears beside it as
    /// `Name (conflict iPhone 2026-11-15 1042).ext`.
    private static func modify(_ template: NSFileProviderItem, baseVersion: NSFileProviderItemVersion,
                               changedFields: NSFileProviderItemFields, contents: URL?,
                               context: ProviderContext, progress: Progress) async throws -> (FileProviderItem, Bool) {
        let itemId = template.itemIdentifier.rawValue
        let isRootFolder = try await isRootOrComputer(template.itemIdentifier, context: context)
        let renameOrMove = changedFields.contains(.filename) || changedFields.contains(.parentItemIdentifier)
        if isRootFolder {
            // Roots are set on the computer; Files can't rename or move them.
            if renameOrMove || changedFields.contains(.contents) {
                throw NSError(domain: NSCocoaErrorDomain, code: NSFileWriteNoPermissionError)
            }
            return (try await Self.item(template.itemIdentifier, context: context), false)
        }

        var current: Item?
        var baseRev = FileProviderItem.baseRev(baseVersion)
        if renameOrMove {
            let newParent = changedFields.contains(.parentItemIdentifier) ? template.parentItemIdentifier : nil
            if newParent == .rootContainer || newParent == .trashContainer {
                throw NSError(domain: NSCocoaErrorDomain, code: NSFileWriteNoPermissionError)
            }
            let newName = changedFields.contains(.filename) ? template.filename : nil
            current = try await move(itemId, baseRev: baseRev, newParentId: newParent?.rawValue, newName: newName, context: context)
            baseRev = current?.rev
        }

        var fetchAgain = false
        if changedFields.contains(.contents), let contents {
            let known: Item
            if let current { known = current } else { known = try await context.client.stat(itemId).item }
            let rev = baseRev ?? known.rev
            let result = try await context.client.upload(
                fileAt: contents,
                to: .replace(rootId: known.rootId, parentId: known.parentId ?? template.parentItemIdentifier.rawValue,
                             itemId: itemId, name: known.name, baseRev: rev),
                onConflict: .keepBoth,
                modifiedAt: template.contentModificationDate ?? nil,
                wake: .whenAutomatic
            ) { done, total in
                progress.totalUnitCount = Int64(max(total, 1))
                progress.completedUnitCount = Int64(done)
            }
            if result.conflict != nil {
                current = try await context.client.stat(itemId).item
                fetchAgain = true
                context.signalWorkingSet()
            } else {
                current = result.item
            }
        }

        let final: Item
        if let current { final = current } else { final = try await context.client.stat(itemId).item }
        await context.catalog.remember([final])
        return (try await context.mapper().map(final), fetchAgain)
    }

    /// A rename or move never loses data, so a stale `base_rev` (the file
    /// changed on the computer) is retried once against the current rev.
    private static func move(_ itemId: ItemId, baseRev: Rev?, newParentId: ItemId?, newName: String?,
                             context: ProviderContext) async throws -> Item {
        var rev: Rev
        if let baseRev { rev = baseRev } else { rev = try await context.client.stat(itemId).item.rev }
        for attempt in 1...2 {
            do {
                return try await context.client.move(MoveRequest(itemId: itemId, baseRev: rev, newParentId: newParentId,
                                                                 newName: newName, onConflict: .keepBoth)).item
            } catch let error as FilesError where error.code == .revMismatch && attempt == 1 {
                if let fresh = error.body?.current?.rev { rev = fresh } else { rev = try await context.client.stat(itemId).item.rev }
            }
        }
        throw NSFileProviderError(.cannotSynchronize)
    }

    // MARK: Delete

    func deleteItem(identifier: NSFileProviderItemIdentifier, baseVersion version: NSFileProviderItemVersion,
                    options: NSFileProviderDeleteItemOptions = [], request: NSFileProviderRequest,
                    completionHandler: @escaping (Error?) -> Void) -> Progress {
        let context = context
        let task = Task {
            do {
                if try await Self.isRootOrComputer(identifier, context: context) {
                    throw NSFileProviderError(.deletionRejected)
                }
                // To the computer's trash, never a permanent delete from Files.
                _ = try await context.client.delete(DeleteRequest(itemId: identifier.rawValue,
                                                                  baseRev: FileProviderItem.baseRev(version),
                                                                  permanent: false))
                completionHandler(nil)
            } catch let error as FilesError where error.code == .notFound {
                completionHandler(nil)
            } catch let error as FilesError where error.code == .revMismatch {
                // Changed on the computer since this device saw it: keep it,
                // and the system brings back the newer version.
                completionHandler(NSFileProviderError(.deletionRejected))
            } catch {
                completionHandler(context.error(error))
            }
        }
        return Self.progress(cancelling: task)
    }

    // MARK: Enumeration

    func enumerator(for containerItemIdentifier: NSFileProviderItemIdentifier,
                    request: NSFileProviderRequest) throws -> NSFileProviderEnumerator {
        switch containerItemIdentifier {
        case .workingSet: return WorkingSetEnumerator(context: context)
        case .trashContainer: return EmptyEnumerator()
        default: return FolderEnumerator(container: containerItemIdentifier, context: context)
        }
    }

    // MARK: Watching

    /// While the system keeps this process alive, check every 30 s whether
    /// any root's cursor moved since the anchor last handed out, and if so
    /// ask the system to enumerate changes. The app also signals on every
    /// foreground (`FilesDriveBridge`).
    private func startWatching() {
        let context = context
        watcher = Task.detached(priority: .utility) {
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: Self.watchInterval * 1_000_000_000)
                guard !Task.isCancelled, let anchor = context.anchor,
                      let (roots, _) = try? await context.catalog.load(refresh: true) else { continue }
                // A cursor moved on the computer, a re-list is still queued,
                // or a pin changed on this device.
                if FilesSyncAnchor(roots: roots).roots != anchor.roots || anchor.hasPendingRelist
                    || Set(anchor.pinned ?? []) != context.pinnedIds {
                    context.signalWorkingSet()
                }
            }
        }
    }

    /// The computer itself or one of its root folders (set on the computer,
    /// not changeable from Files).
    private static func isRootOrComputer(_ identifier: NSFileProviderItemIdentifier, context: ProviderContext) async throws -> Bool {
        if identifier == .rootContainer { return true }
        return try await context.catalog.root(forRootItem: identifier.rawValue) != nil
    }

    private static func progress(cancelling task: Task<Void, Never>) -> Progress {
        let progress = Progress(totalUnitCount: 1)
        progress.cancellationHandler = { task.cancel() }
        return progress
    }
}
