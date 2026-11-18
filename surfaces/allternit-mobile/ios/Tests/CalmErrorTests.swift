import XCTest
@testable import Allternit

/// Calm errors at the source (phone calm pass step 19, contract C5): no
/// status number, status phrase or machine code reaches UI text, and a
/// compute-gate refusal reads as "no computer", not as an error.
final class CalmErrorTests: XCTestCase {
    private func hasStatusText(_ text: String) -> Bool {
        text.range(of: #"\b[1-5]\d\d\b"#, options: .regularExpression) != nil
            || text.range(of: "status", options: .caseInsensitive) != nil
            || text.range(of: "HTTP", options: .caseInsensitive) != nil
    }

    private func response(_ status: Int) -> HTTPURLResponse {
        HTTPURLResponse(url: URL(string: "https://api.allternit.com/api/v1/x")!,
                        statusCode: status, httpVersion: nil, headerFields: nil)!
    }

    private let client = APIClient(tokenProvider: { nil })

    // MARK: - No status in any message

    func testNoStatusForAnyStatusWithoutMessage() {
        for status in 400...599 {
            let text = APIError.httpError(statusCode: status, message: nil).errorDescription ?? ""
            XCTAssertFalse(text.isEmpty, "\(status) has no text")
            XCTAssertFalse(hasStatusText(text), "\(status) -> \(text)")
        }
    }

    func testNoStatusWhenTheServerMessageIsOnlyStatus() {
        for status in 400...599 {
            let raw = "Request failed with status \(status)."
            let text = APIError.httpError(statusCode: status, message: raw).errorDescription ?? ""
            XCTAssertFalse(hasStatusText(text), "\(status) -> \(text)")
        }
    }

    func testValidateNeverCarriesStatusForAnyBodyShape() {
        let bodies: [String] = [
            #"{"error":{"code":"some_code","message":"Request failed with status 500"}}"#,
            #"{"error":"NOT_FOUND","code":"NOT_FOUND"}"#,
            #"{"message":"HTTP 503 Service Unavailable"}"#,
            #"{"detail":"Error 422"}"#,
            "<html>502 Bad Gateway</html>",
            "",
        ]
        for status in 400...599 {
            for body in bodies {
                do {
                    try client.validate(response(status), data: Data(body.utf8), method: "POST")
                    XCTFail("\(status) did not throw")
                } catch {
                    let text = error.localizedDescription
                    XCTAssertFalse(text.isEmpty)
                    XCTAssertFalse(hasStatusText(text), "\(status) \(body) -> \(text)")
                    XCTAssertFalse(text.contains("_"), "code leaked: \(text)")
                }
            }
        }
    }

    // MARK: - Compute gate

    func testComputeRequiredObjectBodyIsAGateRefusal() {
        let body = #"{"error":{"code":"compute_required","message":"Agents need a computer. Link your own, subscribe, or buy credits.","paths":{}}}"#
        do {
            // A read: it marks the refused flag and never opens the gate sheet.
            try client.validate(response(409), data: Data(body.utf8), method: "GET")
            XCTFail("409 did not throw")
        } catch {
            guard let api = error as? APIError else { return XCTFail("not an APIError: \(error)") }
            XCTAssertTrue(api.isComputeGateRefusal)
            XCTAssertTrue(isComputeGateRefusal(error))
            XCTAssertEqual(api.errorDescription, CalmCopy.computeRequired)
            XCTAssertTrue((api.errorDescription ?? "").localizedCaseInsensitiveContains("computer"))
            XCTAssertFalse(hasStatusText(plainErrorText(error)))
        }
    }

    func testOtherGateShapes() {
        XCTAssertTrue(APIError.httpError(statusCode: 428, message: "no data-plane node, pair a device").isComputeGateRefusal)
        XCTAssertTrue(APIError.httpError(statusCode: 409, message: "compute_required").isComputeGateRefusal)
        XCTAssertTrue(APIError.httpError(statusCode: 410, message: "free_computers_retired").isComputeGateRefusal)
        XCTAssertTrue(APIError.transport(APIError.httpError(statusCode: 428, message: nil)).isComputeGateRefusal)
        // A plain conflict is not the gate.
        XCTAssertFalse(APIError.httpError(statusCode: 409, message: "changed").isComputeGateRefusal)
        XCTAssertFalse(APIError.httpError(statusCode: 500, message: "compute_required").isComputeGateRefusal)
    }

    @MainActor
    func testGateReadsAsNoComputerOnScreen() {
        let shown = CalmStateText(isError: true, icon: "exclamationmark.triangle",
                                  title: "Couldn't load", message: CalmCopy.computeRequired)
        XCTAssertTrue(shown.isNoComputer)
        XCTAssertEqual(shown.title, CalmCopy.noComputer)
        XCTAssertTrue(SettingsLoadFailure.from(APIError.httpError(statusCode: 428, message: nil)).isNoComputer)
        XCTAssertTrue(ShellSessionsStore.isNoComputer(APIError.httpError(statusCode: 409, message: CalmCopy.computeRequired)))
    }

    func testReadsOnlyMarkTheRefusedFlag() {
        XCTAssertTrue(ComputeReadState.isRead(method: "GET"))
        XCTAssertTrue(ComputeReadState.isRead(method: "head"))
        XCTAssertFalse(ComputeReadState.isRead(method: "POST"))
        XCTAssertFalse(ComputeReadState.isRead(method: nil))
    }

    // MARK: - Cleaning server text

    func testStatus502MessageIsCleaned() {
        let text = APIError.httpError(statusCode: 502, message: "Request failed with status 502.").errorDescription
        XCTAssertEqual(text, "Something went wrong on our side. Try again.")
        XCTAssertEqual(stripStatusText("Request failed with status 502."), "Request failed.")
        XCTAssertNil(calmMessageText("Request failed with status 502."))
    }

    func testStripKeepsTheHumanPart() {
        XCTAssertEqual(stripStatusText("Couldn't save (409)"), "Couldn't save")
        XCTAssertEqual(stripStatusText("Couldn't load skills: HTTP 500"), "Couldn't load skills")
        XCTAssertEqual(stripStatusText("Imported 404 files"), "Imported 404 files")
        XCTAssertEqual(plainErrorText(message: "compute_required: Agents need a computer."), "Agents need a computer.")
        XCTAssertEqual(plainErrorText(message: "RUNTIME_OFFLINE"), "Can't reach your computer.")
        XCTAssertEqual(plainErrorText(message: "UNKNOWN_THING", status: 404), "That isn't there anymore.")
        XCTAssertFalse(hasStatusText(FriendlyErrorMessage.from("Request failed with status 500.")))
    }

    func testStatusSentencesMatchTheWeb() {
        XCTAssertEqual(statusSentence(401), "Sign in again to continue.")
        XCTAssertEqual(statusSentence(403), "You don't have access to that.")
        XCTAssertEqual(statusSentence(404), "That isn't there anymore.")
        XCTAssertEqual(statusSentence(429), "Too many requests. Wait a moment and try again.")
        XCTAssertEqual(statusSentence(503), "Something went wrong on our side. Try again.")
        XCTAssertNil(statusSentence(409))
        XCTAssertNil(statusSentence(nil))
    }

    func testCopyMatchesTheWeb() {
        XCTAssertEqual(CalmCopy.noComputer, "No computer connected")
        XCTAssertEqual(CalmCopy.cantReach, "Can't reach your computer")
        XCTAssertEqual(CalmCopy.waking, "Waking your computer…")
    }
}
