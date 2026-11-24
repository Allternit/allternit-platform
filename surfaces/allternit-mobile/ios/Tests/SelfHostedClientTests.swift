import XCTest
@testable import Allternit

final class SSEParserTests: XCTestCase {
    func testParsesFramesAcrossChunks() {
        var p = SSEParser()
        var frames = p.feed(Array("id: 1\nevent: message\ndata: {\"seq\"".utf8))
        XCTAssertTrue(frames.isEmpty)
        frames += p.feed(Array(":1}\n\n: keepalive\n\nid: 2\nevent: status\ndata: x\n\n".utf8))
        XCTAssertEqual(frames, [
            SSEFrame(id: "1", event: "message", data: "{\"seq\":1}"),
            SSEFrame(id: "2", event: "status", data: "x"),
        ])
        XCTAssertEqual(p.lastEventID, "2")
    }

    func testCRLFMultilineDataAndComments() {
        var p = SSEParser()
        let frames = p.feed(Array(": hi\r\nid:7\r\ndata: a\r\ndata: b\r\n\r\n".utf8))
        XCTAssertEqual(frames, [SSEFrame(id: "7", event: nil, data: "a\nb")])
    }

    func testBlankLineWithoutDataDispatchesNothing() {
        var p = SSEParser(lastEventID: "5")
        XCTAssertTrue(p.feed(Array("\n\n: keepalive\n\n".utf8)).isEmpty)
        XCTAssertEqual(p.lastEventID, "5")
    }

    func testEventDecoding() {
        let json = #"{"id":"e1","seq":3,"type":"tool_call","data":{"tool_call_id":"t","name":"bash","input":{"cmd":"ls"}}}"#
        let ev = SelfHostedClient.decodeEvent(SSEFrame(id: "3", event: "tool_call", data: json))
        XCTAssertEqual(ev?.seq, 3)
        XCTAssertEqual(ev?.payload, .toolCall(id: "t", name: "bash", input: "{cmd: \"ls\"}"))
    }

    func testURLNormalizeRefusesHTTP() {
        XCTAssertThrowsError(try SelfHostedClient.normalize("http://192.168.1.5:8400")) {
            XCTAssertEqual($0 as? SelfHostedError, .insecureURL)
        }
        XCTAssertEqual(try? SelfHostedClient.normalize(" agents.example.com/ "), URL(string: "https://agents.example.com"))
        XCTAssertThrowsError(try SelfHostedClient.normalize("ftp://x.y"))
    }
}
