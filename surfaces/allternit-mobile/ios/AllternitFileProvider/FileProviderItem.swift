import FileProvider
import UniformTypeIdentifiers
import AllternitFilesKit

/// A `files/2` item as the File Provider sees it.
///
/// Identifiers are the contract's `ItemId` as is. The hierarchy:
/// - the domain's root container = the computer; its children are the
///   top-level roots (`parent_root_id == nil`: Allternit, Code, Models, …);
/// - a root folder is exposed under its `root_item_id`;
/// - a link entry inside ~/Allternit (`root_link`, e.g. Inbox) is exposed as
///   the linked root's folder, so that root's children line up under it.
///
/// Versions: `contentVersion` = the BLAKE3 content hash (or the rev for
/// folders), `metadataVersion` = the rev. The rev is what writes send as
/// `base_rev`, read back from `metadataVersion` in modify and delete.
final class FileProviderItem: NSObject, NSFileProviderItem {
    let itemIdentifier: NSFileProviderItemIdentifier
    let parentItemIdentifier: NSFileProviderItemIdentifier
    let filename: String
    let contentType: UTType
    let itemVersion: NSFileProviderItemVersion
    let capabilities: NSFileProviderItemCapabilities
    let documentSize: NSNumber?
    let creationDate: Date?
    let contentModificationDate: Date?
    /// Kept offline on this device ("Keep offline", `FilesSharedStore.pins`).
    let pinned: Bool
    /// The contract item behind this one (nil for the computer itself).
    let item: Item?
    let rootId: RootId?
    /// Allternit's own states, most important first (`FilesBadge`). Shown as
    /// Finder decorations on macOS (plan step 23).
    var badges: [FilesBadge] = []

    init(identifier: NSFileProviderItemIdentifier, parent: NSFileProviderItemIdentifier, name: String,
         isFolder: Bool, item: Item?, rootId: RootId?, rev: Rev, contentVersion: String,
         capabilities: NSFileProviderItemCapabilities, pinned: Bool) {
        itemIdentifier = identifier
        parentItemIdentifier = parent
        filename = name
        let byExtension = UTType(filenameExtension: (name as NSString).pathExtension)
        if isFolder {
            // A package (a `.pages` or `.key` document saved as a folder) is
            // a folder on the computer but one document in Files.
            contentType = byExtension.flatMap { $0.conforms(to: .package) ? $0 : nil } ?? .folder
        } else {
            contentType = byExtension ?? .data
        }
        itemVersion = NSFileProviderItemVersion(contentVersion: Data(contentVersion.utf8), metadataVersion: Data(rev.utf8))
        self.capabilities = capabilities
        documentSize = isFolder ? nil : NSNumber(value: item?.size ?? 0)
        creationDate = item.flatMap { ProviderDates.parse($0.createdAt) }
        contentModificationDate = item.flatMap { ProviderDates.parse($0.modifiedAt) }
        self.pinned = pinned
        self.item = item
        self.rootId = rootId
    }

    #if os(macOS)
    /// macOS keeps pinned files downloaded by policy (plan step 23). iOS has
    /// no such policy; the app re-reads pinned files (`FilesPinKeeper`).
    var contentPolicy: NSFileProviderContentPolicy { pinned ? .downloadEagerlyAndKeepDownloaded : .inherited }
    #endif

    /// The rev a write must send as `base_rev`, from a version the system kept.
    static func baseRev(_ version: NSFileProviderItemVersion) -> Rev? {
        let rev = String(decoding: version.metadataVersion, as: UTF8.self)
        return rev.isEmpty ? nil : rev
    }
}

/// Maps contract items to `FileProviderItem`s for one computer.
struct ItemMapper {
    let computerName: String
    let roots: [Root]
    let freshness: Freshness?
    let pinnedIds: Set<ItemId>

    private var rootsById: [RootId: Root] { Dictionary(roots.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a }) }

    /// Content is read only from the live computer: from a snapshot (asleep)
    /// listing works and opening a file asks for a wake.
    ///
    /// Saving still works while it sleeps (plan step 15, queued offline
    /// writes): new content and new files are accepted, the upload fails with
    /// `.serverUnreachable` (`FileProviderErrors.map`), and the system keeps
    /// the edit on this device as a pending upload, retried when the computer
    /// is on (`FilesDomains.signalChanges`). It never wakes the computer. The
    /// edit carries the rev it was made against, so a file that changed there
    /// meanwhile is kept twice. Rename, move and delete stay live only: a
    /// stale rev has no keep-both form for them (SPEC section 4).
    private var isLive: Bool { freshness == nil || freshness?.servedBy == .live }

    /// The computer itself.
    func rootContainer() -> FileProviderItem {
        FileProviderItem(identifier: .rootContainer, parent: .rootContainer, name: computerName, isFolder: true,
                         item: nil, rootId: nil, rev: "computer", contentVersion: "computer",
                         capabilities: [.allowsReading, .allowsContentEnumerating], pinned: false)
    }

    /// Top-level roots, the children of the root container.
    func topLevelRoots() -> [FileProviderItem] {
        roots.filter { $0.parentRootId == nil }.map(rootFolder)
    }

    /// The folder for `root`, under its parent root (or the computer).
    func rootFolder(_ root: Root) -> FileProviderItem {
        let parent: NSFileProviderItemIdentifier
        if let parentRoot = root.parentRootId.flatMap({ rootsById[$0] }) {
            parent = NSFileProviderItemIdentifier(parentRoot.rootItemId)
        } else {
            parent = .rootContainer
        }
        var caps: NSFileProviderItemCapabilities = [.allowsReading, .allowsContentEnumerating]
        // Asleep too: new items queue on this device (step 15).
        if root.policy.write { caps.insert(.allowsAddingSubItems) }
        let folder = FileProviderItem(identifier: NSFileProviderItemIdentifier(root.rootItemId), parent: parent, name: root.name,
                                      isFolder: true, item: nil, rootId: root.id, rev: "root-\(root.cursor)",
                                      contentVersion: "root", capabilities: caps, pinned: false)
        folder.badges = FilesBadge.badges(for: nil, root: root, pinned: false)
        return folder
    }

    /// `item` as the File Provider sees it (a link entry becomes its root).
    func map(_ item: Item) -> FileProviderItem {
        if let link = item.rootLink, let linked = rootsById[link] {
            return rootFolder(linked)
        }
        if item.parentId == nil, let root = rootsById[item.rootId] {
            return rootFolder(root)
        }
        let parent = item.parentId.map { NSFileProviderItemIdentifier($0) } ?? .rootContainer
        let isFolder = item.kind == .folder
        let pinned = pinnedIds.contains(item.id)
        let mapped = FileProviderItem(identifier: NSFileProviderItemIdentifier(item.id), parent: parent, name: item.name,
                                      isFolder: isFolder, item: item, rootId: item.rootId, rev: item.rev,
                                      contentVersion: item.contentHash ?? item.rev,
                                      capabilities: capabilities(item), pinned: pinned)
        mapped.badges = FilesBadge.badges(for: item, root: rootsById[item.rootId], pinned: pinned)
        return mapped
    }

    private func capabilities(_ item: Item) -> NSFileProviderItemCapabilities {
        let p = item.permissions
        var caps: NSFileProviderItemCapabilities = []
        if item.kind == .folder {
            if p.list { caps.insert(.allowsContentEnumerating) }
            if p.read { caps.insert(.allowsReading) }
        } else if p.read {
            caps.insert(.allowsReading)
        }
        guard isLive else {
            // Asleep: saves queue on this device (see isLive).
            if p.write { caps.insert(item.kind == .folder ? .allowsAddingSubItems : .allowsWriting) }
            return caps
        }
        if p.write {
            caps.insert(.allowsRenaming)
            caps.insert(.allowsReparenting)
            caps.insert(item.kind == .folder ? .allowsAddingSubItems : .allowsWriting)
        }
        if p.delete {
            caps.insert(.allowsDeleting)
            caps.insert(.allowsTrashing)
        }
        return caps
    }
}

enum ProviderDates {
    static func parse(_ value: String) -> Date? {
        let fractional = ISO8601DateFormatter()
        fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return fractional.date(from: value) ?? ISO8601DateFormatter().date(from: value)
    }
}

#if os(macOS)
/// Finder badges (design 6A). The identifiers are declared in the macOS
/// extension's Info.plist (`surfaces/allternit-desktop/native/file-provider/
/// Extension/Info.plist`, `NSFileProviderDecorations`) as
/// `<extension bundle id>.badge.<name>`. Step 20 can turn these on for iOS
/// by dropping the `#if` and declaring the same identifiers there.
extension FileProviderItem: NSFileProviderItemDecorating {
    var decorations: [NSFileProviderItemDecorationIdentifier]? {
        guard !badges.isEmpty, let bundle = Bundle.main.bundleIdentifier else { return nil }
        return badges.map { NSFileProviderItemDecorationIdentifier(rawValue: bundle + "." + $0.decorationSuffix) }
    }
}
#endif
