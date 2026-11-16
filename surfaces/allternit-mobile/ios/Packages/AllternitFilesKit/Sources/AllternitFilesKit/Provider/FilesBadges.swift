import Foundation

/// The Files badge set (design 6A, colour badges; the same set as the iOS
/// chips in 5C). The system already shows cloud-only, downloading,
/// uploading and up to date, so these are the states only Allternit knows.
///
/// A File Provider shows one icon badge per item, so `badges(...)` returns
/// them most important first. `shared` is not an icon badge: Finder shows
/// it as the "Sharing" text next to the icon.
public enum FilesBadge: String, CaseIterable, Sendable {
    /// A conflict copy (`Name (conflict <device> <YYYY-MM-DD HHmm>).ext`).
    case conflict
    /// In a protected root: opening it needs a fresh unlock.
    case protected
    /// Written by an agent (provenance `ai_generated`, or last modified by an agent).
    case agent
    /// Kept offline on this device.
    case pinned
    /// In the Shared root.
    case shared

    public var label: String {
        switch self {
        case .conflict: return "Conflict"
        case .protected: return "Protected"
        case .agent: return "Made by an agent"
        case .pinned: return "Kept on this device"
        case .shared: return "Shared"
        }
    }

    /// The decoration identifier suffix in the extension's Info.plist
    /// (`<extension bundle id>.<suffix>`).
    public var decorationSuffix: String { "badge.\(rawValue)" }

    /// Badges for `item` in `root`, most important first.
    public static func badges(for item: Item?, root: Root?, pinned: Bool) -> [FilesBadge] {
        var out: [FilesBadge] = []
        if let item, isConflictCopy(item.name) { out.append(.conflict) }
        if root?.policy.protected == true { out.append(.protected) }
        if let item, item.provenance?.aiGenerated == true || item.modifiedBy?.kind == .agent { out.append(.agent) }
        if pinned { out.append(.pinned) }
        if root?.kind == .shared { out.append(.shared) }
        return out
    }

    /// True for a name the runtime gives a conflict copy (SPEC section 4,
    /// `ConflictInfo.conflict_name`): `<stem> (conflict <device> <YYYY-MM-DD
    /// HHmm>)<.ext>`, with " 2", " 3" before ")" when that name was taken.
    public static func isConflictCopy(_ name: String) -> Bool {
        name.range(of: #" \(conflict .+ \d{4}-\d{2}-\d{2} \d{4}( \d+)?\)(\.[^.()]+)?$"#, options: .regularExpression) != nil
    }
}
