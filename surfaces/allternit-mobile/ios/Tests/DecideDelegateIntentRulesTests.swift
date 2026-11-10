import XCTest
@testable import Allternit

/// The rules behind the Talk, Delegate and Decide intents that run without
/// the system (plan steps I4b, I4c): what each outcome says, when Approve
/// must open the app instead, and the 10 MB file cap.
final class DecideDelegateIntentRulesTests: XCTestCase {
    private func item(summary: String = "Reply to Dana",
                      fullText: String = "Hi Dana, Thursday at 3 works for me. See you then.",
                      recipient: String? = "Dana",
                      hash: String? = "ab12",
                      peerHeld: Bool = true,
                      canApprove: Bool = true) -> ApprovalInfo {
        ApprovalInfo(id: "i1", kind: "peer.send", botId: "b1", botName: "Scout", summary: summary,
                     fullText: fullText, recipient: recipient, contentHash: hash, isPeerHeld: peerHeld,
                     canApprove: canApprove)
    }

    // MARK: Outcome -> dialog

    func testChangedSaysItChangedAndWasNotApproved() {
        let text = DecisionWording.dialog(for: DecisionResult(kind: .approve, outcome: .changed, itemId: "i1"), item: item())
        XCTAssertTrue(text.contains("changed"))
        XCTAssertTrue(text.contains("not approved"))
    }

    func testEachOutcomeSaysWhatHappened() {
        let i = item()
        func say(_ kind: DecisionKind, _ o: DecisionOutcome) -> String {
            DecisionWording.dialog(for: DecisionResult(kind: kind, outcome: o, itemId: "i1"), item: i)
        }
        XCTAssertEqual(say(.approve, .sent), "Approved and sent to Dana.")
        XCTAssertEqual(say(.approve, .approved), "Approved. Scout will go ahead.")
        XCTAssertEqual(say(.deny, .denied), "Denied. Scout will not go ahead.")
        XCTAssertTrue(say(.approve, .alreadyHandled).contains("already handled"))
        XCTAssertTrue(say(.deny, .offline).contains("nothing was denied"))
        XCTAssertTrue(say(.approve, .offline).contains("nothing was approved"))
        XCTAssertTrue(say(.deny, .changed).contains("not denied"))
    }

    func testEveryOutcomeMapsToTheShortcutsValue() {
        let all: [DecisionOutcome] = [.approved, .sent, .changed, .alreadyHandled, .offline, .denied, .snoozed]
        for o in all { XCTAssertEqual(DecisionOutcomeAppEnum(o).rawValue, o.rawValue) }
    }

    // MARK: No full text -> open instead of approve

    func testFullTextWithRecipientAndHashCanBeDecidedHere() {
        XCTAssertFalse(DecisionRules.needsAppReview(item()))
    }

    func testMissingOrCutTextOpensTheApp() {
        XCTAssertTrue(DecisionRules.needsAppReview(item(fullText: "  ")))
        // The summary stands in for the text and is cut.
        XCTAssertTrue(DecisionRules.needsAppReview(item(summary: "Hi Dana, Thursday at…", fullText: "Hi Dana, Thursday at…")))
        // Snapshot rows are read-only.
        XCTAssertTrue(DecisionRules.needsAppReview(item(canApprove: false)))
        // A real text that happens to end in an ellipsis is fine.
        XCTAssertFalse(DecisionRules.needsAppReview(item(fullText: "Let me think...")))
    }

    func testAMessageToSendNeedsRecipientAndHash() {
        XCTAssertTrue(DecisionRules.needsAppReview(item(recipient: nil)))
        XCTAssertTrue(DecisionRules.needsAppReview(item(hash: " ")))
        XCTAssertFalse(DecisionRules.needsAppReview(item(recipient: nil, hash: nil, peerHeld: false)))
    }

    func testConfirmationShowsTheWholeTextAndTheRecipient() {
        let long = String(repeating: "word ", count: 400) + "END"
        let text = DecisionWording.confirmation(item(fullText: long), kind: .approve)
        XCTAssertTrue(text.contains(long))
        XCTAssertTrue(text.contains("to Dana"))
    }

    // MARK: 10 MB cap

    func testFilesUpToTenMegabytesPass() throws {
        XCTAssertNoThrow(try SendFileRules.check(byteCount: SendFileRules.maxBytes))
        XCTAssertEqual(SendFileRules.maxBytes, 10 * 1024 * 1024)
    }

    func testLargerFilesGetAClearError() {
        XCTAssertThrowsError(try SendFileRules.check(byteCount: SendFileRules.maxBytes + 1)) { error in
            let tooLarge = error as? SendFileRules.TooLarge
            XCTAssertNotNil(tooLarge)
            XCTAssertTrue(tooLarge?.message.contains("up to 10 MB") == true)
        }
    }

    func testFileReachesTheBotAsALinkUnderTheNote() {
        XCTAssertEqual(SendFileRules.message(filename: "a[1].png", url: "https://x/u/1", note: " look "),
                       "look\n\n[a(1).png](https://x/u/1)")
        XCTAssertNil(SendFileRules.compose(text: "  ", link: nil))
    }

    // MARK: Ask

    func testAskStillWorkingNamesTheConversation() {
        let r = AskResult(botName: "Scout", answer: "partial", stillWorking: true,
                          conversation: ConversationInfo(id: "c1", title: "Main chat"))
        XCTAssertEqual(TalkWording.askDialog(r), "Scout is still working on it. The answer will be in the chat \"Main chat\".")
    }
}
