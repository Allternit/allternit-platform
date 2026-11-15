import XCTest
@testable import Allternit

final class LinkBoardDecodingTests: XCTestCase {
    func testContractListDecodesAndUnknownValuesDoNotCrash() throws {
        let json = #"""
        {"desktop":true,"boards":[
         {"boardId":"usba-0123456789ab","serial":"usb:usba-0123456789ab","name":"Desk board","online":true,"armed":true,"status":"ready","link":"lan","phone":"app:abc","future":1},
         {"boardId":"usba-2","name":"","online":false,"armed":false,"status":"brand_new_state","link":"mesh","phone":null},
         {"name":"no id, dropped"}]}
        """#
        let list = try JSONDecoder().decode(LinkBoardList.self, from: Data(json.utf8))
        XCTAssertEqual(list.boards.count, 2)
        XCTAssertEqual(list.boards[0].status, .ready)
        XCTAssertEqual(list.boards[0].link, .lan)
        XCTAssertTrue(list.boards[0].armed)
        XCTAssertEqual(list.boards[1].status, .other("brand_new_state"))
        XCTAssertEqual(list.boards[1].link, .other("mesh"))
        XCTAssertEqual(list.boards[1].name, "usba-2")
    }

    func testStopAnswers() {
        func r(_ s: Int, _ b: String) -> LinkBoardStopResult { .from(status: s, data: Data(b.utf8)) }
        XCTAssertEqual(r(200, #"{"ok":true}"#), .stopped)
        XCTAssertEqual(r(502, #"{"error":"uncertain"}"#), .uncertain)
        XCTAssertEqual(r(409, #"{"error":"board_offline"}"#), .boardOffline)
        XCTAssertEqual(r(503, #"{"error":"desktop_offline"}"#), .desktopOffline)
        XCTAssertNotEqual(r(500, "{}"), .stopped)
    }
}
