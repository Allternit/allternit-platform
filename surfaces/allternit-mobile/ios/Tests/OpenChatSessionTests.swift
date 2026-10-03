import XCTest
@testable import Allternit

final class OpenChatSessionTests: XCTestCase {
    func testExtractsSessionId() {
        let note = Notification(name: .openChatSession, object: nil,
                                userInfo: ["sessionId": "s-1", "agentId": "a-1"])
        XCTAssertEqual(note.openChatSessionId, "s-1")
    }

    func testRejectsMissingOrEmptySessionId() {
        XCTAssertNil(Notification(name: .openChatSession).openChatSessionId)
        let empty = Notification(name: .openChatSession, object: nil, userInfo: ["sessionId": ""])
        XCTAssertNil(empty.openChatSessionId)
    }
}
