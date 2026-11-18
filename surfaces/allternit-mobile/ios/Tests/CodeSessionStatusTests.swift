import XCTest
@testable import Allternit

/// Port of the web `code-session-filter.test.ts` (ai #1155) and the
/// `codeSessionFilterStatus` order from `code-session-status.ts` (ai #1154).
final class CodeSessionStatusTests: XCTestCase {
    func testOrderFirstMatchWins() {
        // Archived beats everything, even a pending question while streaming.
        XCTAssertEqual(codeSessionFilterStatus(CodeSessionSignals(archived: true, needsInput: true, readyForReview: true, completed: true), streaming: true), .archived)
        // Needs input beats Working: a run waiting on the user is not "working".
        XCTAssertEqual(codeSessionFilterStatus(CodeSessionSignals(needsInput: true, readyForReview: true), streaming: true), .needsInput)
        // Working beats Ready for review and Done.
        XCTAssertEqual(codeSessionFilterStatus(CodeSessionSignals(readyForReview: true, completed: true), streaming: true), .working)
        // Ready for review beats Done.
        XCTAssertEqual(codeSessionFilterStatus(CodeSessionSignals(readyForReview: true, completed: true)), .readyForReview)
        XCTAssertEqual(codeSessionFilterStatus(CodeSessionSignals(completed: true)), .done)
        XCTAssertEqual(codeSessionFilterStatus(CodeSessionSignals()), .idle)
    }

    func testInactiveSessionReadsArchived() throws {
        // iOS archives with PATCH active:false; the open lists drop it, the Archived filter shows it.
        let json = #"{"id":"ses_1","created_at":"2026-11-17T10:00:00Z","active":false,"metadata":{"originSurface":"code","needsInput":true}}"#
        let session = try JSONDecoder().decode(AgentSession.self, from: Data(json.utf8))
        XCTAssertEqual(codeSessionFilterStatus(session), .archived)
    }

    func testDecodesServerFlagsAndCompleted() throws {
        let review = #"{"id":"ses_2","created_at":"2026-11-17T10:00:00Z","metadata":{"readyForReview":true,"completed":true}}"#
        XCTAssertEqual(codeSessionFilterStatus(try JSONDecoder().decode(AgentSession.self, from: Data(review.utf8))), .readyForReview)
        let done = #"{"id":"ses_3","created_at":"2026-11-17T10:00:00Z","metadata":{"completed":true}}"#
        XCTAssertEqual(codeSessionFilterStatus(try JSONDecoder().decode(AgentSession.self, from: Data(done.utf8))), .done)
        // Missing keys mean false.
        let bare = #"{"id":"ses_4","created_at":"2026-11-17T10:00:00Z"}"#
        XCTAssertEqual(codeSessionFilterStatus(try JSONDecoder().decode(AgentSession.self, from: Data(bare.utf8)), streaming: true), .working)
    }

    func testAllHidesOnlyArchived() {
        for status in CodeSessionFilterStatus.allCases {
            XCTAssertEqual(CodeSessionFilter.all.matches(status), status != .archived, "\(status)")
        }
        XCTAssertTrue(CodeSessionFilter.archived.matches(.archived))
        XCTAssertFalse(CodeSessionFilter.done.matches(.idle))
    }

    func testFilterKeepsOrderAndCountsEveryFilter() {
        let rows: [(String, CodeSessionFilterStatus)] = [
            ("a", .working), ("b", .needsInput), ("c", .idle), ("d", .archived),
            ("e", .readyForReview), ("f", .done), ("g", .needsInput),
        ]
        let needs = filterCodeSessionRows(rows, filter: .needsInput, status: { $0.1 })
        XCTAssertEqual(needs.items.map { $0.0 }, ["b", "g"])
        let all = filterCodeSessionRows(rows, filter: .all, status: { $0.1 })
        XCTAssertEqual(all.items.map { $0.0 }, ["a", "b", "c", "e", "f", "g"])
        let counts = all.counts
        XCTAssertEqual(counts[.all], 6)
        XCTAssertEqual(counts[.needsInput], 2)
        XCTAssertEqual(counts[.readyForReview], 1)
        XCTAssertEqual(counts[.working], 1)
        XCTAssertEqual(counts[.done], 1)
        XCTAssertEqual(counts[.archived], 1)
        XCTAssertEqual(countCodeSessionFilters([])[.all], 0)
    }

    func testLabelsAndEmptyTextsMatchWeb() {
        XCTAssertEqual(CodeSessionFilter.allCases.map(\.label), ["All", "Needs input", "Ready for review", "Working", "Done", "Archived"])
        XCTAssertEqual(CodeSessionFilter.needsInput.emptyText, "No session is waiting on you.")
        XCTAssertEqual(CodeSessionFilter.all.emptyText, "No code sessions yet. Start one and it shows here.")
        XCTAssertEqual(CodeSessionFilterStatus.idle.label, "")
        // No status number or code in any UI text (C5).
        for filter in CodeSessionFilter.allCases {
            XCTAssertNil(filter.label.rangeOfCharacter(from: .decimalDigits))
            XCTAssertNil(filter.emptyText.rangeOfCharacter(from: .decimalDigits))
        }
    }

    func testFilterArgument() {
        XCTAssertEqual(CodeSessionFilter(argument: "needs_input"), .needsInput)
        XCTAssertEqual(CodeSessionFilter(argument: "ready-for-review"), .readyForReview)
        XCTAssertEqual(CodeSessionFilter(argument: "Needs input"), .needsInput)
        XCTAssertEqual(CodeSessionFilter(argument: "archived"), .archived)
        XCTAssertNil(CodeSessionFilter(argument: "nope"))
    }

    func testNewlyOnlineOnlyAfterFirstRead() {
        let now: [(String, Bool)] = [("old", true), ("new", true), ("asleep", false)]
        // No snapshot yet: never "Connected".
        XCTAssertTrue(newlyOnlineComputers(before: nil, now: now, id: { $0.0 }, isOnline: { $0.1 }).isEmpty)
        let fresh = newlyOnlineComputers(before: ["old"], now: now, id: { $0.0 }, isOnline: { $0.1 })
        XCTAssertEqual(fresh.map { $0.0 }, ["new"])
    }
}
