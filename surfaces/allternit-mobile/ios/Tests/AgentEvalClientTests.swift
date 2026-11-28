import XCTest
@testable import Allternit

final class AgentEvalClientTests: XCTestCase {
    func testSaveOutcomeMapsToTheSheetState() throws {
        let json = #"{"case":{"id":"case_1","title":"t","label":"bad","needs_check":true,"saved_at":1},"self_test":{"passed":true,"agrees_with_label":false},"needs_check":true}"#
        let saved = try JSONDecoder().decode(EvalSaveResult.self, from: Data(json.utf8))
        XCTAssertEqual(saveTestCaseState(result: .success(saved)), .needsCheck)
        XCTAssertEqual(saveTestCaseState(result: .failure(.noComputer)), .needsComputer)
        XCTAssertEqual(saveTestCaseState(result: .failure(.unreachable)), .unreachable)
        XCTAssertEqual(saveTestCaseState(result: .failure(.caseLimit)), .failed(AgentEvalError.caseLimit.message))
    }

    func testErrorsMapByStatusAndCall() {
        XCTAssertEqual(AgentEvalError.map(APIError.httpError(statusCode: 409, message: nil), conflict: .caseLimit), .caseLimit)
        XCTAssertEqual(AgentEvalError.map(APIError.httpError(statusCode: 409, message: nil), conflict: .estimateRequired), .estimateRequired)
        XCTAssertEqual(AgentEvalError.map(APIError.httpError(statusCode: 502, message: nil)), .unreachable)
        XCTAssertEqual(AgentEvalError.map(APIError.httpError(statusCode: 404, message: nil)), .notFound)
        XCTAssertEqual(AgentEvalError.map(RuntimeRelayError.noComputer), .noComputer)
    }

    func testRunDecodesProgressAndPerCaseRows() throws {
        let json = #"{"run_id":"run_1","state":"done","progress":{"done":2,"total":2},"report":{"schema":"allternit.eval_report.v1","suites":{"my-cases":{"pass_rate":0.5,"failed_cases":["case_2"],"extra":{"per_case":[{"id":"case_1","pass":true},{"id":"case_2","pass":false}]}}}}}"#
        let run = try JSONDecoder().decode(EvalRun.self, from: Data(json.utf8))
        XCTAssertTrue(run.isFinished)
        XCTAssertEqual(run.report?.passRate, 0.5)
        XCTAssertEqual(run.report?.perCase.map(\.pass), [true, false])
    }
}
