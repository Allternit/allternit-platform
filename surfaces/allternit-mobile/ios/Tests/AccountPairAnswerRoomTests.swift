import XCTest
@testable import Allternit

/// Connections rows (design pick B grouped) and the chat-wait anchor (pick B).
/// Mirrors allternit-ai `accountPair.test.tsx` (#1227) and `useAnswerRoom` (#1229).
final class AccountPairAnswerRoomTests: XCTestCase {
    func testGroupsOnlyWhenBothRowsAreListed() {
        XCTAssertTrue(AccountPair.pairsUp(["chatgpt", "codex", "claude"]))
        XCTAssertFalse(AccountPair.pairsUp(["chatgpt", "claude"]))
        XCTAssertFalse(AccountPair.pairsUp(["codex"]))
    }

    func testChatGPTRowIsTheSubscriptionWithAModelCount() throws {
        let row = ConnectionWire(id: "chatgpt", state: .on, models: ["a", "b", "c"])
        let extras = try XCTUnwrap(AccountPair.extras(id: "chatgpt", row: row, paired: true))
        XCTAssertEqual(extras.title, "ChatGPT subscription")
        XCTAssertEqual(extras.badge, "Device sign-in")
        XCTAssertTrue(extras.powers.hasPrefix("Powers: chat models"))
        XCTAssertEqual(AccountPair.statusLine(extras, state: .on, fallback: "x"), "Signed in on this iPhone · 3 models")
        XCTAssertEqual(AccountPair.statusLine(extras, state: .off, fallback: "x"), "Not signed in on this iPhone")
    }

    func testNoModelCountWhenNoneAreKnown() throws {
        let extras = try XCTUnwrap(AccountPair.extras(id: "chatgpt", row: ConnectionWire(id: "chatgpt", state: .on), paired: false))
        XCTAssertEqual(AccountPair.statusLine(extras, state: .on, fallback: "x"), "Signed in on this iPhone")
        let one = try XCTUnwrap(AccountPair.extras(id: "chatgpt", row: ConnectionWire(id: "chatgpt", state: .on, models: ["a"]), paired: false))
        XCTAssertEqual(AccountPair.statusLine(one, state: .on, fallback: "x"), "Signed in on this iPhone · 1 model")
    }

    func testCodexRowIsItsOwnSignInWithItsOwnButtons() throws {
        let row = ConnectionWire(id: "codex", state: .on, found: "Codex CLI ready")
        let paired = try XCTUnwrap(AccountPair.extras(id: "codex", row: row, paired: true))
        XCTAssertEqual(paired.title, "Codex CLI")
        XCTAssertEqual(paired.badge, "Own sign-in")
        XCTAssertEqual(paired.connectLabel, "Set up")
        XCTAssertEqual(paired.disconnectLabel, "Sign out")
        XCTAssertTrue(paired.powers.contains("Signing out of one does not sign out the other"))
        XCTAssertEqual(AccountPair.statusLine(paired, state: .on, fallback: "x"), "Signed in · Codex CLI ready")
        XCTAssertEqual(AccountPair.statusLine(paired, state: .off, fallback: "x"), "Not set up")
        XCTAssertNil(paired.modelCount)
        let alone = try XCTUnwrap(AccountPair.extras(id: "codex", row: row, paired: false))
        XCTAssertEqual(alone.powers, "Powers: coding runs.")
    }

    func testNeedsYouKeepsTheRealReason() throws {
        let extras = try XCTUnwrap(AccountPair.extras(id: "chatgpt", row: ConnectionWire(id: "chatgpt", state: .needs), paired: true))
        XCTAssertEqual(AccountPair.statusLine(extras, state: .needs, fallback: "Needs you · Sign in again"), "Needs you · Sign in again")
    }

    func testOtherRowsGetNoRole() {
        XCTAssertNil(AccountPair.extras(id: "claude", row: ConnectionWire(id: "claude"), paired: true))
    }

    // MARK: chat wait

    private func thread() -> [MessageRecord] {
        [MessageRecord(id: "u0", role: "user", content: "old"), MessageRecord(id: "a0", role: "assistant", content: "old answer"),
         MessageRecord(id: "u1", role: "user", content: "draw a fox"), MessageRecord(id: "a1", role: "assistant", content: "")]
    }

    func testAnchorsTheNewestSentMessageWhenATurnStarts() {
        XCTAssertEqual(AnswerRoom.anchorId(wasStreaming: false, isStreaming: true, messages: thread()), "u1")
        XCTAssertNil(AnswerRoom.anchorId(wasStreaming: true, isStreaming: true, messages: thread()))
        XCTAssertNil(AnswerRoom.anchorId(wasStreaming: true, isStreaming: false, messages: thread()))
        XCTAssertNil(AnswerRoom.anchorId(wasStreaming: false, isStreaming: true, messages: [MessageRecord(id: "a", role: "assistant", content: "")]))
    }

    func testRoomShrinksByWhatTheAnswerAdds() {
        let empty = AnswerRoom.room(viewport: 600, turnHeight: 80)
        let grown = AnswerRoom.room(viewport: 600, turnHeight: 280)
        XCTAssertEqual(empty - grown, 200)
        XCTAssertEqual(AnswerRoom.room(viewport: 600, turnHeight: 900), 0)
    }

    func testAnAnchoredTurnDoesNotFollowTheBottomWhileItRuns() {
        XCTAssertFalse(AnswerRoom.followsBottom(anchored: true, streaming: true))
        XCTAssertTrue(AnswerRoom.followsBottom(anchored: true, streaming: false))
        XCTAssertTrue(AnswerRoom.followsBottom(anchored: false, streaming: true))
    }

    func testSplitStartsTheTurnAtTheAnchor() throws {
        let parts = try XCTUnwrap(AnswerRoom.split(thread(), anchor: "u1"))
        XCTAssertEqual(parts.head.map(\.id), ["u0", "a0"])
        XCTAssertEqual(parts.turn.map(\.id), ["u1", "a1"])
        XCTAssertNil(AnswerRoom.split(thread(), anchor: "gone"))
        XCTAssertNil(AnswerRoom.split(thread(), anchor: nil))
    }
}
