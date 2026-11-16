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

    /// Content can be read and written only from the live computer. A
    /// snapshot (asleep) is metadata only: listing works, writing doesn't,
    /// and opening a file asks for a wake.
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
        if isLive && root.policy.write { caps.insert(.allowsAddingSubItems) }
        return FileProviderItem(identifier: NSFileProviderItemIdentifier(root.rootItemId), parent: parent, name: root.name,
                                isFolder: true, item: nil, rootId: root.id, rev: "root-\(root.cursor)",
                                contentVersion: "root", capabilities: caps, pinned: false)
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
        return FileProviderItem(identifier: NSFileProviderItemIdentifier(item.id), parent: parent, name: item.name,
                                isFolder: isFolder, item: item, rootId: item.rootId, rev: item.rev,
                                contentVersion: item.contentHash ?? item.rev,
                                capabilities: capabilities(item), pinned: pinnedIds.contains(item.id))
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
        guard isLive else { return caps }
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
