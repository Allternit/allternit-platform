import BTHIDCore
import BTHIDTool
import Foundation
import XCTest

final class ParseTests: XCTestCase {
    private func parse(_ json: String) -> Result<Command, BTHIDError> { parseRequest(json).result }

    func testCommandsAndDefaults() {
        XCTAssertEqual(try parse(#"{"cmd":"pair"}"#).get(), .pair)
        XCTAssertEqual(try parse(#"{"cmd":"key","usage":44,"modifiers":8,"down":true}"#).get(), .key(usage: 44, modifiers: 8, down: true))
        XCTAssertEqual(try parse(#"{"cmd":"key","usage":0,"down":false}"#).get(), .key(usage: 0, modifiers: 0, down: false))
        XCTAssertEqual(try parse(#"{"cmd":"move","dx":-300,"dy":12}"#).get(), .move(dx: -300, dy: 12))
        XCTAssertEqual(try parse(#"{"cmd":"button"}"#).get(), .button(.left, down: true, up: true))
        XCTAssertEqual(try parse(#"{"cmd":"button","button":"right","up":false}"#).get(), .button(.right, down: true, up: false))
        XCTAssertEqual(try parse(#"{"cmd":"scroll","lines":-3}"#).get(), .scroll(lines: -3))
        XCTAssertEqual(try parse(#"{"cmd":"type","text":"Hello, world!"}"#).get(), .type(text: "Hello, world!"))
    }

    func testIdIsKeptEvenWhenTheCommandIsBad() {
        let (id, result) = parseRequest(#"{"id":7,"cmd":"move","dx":1.5,"dy":0}"#)
        XCTAssertEqual(id, "7")
        XCTAssertEqual(result, .failure(BTHIDError("bad_request", "move needs whole dx and dy within ±32767.")))
    }

    func testRefusesWhatTheKeyboardCannotSend() {
        let codes = [
            #"not json"#,
            #"{"cmd":"key","usage":224,"down":true}"#, // modifiers are bits, not usages
            #"{"cmd":"key","usage":4,"modifiers":256}"#,
            #"{"cmd":"key","usage":true}"#, // a boolean is not a number
            #"{"cmd":"key","usage":4,"down":"yes"}"#,
            #"{"cmd":"type","text":"café"}"#,
            #"{"cmd":"type","text":""}"#,
            #"{"cmd":"move","dx":40000,"dy":0}"#,
            #"{"cmd":"button","button":"back"}"#,
            #"{"cmd":"button","down":false,"up":false}"#,
            #"{"cmd":"scroll","lines":101}"#,
        ].map { (try? parse($0).get()) == nil }
        XCTAssertEqual(codes, Array(repeating: true, count: codes.count))
        XCTAssertEqual(parseRequest(#"{"cmd":"fly"}"#).result, .failure(BTHIDError("unknown_command", "Unknown command fly.")))
        let long = String(repeating: "a", count: maxTypeLength + 1)
        XCTAssertNil(try? parse(#"{"cmd":"type","text":"\#(long)"}"#).get())
    }

    func testEveryCommandHasALimitThatCoversItsWork() {
        XCTAssertGreaterThan(Command.move(dx: 32_767, dy: 0).timeLimit, Command.move(dx: 10, dy: 0).timeLimit)
        XCTAssertGreaterThan(Command.type(text: String(repeating: "a", count: 500)).timeLimit, 2 * 500 * 0.03)
        XCTAssertFalse(Command.pair.isInput)
        XCTAssertTrue(Command.reset.isInput)
    }
}

final class StateTests: XCTestCase {
    func testStateJsonCarriesWhatDesktopReads() throws {
        var s = HIDPeripheralStatus(radio: .on, authorization: .allowed, servicesReady: true, advertising: true, subscribers: 1, centralID: "C1")
        s.lastError = BTHIDError("send_timeout", "slow")
        let line = encodeReply(id: "3", stateJSON(s, localName: "Allternit Keyboard"))
        let obj = try XCTUnwrap(try JSONSerialization.jsonObject(with: Data(line.utf8)) as? [String: Any])
        XCTAssertEqual(obj["id"] as? String, "3")
        XCTAssertEqual(obj["bluetooth"] as? String, "on")
        XCTAssertEqual(obj["authorization"] as? String, "allowed")
        XCTAssertEqual(obj["connected"] as? Bool, true)
        XCTAssertEqual(obj["advertising"] as? Bool, true)
        XCTAssertEqual(obj["refused"] as? Bool, false)
        XCTAssertEqual((obj["last_error"] as? [String: String])?["code"], "send_timeout")
        XCTAssertTrue(obj["name"] is NSNull)
    }

    func testRefusalIsReported() throws {
        let s = HIDPeripheralStatus(radio: .on, authorization: .allowed, refused: true, lastError: .refused("no"))
        let obj = stateJSON(s, localName: "Allternit Keyboard")
        XCTAssertEqual(obj["refused"] as? Bool, true)
        XCTAssertEqual(obj["connected"] as? Bool, false)
        XCTAssertEqual((obj["last_error"] as? [String: String])?["code"], "refused")
    }
}
