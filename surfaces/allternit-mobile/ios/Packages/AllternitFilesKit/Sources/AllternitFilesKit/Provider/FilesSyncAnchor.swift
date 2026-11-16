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

    public var version: Int
    public var roots: [RootId: RootMark]

    public init(roots: [RootId: RootMark] = [:]) {
        version = 1
        self.roots = roots
    }

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
