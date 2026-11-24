import Foundation

/// The File Provider sync anchor for one computer: each root's change
/// cursor (SPEC section 6) plus the root's folder item, so a root that
/// disappears can be reported as deleted. Opaque to the system; encoded as
/// JSON. A root missing from the anchor is new since the anchor was made.
public struct FilesSyncAnchor: Codable, Sendable, Equatable {
    public struct RootMark: Codable, Sendable, Equatable {
        public var cursor: Cursor
        public var rootItemId: ItemId

        public init(cursor: Cursor, rootItemId: ItemId) {
            self.cursor = cursor
            self.rootItemId = rootItemId
        }
    }

    /// A folder still to be listed again after its root's cursor was reset
    /// (`FilesResync`): kept in the anchor, so the work survives the
    /// extension being stopped between passes.
    public struct Relist: Codable, Sendable, Equatable {
        public var rootId: RootId
        public var itemId: ItemId

        public init(rootId: RootId, itemId: ItemId) {
            self.rootId = rootId
            self.itemId = itemId
        }
    }

    public var version: Int
    public var roots: [RootId: RootMark]
    /// Folders still to re-list (nil or empty: none).
    public var relist: [Relist]?
    /// Items kept offline on this device when the anchor was made, sorted.
    /// A pin added or removed since is reported as an update, so its badge
    /// and actions change without a server change.
    public var pinned: [ItemId]?

    public init(roots: [RootId: RootMark] = [:], relist: [Relist]? = nil, pinned: [ItemId]? = nil) {
        version = 1
        self.roots = roots
        self.relist = relist
        self.pinned = pinned
    }

    /// True while folders are left to re-list after a cursor reset.
    public var hasPendingRelist: Bool { !(relist ?? []).isEmpty }

    /// The anchor for the roots as listed now.
    public init(roots: [Root]) {
        self.init(roots: Dictionary(roots.map { ($0.id, RootMark(cursor: $0.cursor, rootItemId: $0.rootItemId)) },
                                    uniquingKeysWith: { first, _ in first }))
    }

    /// nil for anything this version did not write (the caller answers
    /// `syncAnchorExpired`, and the system enumerates again).
    public init?(data: Data) {
        guard let decoded = try? JSONDecoder().decode(FilesSyncAnchor.self, from: data), decoded.version == 1 else { return nil }
        self = decoded
    }

    public var data: Data {
        let encoder = JSONEncoder()
        encoder.outputFormatting = .sortedKeys
        return (try? encoder.encode(self)) ?? Data()
    }
}
