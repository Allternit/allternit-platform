import XCTest
@testable import Allternit

/// CONTRACT-colibri-unkernel section 7: the browser computer tool loop with a
/// fake computer and a fake model stream (the two edges).
@MainActor
final class BrowserComputerToolLoopTests: XCTestCase {
    final class FakeComputer: BrowserComputerExecutor {
        var calls: [BoxExecParams] = []
        var reply = BoxExecResult(stdout: "ok\n", stderr: "", exitCode: 0)
        func exec(_ params: BoxExecParams) async throws -> BoxExecResult {
            calls.append(params)
            return reply
        }
    }

    private func bashCall(_ n: Int) -> BrowserToolCall {
        BrowserToolCall(id: "call_\(n)", name: "bash", arguments: #"{"command":"echo \#(n)"}"#)
    }

    func testRoundCapSendsOneLastRequestWithNoToolsAndShowsTheCount() async throws {
        let computer = FakeComputer()
        var requests: [BrowserToolRequest] = []
        // The model asks for a tool on every request, even the last one.
        let result = try await BrowserToolLoop.run(
            brain: .tools, messages: [.text(role: "user", content: "go")], executor: computer,
            streamCall: { request in
                requests.append(request)
                return BrowserToolResponse(toolCalls: [self.bashCall(requests.count)])
            })
        XCTAssertEqual(result.rounds, 8)
        XCTAssertEqual(result.roundsLine, "Used 8 of 8 tool rounds")
        XCTAssertEqual(requests.count, 9)
        XCTAssertTrue(requests.last?.toolChoiceNone == true)
        XCTAssertFalse(requests.dropLast().contains { $0.toolChoiceNone })
        // The call in the last request is ignored: only 8 commands ran.
        XCTAssertEqual(computer.calls.count, 8)
    }

    func testOutputOverSixteenKiBKeepsBothEndsAndSaysSo() async throws {
        let computer = FakeComputer()
        computer.reply = BoxExecResult(stdout: String(repeating: "a", count: 10_000) + String(repeating: "z", count: 30_000),
                                       stderr: "", exitCode: 0)
        var sent: [BrowserWireMessage] = []
        var turn = 0
        _ = try await BrowserToolLoop.run(
            brain: .tools, messages: [.text(role: "user", content: "go")], executor: computer,
            streamCall: { request in
                turn += 1
                sent = request.messages
                return BrowserToolResponse(toolCalls: turn == 1 ? [self.bashCall(1)] : [])
            })
        guard case .toolResult(_, let content)? = sent.last else { return XCTFail("no tool result sent back") }
        XCTAssertLessThanOrEqual(content.utf8.count, BrowserToolCaps.resultMessageBytes)
        let json = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(content.utf8)) as? [String: Any])
        XCTAssertEqual(json["truncated"] as? Bool, true)
        let stdout = try XCTUnwrap(json["stdout"] as? String)
        XCTAssertTrue(stdout.hasPrefix("aaaa"))
        XCTAssertTrue(stdout.hasSuffix("zzzz"))
        XCTAssertTrue(stdout.contains("bytes cut ...]"))
    }

    func testPlanBrainGetsNoToolsAndThePlainLine() async throws {
        let computer = FakeComputer()
        var requests: [BrowserToolRequest] = []
        var text = ""
        var callbacks = BrowserToolLoopCallbacks()
        callbacks.onChunk = { text += $0 }
        let result = try await BrowserToolLoop.run(
            brain: .plan, messages: [.text(role: "user", content: "hi")], executor: computer,
            streamCall: { request in
                requests.append(request)
                request.onDelta("Hello.")
                return BrowserToolResponse()
            }, callbacks: callbacks)
        XCTAssertEqual(requests.count, 1)
        XCTAssertNil(requests[0].tools)
        XCTAssertEqual(text, "Hello.\n\nPlan models cannot run commands. Open the Terminal to use your computer yourself.")
        XCTAssertEqual(result.rounds, 0)
        XCTAssertTrue(computer.calls.isEmpty)
    }

    func testBadArgumentsAreAnErrorTheLoopSurvives() async throws {
        let computer = FakeComputer()
        var errors: [String] = []
        var callbacks = BrowserToolLoopCallbacks()
        callbacks.onToolError = { _, _, error in errors.append(error) }
        var turn = 0
        let result = try await BrowserToolLoop.run(
            brain: .tools, messages: [], executor: computer,
            streamCall: { _ in
                turn += 1
                return BrowserToolResponse(toolCalls: turn == 1 ? [BrowserToolCall(id: "c", name: "bash", arguments: "{nope")] : [])
            }, callbacks: callbacks)
        XCTAssertEqual(errors, ["The tool arguments were not valid JSON."])
        XCTAssertEqual(result.rounds, 1)
        XCTAssertTrue(computer.calls.isEmpty)
    }
}
