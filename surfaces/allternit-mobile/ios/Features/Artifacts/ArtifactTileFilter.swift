import Foundation

/// Tile-tap filtering for the Artifacts library (phone calm step 28),
/// mirroring web `tileKind` / "New <Kind>" / "No <kind> yet" in `ArtifactsView.tsx`.
extension ArtifactStudioKind {
    /// The library kind this tile filters to; nil when no stored kind matches
    /// (Build, Deep Research), so the tile still creates.
    var filterKind: ArtifactV2Kind? {
        switch self {
        case .docs: return .doc
        case .sheets: return .sheet
        case .slides: return .slides
        case .design: return .design
        case .ops: return .ops
        case .dashboard: return .dashboard
        case .motion: return .motion
        case .page: return .page
        case .image: return .image
        case .video: return .video
        case .build, .research: return nil
        }
    }
}

enum ArtifactTileFilter {
    /// Tapping a tile: the kind to filter to (nil clears when it is already
    /// selected). `.noFilter` means the tile has no matching kind: create.
    enum Outcome: Equatable {
        case filter(ArtifactV2Kind)
        case clear
        case noFilter
    }

    static func tap(_ tile: ArtifactStudioKind, current: ArtifactV2Kind?) -> Outcome {
        guard let kind = tile.filterKind else { return .noFilter }
        return current == kind ? .clear : .filter(kind)
    }

    /// The tile that represents the active kind, if any.
    static func selectedTile(for kind: ArtifactV2Kind?, in tiles: [ArtifactStudioKind]) -> ArtifactStudioKind? {
        guard let kind else { return nil }
        return tiles.first { $0.filterKind == kind }
    }

    /// Launch value for `-calm-artifacts-filter=`: a tile id ("docs") or a kind ("doc").
    static func kind(fromArgument value: String) -> ArtifactV2Kind? {
        let v = value.lowercased()
        if let tile = ArtifactStudioKind(rawValue: v), let kind = tile.filterKind { return kind }
        return ArtifactV2Kind(rawValue: v)
    }

    /// "New Docs" (web `New {tile.label}`).
    static func newRowTitle(_ tile: ArtifactStudioKind) -> String { "New \(tile.label)" }

    /// "No docs yet" (web `No {label.toLowerCase()} yet`).
    static func emptyText(kind: ArtifactV2Kind, tile: ArtifactStudioKind?) -> String {
        "No \((tile?.label ?? kind.label).lowercased()) yet"
    }

    /// "Doc · Edited 2 hours ago".
    static func subtitle(kindLabel: String, edited: String) -> String {
        edited.isEmpty ? kindLabel : "\(kindLabel) · \(edited)"
    }
}
