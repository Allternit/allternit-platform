import XCTest
@testable import Allternit

final class StatusSnapshotTests: XCTestCase {
    func testDecodesFullSnapshot() throws {
        let json = """
        {"activeBots":2,"attentionBots":1,"pendingApprovals":1,"paused":false,
         "focus":{"botId":"b1","botName":"Raven","status":"waiting_approval","step":"Drafting",
          "progress":0.4,"inboxId":"approval:ap1","sessionId":"s1","question":"Send?",
          "contentHash":"ab12","approveInline":true,"private":false},
         "bots":[{"botId":"b1","name":"Raven","status":"waiting_approval","step":"x"}],
         "needsYou":["approval:ap1"],
         "needsYouRows":[{"id":"approval:ap1","kind":"approval","botName":"Raven","line":"Approval needed"}],
         "loops":[{"loopId":"l1","command":"c","iterationsCompleted":3,"maxIterations":10,"state":"running"}],
         "lastRoutine":{"routineId":"r1","title":"Morning brief","status":"completed","at":1794200000}}
        """
        let s = try JSONDecoder().decode(StatusSnapshot.self, from: Data(json.utf8))
        XCTAssertEqual(s.activeBots, 2)
        XCTAssertEqual(s.focus?.botName, "Raven")
        XCTAssertEqual(s.focus?.progress, 0.4)
        XCTAssertTrue(s.focus?.approveInline ?? false)
        XCTAssertEqual(s.needsYouRows.first?.line, "Approval needed")
        XCTAssertEqual(s.loops.first?.iterationsCompleted, 3)
        XCTAssertEqual(s.lastRoutine?.title, "Morning brief")
    }

    func testDecodesPartialSnapshot() throws {
        let s = try JSONDecoder().decode(StatusSnapshot.self, from: Data(#"{"activeBots":1,"focus":{"botName":"A"}}"#.utf8))
        XCTAssertEqual(s.activeBots, 1)
        XCTAssertEqual(s.pendingApprovals, 0)
        XCTAssertTrue(s.bots.isEmpty)
        XCTAssertEqual(s.focus?.botName, "A")
        XCTAssertFalse(s.focus?.private ?? true)
        XCTAssertNil(try JSONDecoder().decode(StatusSnapshot.self, from: Data("{}".utf8)).focus)
    }

    func testPrivateQuestionDroppedBeforeStorage() {
        var s = StatusSnapshot()
        s.focus = StatusSnapshot.Focus()
        s.focus?.question = "secret"
        s.focus?.private = true
        XCTAssertNil(s.redactedForStorage().focus?.question)
        s.focus?.private = false
        XCTAssertEqual(s.redactedForStorage().focus?.question, "secret")
    }
}
