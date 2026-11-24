import XCTest
@testable import Allternit

final class ArtifactCollabRulesTests: XCTestCase {
    private func comment(_ id: String, parent: String? = nil, author: String = "u_me", resolved: Bool = false,
                         name: String? = nil, agent: String? = nil) -> ArtifactComment {
        ArtifactComment(id: id, parentId: parent, authorId: author, authorName: name, body: "text \(id)",
                        resolvedAt: resolved ? "2026-11-17T10:00:00Z" : nil, createdAt: "2026-11-17T09:00:00Z",
                        agentName: agent)
    }

    // MARK: Threads

    func testThreadsGroupRepliesUnderTheirRootInOrder() {
        let items = [comment("a"), comment("b"), comment("a1", parent: "a"), comment("b1", parent: "b"), comment("a2", parent: "a")]
        let threads = ArtifactCommentRules.threads(items)
        XCTAssertEqual(threads.map(\.id), ["a", "b"])
        XCTAssertEqual(threads[0].replies.map(\.id), ["a1", "a2"])
        XCTAssertEqual(threads[1].replies.map(\.id), ["b1"])
    }

    func testReplyWithMissingRootStandsAloneAndCannotBeResolved() {
        let orphan = comment("x1", parent: "gone")
        let threads = ArtifactCommentRules.threads([orphan])
        XCTAssertEqual(threads.map(\.id), ["x1"])
        XCTAssertFalse(ArtifactCommentRules.canResolve(threads[0].root, me: "u_me", access: .owner))
    }

    func testOpenThreadCountCountsUnresolvedRootsOnly() {
        let items = [comment("a"), comment("a1", parent: "a"), comment("b", resolved: true), comment("c")]
        XCTAssertEqual(ArtifactCommentRules.openThreadCount(items), 2)
        XCTAssertEqual(ArtifactCommentRules.openThreadCount([]), 0)
    }

    func testResolvedThreadsHiddenUntilAsked() {
        let threads = ArtifactCommentRules.threads([comment("a"), comment("b", resolved: true)])
        XCTAssertEqual(ArtifactCommentRules.visibleThreads(threads, showResolved: false).map(\.id), ["a"])
        XCTAssertEqual(ArtifactCommentRules.visibleThreads(threads, showResolved: true).map(\.id), ["a", "b"])
        XCTAssertEqual(ArtifactCommentRules.resolvedCount(threads), 1)
    }

    // MARK: Permissions (server rules)

    func testCommentNeedsCommentAccess() {
        XCTAssertFalse(ArtifactCommentRules.canComment(access: .view))
        XCTAssertTrue(ArtifactCommentRules.canComment(access: .comment))
        XCTAssertTrue(ArtifactCommentRules.canComment(access: .edit))
        XCTAssertTrue(ArtifactCommentRules.canComment(access: .owner))
    }

    func testResolveIsAuthorOrEditOnRootsOnly() {
        let mine = comment("a", author: "u_me")
        let theirs = comment("b", author: "u_other")
        let reply = comment("a1", parent: "a", author: "u_me")
        XCTAssertTrue(ArtifactCommentRules.canResolve(mine, me: "u_me", access: .comment))
        XCTAssertFalse(ArtifactCommentRules.canResolve(theirs, me: "u_me", access: .comment))
        XCTAssertTrue(ArtifactCommentRules.canResolve(theirs, me: "u_me", access: .edit))
        XCTAssertFalse(ArtifactCommentRules.canResolve(reply, me: "u_me", access: .owner))
    }

    func testEditBodyIsAuthorOnlyEvenForOwner() {
        XCTAssertTrue(ArtifactCommentRules.canEditBody(comment("a", author: "u_me"), me: "u_me"))
        XCTAssertFalse(ArtifactCommentRules.canEditBody(comment("a", author: "u_other"), me: "u_me"))
        XCTAssertFalse(ArtifactCommentRules.canEditBody(comment("a", author: "u_me"), me: nil))
    }

    func testDeleteIsAuthorOrOwner() {
        let theirs = comment("a", author: "u_other")
        XCTAssertFalse(ArtifactCommentRules.canDelete(theirs, me: "u_me", access: .edit))
        XCTAssertTrue(ArtifactCommentRules.canDelete(theirs, me: "u_me", access: .owner))
        XCTAssertTrue(ArtifactCommentRules.canDelete(comment("b", author: "u_me"), me: "u_me", access: .view))
    }

    func testAccessParsesAndOrders() {
        XCTAssertEqual(ArtifactAccessLevel(myAccess: "owner"), .owner)
        XCTAssertEqual(ArtifactAccessLevel(myAccess: "comment"), .comment)
        XCTAssertEqual(ArtifactAccessLevel(myAccess: "something-new"), .view)
        XCTAssertTrue(ArtifactAccessLevel.edit > .comment)
    }

    func testAuthorLabels() {
        XCTAssertEqual(ArtifactCommentRules.authorLabel(comment("a", author: "u_me"), me: "u_me"), "You")
        XCTAssertEqual(ArtifactCommentRules.authorLabel(comment("a", author: "u_x", name: "Maya"), me: "u_me"), "Maya")
        XCTAssertEqual(ArtifactCommentRules.authorLabel(comment("a", author: "u_x"), me: "u_me"), "Collaborator")
        XCTAssertEqual(ArtifactCommentRules.authorLabel(comment("a", author: "assistant", agent: "Gizzi"), me: "u_me"), "Gizzi")
        XCTAssertEqual(ArtifactCommentRules.authorLabel(comment("a", author: "assistant"), me: "u_me"), "Allternit")
    }

    func testPostableBodyTrimsAndBounds() {
        XCTAssertNil(ArtifactCommentRules.postableBody("   \n"))
        XCTAssertEqual(ArtifactCommentRules.postableBody("  hi  "), "hi")
        XCTAssertNil(ArtifactCommentRules.postableBody(String(repeating: "a", count: 4001)))
    }

    func testCommentDecodesServerShape() throws {
        let json = """
        {"items":[{"id":"c1","artifact_id":"art_1","version":2,"anchor":{},"parent_id":null,"author_id":"u1",
        "author_name":"Maya","author_image_url":null,"body":"Looks good","to_assistant":false,
        "resolved_at":null,"created_at":"2026-11-17T09:00:00.123456Z","agent_name":null}]}
        """
        struct Page: Decodable { let items: [ArtifactComment] }
        let page = try JSONDecoder().decode(Page.self, from: Data(json.utf8))
        XCTAssertEqual(page.items.first?.authorName, "Maya")
        XCTAssertTrue(page.items.first?.isRoot == true)
        XCTAssertNotNil(page.items.first?.createdDate)
    }

    // MARK: Sharing

    func testDocsHaveNoCommentLevel() {
        XCTAssertEqual(ArtifactShareRules.levels(kind: "doc"), ["view", "edit"])
        XCTAssertEqual(ArtifactShareRules.levels(kind: "image"), ["view", "comment", "edit"])
        XCTAssertEqual(ArtifactShareRules.clampLevel(kind: "doc", level: "comment"), "view")
    }

    func testInviteChecks() {
        let existing = [ArtifactV2Share(principalType: "email", principalId: "a@b.co", level: "view")]
        XCTAssertEqual(ArtifactShareRules.checkInvite("nope", kind: "image", existing: []), .refused("Enter a valid email address."))
        XCTAssertEqual(ArtifactShareRules.checkInvite(" A@B.co ", kind: "image", existing: existing), .refused("Already shared with this person."))
        XCTAssertEqual(ArtifactShareRules.checkInvite("x@y.co", kind: "doc", existing: []), .refused("Docs can’t be shared by email."))
        XCTAssertEqual(ArtifactShareRules.checkInvite(" X@Y.co ", kind: "page", existing: existing), .ok(email: "x@y.co"))
    }

    func testInviteOpensAPrivateArtifactToPeople() {
        XCTAssertEqual(ArtifactShareRules.visibilityAfterInvite("private"), "people")
        XCTAssertEqual(ArtifactShareRules.visibilityAfterInvite("org"), "org")
    }

    func testLinkBlockedByPolicy() {
        let blocked = ArtifactV2SharingPolicy(linkAllowed: false, outsideInvitesAllowed: true, reason: "No links here.")
        let open = ArtifactV2SharingPolicy(linkAllowed: true, outsideInvitesAllowed: true, reason: nil)
        XCTAssertEqual(ArtifactShareRules.visibilityBlockReason("link", kind: "page", policy: blocked), "No links here.")
        XCTAssertNil(ArtifactShareRules.visibilityBlockReason("link", kind: "page", policy: open))
        XCTAssertNotNil(ArtifactShareRules.visibilityBlockReason("people", kind: "doc", policy: open))
    }

    func testCopyLinkLabelAndTarget() {
        let origin = URL(string: "https://ai.allternit.com")!
        let priv = ArtifactShareRules.copyLink(artifactId: "art_1", sharing: nil, visibility: "private", origin: origin)
        XCTAssertEqual(priv.label, "Copy private link")
        XCTAssertEqual(priv.url.absoluteString, "https://ai.allternit.com/?artifact=art_1")
        let sharing = ArtifactV2Sharing(visibility: "link", shares: [], policy: .init(linkAllowed: true, outsideInvitesAllowed: true, reason: nil),
                                        linkURL: "https://ai.allternit.com/a/art_1")
        let link = ArtifactShareRules.copyLink(artifactId: "art_1", sharing: sharing, visibility: "link", origin: origin)
        XCTAssertEqual(link.label, "Copy link")
        XCTAssertEqual(link.url.absoluteString, "https://ai.allternit.com/a/art_1")
    }

    // MARK: Fill / Fit

    func testFitOnlyForImageAndVideoDefaultFit() {
        XCTAssertTrue(ArtifactFit.hasToggle(kind: "image"))
        XCTAssertTrue(ArtifactFit.hasToggle(kind: "video"))
        XCTAssertFalse(ArtifactFit.hasToggle(kind: "page"))
        XCTAssertEqual(ArtifactFit(stored: nil), .fit)
        XCTAssertEqual(ArtifactFit(stored: "fill"), .fill)
        XCTAssertEqual(ArtifactFit.fit.menuTitle, "Fill screen")
        XCTAssertEqual(ArtifactFit.fill.toggled, .fit)
    }
}
