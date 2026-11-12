import XCTest
@testable import Allternit

final class EditHistoryTests: XCTestCase {
    private func iso(_ minute: Int) -> String { String(format: "2026-11-12T10:%02d:00Z", minute) }
    private func date(_ minute: Int) -> Date { EditHistory.parse(iso(minute))! }

    private func branch(_ id: String, anchor: String, at minute: Int) -> MessageBranch {
        MessageBranch(id: id, anchorMessageId: anchor, createdAt: iso(minute + 5), messages: [
            .init(id: anchor, role: "user", content: "old", timestamp: iso(minute)),
            .init(id: anchor + "r", role: "assistant", content: "old reply", timestamp: iso(minute + 1)),
        ])
    }

    func testBranchMapsToResentUserMessageByPosition() {
        let transcript = [
            EditHistory.Entry(id: "u1", role: "user", date: date(0)),
            EditHistory.Entry(id: "a1", role: "assistant", date: date(1)),
            EditHistory.Entry(id: "u2new", role: "user", date: date(10)),
            EditHistory.Entry(id: "a2new", role: "assistant", date: date(11)),
        ]
        let result = EditHistory.map(branches: [branch("brn_1", anchor: "u2old", at: 5)], transcript: transcript)
        XCTAssertEqual(result.keys.sorted(), ["u2new"])
        XCTAssertEqual(result["u2new"]?.first?.id, "brn_1")
    }

    func testEditOfFirstMessageAndRepeatedEdits() {
        let transcript = [
            EditHistory.Entry(id: "u1new", role: "user", date: date(20)),
            EditHistory.Entry(id: "a1new", role: "assistant", date: date(21)),
        ]
        let result = EditHistory.map(
            branches: [branch("brn_2", anchor: "u1v2", at: 10), branch("brn_1", anchor: "u1v1", at: 0)],
            transcript: transcript)
        XCTAssertEqual(result["u1new"]?.map(\.id), ["brn_2", "brn_1"])
    }

    func testUnmatchedBranchIsDropped() {
        let transcript = [EditHistory.Entry(id: "a0", role: "assistant", date: date(0))]
        XCTAssertTrue(EditHistory.map(branches: [branch("brn_1", anchor: "x", at: 5)], transcript: transcript).isEmpty)
    }
}
