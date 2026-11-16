import Foundation

/// One item the system keeps on this device for a File Provider domain
/// (from `NSFileProviderManager.enumeratorForMaterializedItems()`): only
/// what a re-list needs, so a big materialized set stays small in memory.
public struct FilesMaterializedEntry: Sendable, Hashable {
    public var id: ItemId
    public var parentId: ItemId
    public var isFolder: Bool

    public init(id: ItemId, parentId: ItemId, isFolder: Bool) {
        self.id = id
        self.parentId = parentId
        self.isFolder = isFolder
    }
}

/// What to do when a root's change cursor is reset (SPEC section 6,
/// `reset_required`): the changes made while the cursor was stale are lost,
/// so every folder the device holds under that root is listed again, and
/// whatever it holds that a fresh listing no longer has is reported deleted.
///
/// Listing only the root's top folder (the step 19 behaviour) missed edits
/// and deletions in every folder below it until the user happened to open
/// that folder again.
public enum FilesResync {
    /// The folders to list again after `rootItemId`'s cursor reset: the root
    /// folder first, then every materialized folder below it, parents before
    /// children. It stops at `otherRoots` (sub-roots such as Inbox under
    /// Allternit have their own cursor and are re-listed only when theirs
    /// resets).
    public static func containers(below rootItemId: ItemId, materialized: [FilesMaterializedEntry],
                                  otherRoots: Set<ItemId>) -> [ItemId] {
        var children: [ItemId: [ItemId]] = [:]
        for entry in materialized where entry.isFolder {
            children[entry.parentId, default: []].append(entry.id)
        }
        var order: [ItemId] = [rootItemId]
        var seen: Set<ItemId> = [rootItemId]
        var index = 0
        while index < order.count {
            let folder = order[index]
            index += 1
            for child in children[folder] ?? [] where !otherRoots.contains(child) && seen.insert(child).inserted {
                order.append(child)
            }
        }
        return order
    }

    /// What a fresh listing of `container` says is gone: the materialized
    /// children of `container` that are not in `fresh`. Root folders are
    /// never reported (they come and go with `GET /roots`).
    public static func gone(in container: ItemId, fresh: Set<ItemId>, materialized: [FilesMaterializedEntry],
                            roots: Set<ItemId>) -> [ItemId] {
        materialized
            .filter { $0.parentId == container && !fresh.contains($0.id) && !roots.contains($0.id) }
            .map(\.id)
    }
}
