import Foundation

// Pure rules behind the artifact viewer's comments, share sheet and Fill/Fit
// (phone calm I12). They mirror the server (routes/artifacts_comments.rs,
// artifacts/sharing.rs) and the web (CommentsPanel.tsx, share-rules.ts,
// components/artifacts/phone-viewer.ts) so the phone never offers an action
// the server refuses. The server still enforces every rule.

/// `my_access`, ordered (artifacts/access.rs `Access`).
enum ArtifactAccessLevel: Int, Comparable, Sendable {
    case view = 0, comment, edit, owner

    /// Unknown strings read as view, the least access.
    init(myAccess: String) {
        switch myAccess.lowercased() {
        case "owner": self = .owner
        case "edit": self = .edit
        case "comment": self = .comment
        default: self = .view
        }
    }

    static func < (lhs: Self, rhs: Self) -> Bool { lhs.rawValue < rhs.rawValue }
}

/// A thread: its first comment and the replies under it, oldest first.
struct ArtifactCommentThread: Identifiable, Hashable, Sendable {
    let root: ArtifactComment
    let replies: [ArtifactComment]

    var id: String { root.id }
    var isOpen: Bool { !root.isResolved }
}

enum ArtifactCommentRules {
    /// The id the server stores for an agent's comment.
    static let assistantId = "assistant"

    /// Rebuilds threads from `parent_id` (the server lists comments in
    /// creation order). A reply whose root is missing (the list is capped at
    /// 500) stands as its own thread so nothing disappears; it can't be
    /// resolved, since it isn't a root.
    static func threads(_ comments: [ArtifactComment]) -> [ArtifactCommentThread] {
        let rootIds = Set(comments.filter(\.isRoot).map(\.id))
        var repliesByRoot: [String: [ArtifactComment]] = [:]
        var heads: [ArtifactComment] = []
        for comment in comments {
            if let parent = comment.parentId, rootIds.contains(parent) {
                repliesByRoot[parent, default: []].append(comment)
            } else {
                heads.append(comment)
            }
        }
        return heads.map { ArtifactCommentThread(root: $0, replies: repliesByRoot[$0.id] ?? []) }
    }

    /// Unresolved threads: the comments badge (web `useOpenThreadCount`).
    static func openThreadCount(_ comments: [ArtifactComment]) -> Int {
        comments.filter { $0.isRoot && !$0.isResolved }.count
    }

    /// Threads shown: open ones always, resolved ones when asked.
    static func visibleThreads(_ threads: [ArtifactCommentThread], showResolved: Bool) -> [ArtifactCommentThread] {
        showResolved ? threads : threads.filter(\.isOpen)
    }

    static func resolvedCount(_ threads: [ArtifactCommentThread]) -> Int {
        threads.filter { !$0.isOpen }.count
    }

    /// Create a thread or reply: comment access or more.
    static func canComment(access: ArtifactAccessLevel) -> Bool {
        access >= .comment
    }

    /// Resolve or reopen: thread roots only, by their author or anyone who can edit.
    static func canResolve(_ comment: ArtifactComment, me: String?, access: ArtifactAccessLevel) -> Bool {
        guard comment.isRoot else { return false }
        return isAuthor(comment, me: me) || access >= .edit
    }

    /// Edit the text: the author only.
    static func canEditBody(_ comment: ArtifactComment, me: String?) -> Bool {
        isAuthor(comment, me: me)
    }

    /// Delete (a root takes its replies): the author or the artifact's owner.
    static func canDelete(_ comment: ArtifactComment, me: String?, access: ArtifactAccessLevel) -> Bool {
        isAuthor(comment, me: me) || access == .owner
    }

    static func isAuthor(_ comment: ArtifactComment, me: String?) -> Bool {
        guard let me, !me.isEmpty else { return false }
        return comment.authorId == me
    }

    /// The name shown on a comment (web CommentsPanel `authorLabel`).
    static func authorLabel(_ comment: ArtifactComment, me: String?) -> String {
        if comment.authorId == assistantId { return nonEmpty(comment.agentName) ?? "Allternit" }
        if isAuthor(comment, me: me) { return "You" }
        return nonEmpty(comment.authorName) ?? "Collaborator"
    }

    /// Trimmed text the server accepts (1...4000 characters), else nil.
    static func postableBody(_ raw: String) -> String? {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, trimmed.count <= 4000 else { return nil }
        return trimmed
    }

    private static func nonEmpty(_ value: String?) -> String? {
        guard let value = value?.trimmingCharacters(in: .whitespaces), !value.isEmpty else { return nil }
        return value
    }
}

/// Client mirror of the sharing rules (web share-rules.ts; server
/// artifacts/sharing.rs). Outside-org checks stay on the server: the phone
/// doesn't know the org's domains, so it shows the server's reason.
enum ArtifactShareRules {
    static let visibilities = ["private", "people", "org", "link"]

    static func levelLabel(_ level: String) -> String {
        switch level {
        case "edit": return "Can edit"
        case "comment": return "Commenter"
        default: return "Can view"
        }
    }

    static func visibilityLabel(_ visibility: String) -> String {
        switch visibility {
        case "people": return "Specific people"
        case "org": return "Your org"
        case "link": return "Anyone with the link"
        default: return "Only you"
        }
    }

    static func visibilityHint(_ visibility: String) -> String {
        switch visibility {
        case "people": return "Only the people you add."
        case "org": return "Anyone in your org can view. Add people to give them more."
        case "link": return "Anyone who has the link can view. No sign-in needed."
        default: return "Nothing is shared until you share it."
        }
    }

    /// Docs: Viewer and Editor only (no comment-only level).
    static func levels(kind: String) -> [String] {
        kind == "doc" ? ["view", "edit"] : ["view", "comment", "edit"]
    }

    static func clampLevel(kind: String, level: String) -> String {
        levels(kind: kind).contains(level) ? level : "view"
    }

    /// Why a General access choice can't be picked, or nil when it can.
    static func visibilityBlockReason(_ visibility: String, kind: String, policy: ArtifactV2SharingPolicy) -> String? {
        switch visibility {
        case "people" where kind == "doc":
            return "Docs are shared with your whole org, not by email."
        case "link" where !policy.linkAllowed:
            return policy.reason ?? "Link sharing is turned off for this artifact."
        default:
            return nil
        }
    }

    enum InviteCheck: Equatable {
        case ok(email: String)
        case refused(String)
    }

    /// Validate an email invite before it's sent (web `checkInvite`).
    static func checkInvite(_ raw: String, kind: String, existing: [ArtifactV2Share]) -> InviteCheck {
        let email = raw.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard email.range(of: #"^[^\s@]+@[^\s@]+\.[^\s@]+$"#, options: .regularExpression) != nil else {
            return .refused("Enter a valid email address.")
        }
        if existing.contains(where: { $0.principalType == "email" && $0.principalId.lowercased() == email }) {
            return .refused("Already shared with this person.")
        }
        if kind == "doc" { return .refused("Docs can’t be shared by email.") }
        return .ok(email: email)
    }

    /// Adding someone to a private artifact makes it "Specific people".
    static func visibilityAfterInvite(_ visibility: String) -> String {
        visibility == "private" ? "people" : visibility
    }

    /// The private link opens the artifact in the app for anyone who has
    /// access (web `artifactAppLink`); a link-shared artifact copies the
    /// server's public link.
    static func appLink(artifactId: String, origin: URL) -> URL {
        var components = URLComponents(url: origin, resolvingAgainstBaseURL: false) ?? URLComponents()
        components.path = "/"
        components.queryItems = [URLQueryItem(name: "artifact", value: artifactId)]
        return components.url ?? origin
    }

    static func copyLink(artifactId: String, sharing: ArtifactV2Sharing?, visibility: String, origin: URL) -> (label: String, url: URL) {
        if visibility != "private", let raw = sharing?.linkURL, let url = URL(string: raw) {
            return ("Copy link", url)
        }
        return (visibility == "private" ? "Copy private link" : "Copy link", appLink(artifactId: artifactId, origin: origin))
    }
}

/// Fill / Fit for image and video artifacts (web phone-viewer.ts
/// `ARTIFACT_FIT_KINDS`, `useArtifactFit`). One choice for every artifact on
/// this phone, default Fit.
enum ArtifactFit: String, Sendable {
    case fit, fill

    static let storageKey = "allternit.artifact-fit"
    static let kinds: Set<String> = ["image", "video"]

    static func hasToggle(kind: String) -> Bool { kinds.contains(kind) }

    init(stored: String?) { self = stored == ArtifactFit.fill.rawValue ? .fill : .fit }

    var toggled: ArtifactFit { self == .fill ? .fit : .fill }

    /// The ⋯ item names what a tap does.
    var menuTitle: String { self == .fill ? "Fit to screen" : "Fill screen" }
}
