import XCTest
@testable import Allternit

/// The button → request mapping and what each answer does to the
/// notification (plan step I1).
final class NotificationActionsTests: XCTestCase {
    func testApproveCarriesTheContentHash() {
        XCTAssertEqual(
            NotificationActions.operation(for: .approve, contentHash: "ab12", sessionId: nil, replyText: nil),
            .approve(contentHash: "ab12")
        )
        XCTAssertEqual(
            NotificationActions.operation(for: .approve, contentHash: "  ", sessionId: nil, replyText: nil),
            .approve(contentHash: nil)
        )
    }

    func testReplyNeedsSessionAndText() {
        XCTAssertEqual(
            NotificationActions.operation(for: .reply, contentHash: nil, sessionId: "s1", replyText: " hi "),
            .reply(sessionId: "s1", text: "hi")
        )
        XCTAssertNil(NotificationActions.operation(for: .reply, contentHash: nil, sessionId: nil, replyText: "hi"))
        XCTAssertNil(NotificationActions.operation(for: .reply, contentHash: nil, sessionId: "s1", replyText: "   "))
    }

    func testOpenAndReviewAreNotBackgroundOperations() {
        XCTAssertNil(NotificationActions.operation(for: .open, contentHash: nil, sessionId: nil, replyText: nil))
        XCTAssertNil(NotificationActions.operation(for: .review, contentHash: nil, sessionId: nil, replyText: nil))
    }

    func test404IsSilentRemoval() {
        let result = NotificationActions.classify(APIError.httpError(statusCode: 404, message: nil))
        XCTAssertEqual(result, .alreadyHandled)
        XCTAssertEqual(NotificationActions.followUp(for: result), .remove)
    }

    func test409ChangedReplacesWithALocalNotification() {
        let result = NotificationActions.classify(APIError.httpError(statusCode: 409, message: "changed"))
        XCTAssertEqual(result, .changed)
        XCTAssertEqual(NotificationActions.followUp(for: result), .replace("This changed. Review it again."))
    }

    func testOfflineComputerKeepsTheOriginalAndTellsTheOwner() {
        for error: Error in [RuntimeRelayError.unreachable, RuntimeRelayError.noComputer, NotificationActionTimeout(), URLError(.notConnectedToInternet)] {
            let result = NotificationActions.classify(error)
            XCTAssertEqual(result, .offline, "\(error)")
            XCTAssertEqual(NotificationActions.followUp(for: result), .notify("Your computer is offline. Open Allternit to retry."))
        }
    }

    func testAuthFailureTellsTheOwnerToOpenTheApp() {
        let result = NotificationActions.classify(APIError.httpError(statusCode: 401, message: nil))
        XCTAssertEqual(result, .signedOut)
        if case .notify = NotificationActions.followUp(for: result) {} else { XCTFail("expected a local notification") }
    }

    func testRevokeMatchesByTagThreadOrInboxId() {
        let info: [AnyHashable: Any] = ["tag": "approval-ap1", "data": ["inboxId": "approval:ap1"]]
        XCTAssertTrue(NotificationActions.matches(identifier: "uuid-1", threadId: "", userInfo: info,
                                                  inboxIds: [], tags: ["approval-ap1"]))
        XCTAssertTrue(NotificationActions.matches(identifier: "uuid-1", threadId: "", userInfo: ["data": ["inboxId": "approval:ap1"]],
                                                  inboxIds: [], tags: ["approval-ap1"]))
        XCTAssertTrue(NotificationActions.matches(identifier: "approval-ap1", threadId: "", userInfo: [:],
                                                  inboxIds: [], tags: ["approval-ap1"]))
        XCTAssertTrue(NotificationActions.matches(identifier: "followup-approval:ap1", threadId: "", userInfo: [:],
                                                  inboxIds: ["approval:ap1"], tags: []))
        XCTAssertFalse(NotificationActions.matches(identifier: "uuid-2", threadId: "t9", userInfo: ["data": ["inboxId": "approval:ap2"]],
                                                   inboxIds: ["approval:ap1"], tags: ["approval-ap1"]))
    }

    func testCategoriesHaveTheContractButtonsInOrder() {
        let byId = Dictionary(uniqueKeysWithValues: NotificationCategories.all().map { ($0.identifier, $0) })
        XCTAssertEqual(byId["APPROVAL_INLINE"]?.actions.map(\.identifier), ["APPROVE", "DENY", "REPLY"])
        XCTAssertEqual(byId["APPROVAL_REVIEW"]?.actions.map(\.identifier), ["REVIEW", "DENY", "REPLY"])
        XCTAssertEqual(byId["BOT_REPLY"]?.actions.map(\.identifier), ["REPLY", "OPEN"])
        XCTAssertEqual(byId["RUN_DONE"]?.actions.map(\.identifier), ["OPEN"])
        XCTAssertEqual(byId["NEEDS_FIX"]?.actions.map(\.identifier), ["RETRY", "SNOOZE"])
        for action in byId["APPROVAL_INLINE"]?.actions ?? [] {
            XCTAssertTrue(action.options.contains(.authenticationRequired))
            XCTAssertFalse(action.options.contains(.foreground))
        }
    }

    func testDeepLinks() {
        XCTAssertEqual(DeepLink.parse(url: URL(string: "allternit://inbox/approval%3Aap1")!), .inboxItem("approval:ap1"))
        // Hand-off: the control's link, the path, and the web/Android shortcut all open the composer.
        XCTAssertEqual(DeepLink.parse(url: URL(string: "allternit://handoff")!), .handoff)
        XCTAssertEqual(DeepLink.parse(url: URL(string: "https://m.allternit.com/?allternit_open=handoff")!), .handoff)
        XCTAssertEqual(DeepLink.parse(url: URL(string: "allternit://bot/b1/chat?prompt=hello%20there")!),
                       .botChat(botId: "b1", prompt: "hello there"))
        XCTAssertEqual(DeepLink.parse(url: URL(string: "allternit://call/main")!), .callMain)
        XCTAssertEqual(DeepLink.parse(url: URL(string: "allternit://routine/r1")!), .routine("r1"))
        XCTAssertEqual(DeepLink.parse(url: URL(string: "https://m.allternit.com/inbox/attention:at9")!), .inboxItem("attention:at9"))
    }
}
