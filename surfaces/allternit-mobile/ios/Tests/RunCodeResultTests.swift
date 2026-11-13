import XCTest
@testable import Allternit

final class RunCodeResultTests: XCTestCase {
    private let files = #"[{"name":"plot.png","mediaType":"image/png","uploadId":"u1","size":9,"url":"/api/v1/uploads/u1"}]"#
    private let run = #"{"stdout":"hi","stderr":"","exit_code":null,"timed_out":true,"skipped":[{"name":"big.bin","reason":"too large"}]}"#

    func testLiveFrameAndTranscriptPartDecodeToSameModel() throws {
        let frame = #"{"type":"tool_result","toolCallId":"c1","toolName":"run_code","files":\#(files),"run":\#(run)}"#
        let part = #"{"type":"tool","tool":"run_code","id":"c1","state":{"status":"completed","input":{}},"files":\#(files),"run":\#(run)}"#

        let event = try JSONDecoder().decode(AgentChatEvent.self, from: Data(frame.utf8))
        guard case .toolResult(let payload) = event else { return XCTFail("not a tool result") }
        let record = try JSONDecoder().decode(AgentSessionMessage.Part.self, from: Data(part.utf8))
        let history = ToolCallRecord.fromHistory([record])

        XCTAssertNotNil(payload.runCode)
        XCTAssertEqual(payload.runCode, history.first?.runCode)
        XCTAssertEqual(payload.runCode?.files.first?.uploadId, "u1")
        XCTAssertEqual(payload.runCode?.files.first?.isImage, true)
        XCTAssertNil(payload.runCode?.run?.exitCode)
        XCTAssertEqual(payload.runCode?.run?.summary, "Timed out · 1 file skipped")
    }

    func testOldRuntimeWithoutFilesDecodesToNil() throws {
        let frame = #"{"type":"tool_result","toolCallId":"c1","toolName":"run_code"}"#
        let event = try JSONDecoder().decode(AgentChatEvent.self, from: Data(frame.utf8))
        guard case .toolResult(let payload) = event else { return XCTFail("not a tool result") }
        XCTAssertNil(payload.runCode)
    }
}
