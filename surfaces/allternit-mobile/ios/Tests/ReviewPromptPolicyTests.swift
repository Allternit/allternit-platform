import XCTest
@testable import Allternit

final class ReviewPromptPolicyTests: XCTestCase {
    private var defaults: UserDefaults!
    private var date = Date(timeIntervalSince1970: 1_800_000_000)

    override func setUp() {
        defaults = UserDefaults(suiteName: "ReviewPromptPolicyTests-\(UUID().uuidString)")
    }

    private func policy(version: String = "1.0.1", blocked: Bool = false) -> ReviewPromptPolicy {
        ReviewPromptPolicy(defaults: defaults, now: { self.date }, appVersion: version, isBlocked: blocked)
    }

    private func answers(_ n: Int, on days: Int = 1) {
        let p = policy()
        for i in 0..<n {
            if days > 1, i == n - 1 { date = date.addingTimeInterval(86_400 * 2) }
            p.recordSuccessfulAnswer()
        }
    }

    func testUnderFiveAnswers() {
        answers(4, on: 2)
        XCTAssertFalse(policy().shouldAsk(isStreaming: false))
    }

    func testFiveAnswersOneDay() {
        answers(5)
        XCTAssertFalse(policy().shouldAsk(isStreaming: false))
    }

    func testAlreadyAskedThisVersion() {
        answers(5, on: 2)
        policy().markAsked()
        date = date.addingTimeInterval(86_400 * 200)
        XCTAssertFalse(policy().shouldAsk(isStreaming: false))
        XCTAssertTrue(policy(version: "1.0.2").shouldAsk(isStreaming: false))
    }

    func testInside120Days() {
        answers(5, on: 2)
        policy(version: "1.0.0").markAsked()
        date = date.addingTimeInterval(86_400 * 60)
        XCTAssertFalse(policy(version: "1.0.1").shouldAsk(isStreaming: false))
    }

    func testBlockedLaunch() {
        answers(5, on: 2)
        XCTAssertFalse(policy(blocked: true).shouldAsk(isStreaming: false))
    }

    func testStreamingBlocks() {
        answers(5, on: 2)
        XCTAssertFalse(policy().shouldAsk(isStreaming: true))
    }

    func testHappyPath() {
        answers(5, on: 2)
        XCTAssertTrue(policy().shouldAsk(isStreaming: false))
    }
}
