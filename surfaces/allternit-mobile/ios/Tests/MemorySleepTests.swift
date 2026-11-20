import XCTest
@testable import Allternit

/// Settings → Memory → Sleep: what the section leads with, how a failed call
/// reads, and the report lines. Same rules and sentences as web
/// `memory-sleep-client.ts` (allternit-ai #1191).
final class MemorySleepTests: XCTestCase {
    private func status(_ json: String) throws -> SleepStatus {
        try JSONDecoder().decode(SleepStatus.self, from: Data(json.utf8))
    }

    private func onStatus(lastState: String? = nil, debt: Double = 0) throws -> SleepStatus {
        let last = lastState.map { #","last_cycle":{"id":"c1","trigger":"idle","kind":"nap","state":"\#($0)","spend_usd":0,"started_at":"2026-11-19 10:00:00"}"# } ?? ""
        return try status(#"{"available":true,"enabled":true,"stages":{"orchestrator":true},"debt":\#(debt)\#(last)}"#)
    }

    // MARK: Decoding

    func testStatusFromAPartialServerDecodesWithDefaults() throws {
        let s = try status(#"{"available":true}"#)
        XCTAssertFalse(s.isEnabled)
        XCTAssertEqual(s.dailyCap, 0)
        XCTAssertEqual(MemorySleep.shown(s, disputesNeedingYou: 0), .off)
    }

    func testStageReportWithANonNumberCountStillDecodes() throws {
        let json = #"{"cycle_id":"c1","spend_usd":0,"stages":[{"stage":"consolidation","outcome":"ran","counts":{"merged":3,"note":"x"}}]}"#
        let report = try JSONDecoder().decode(SleepCycleReport.self, from: Data(json.utf8))
        XCTAssertEqual(MemorySleep.reportLines(report), ["Merged 3 repeated memories", "No paid steps"])
    }

    func testSettingsUpdateSendsOnlyWhatItNames() throws {
        let data = try JSONEncoder().encode(SleepSettingsUpdate(enabled: false))
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(Set(object.keys), ["enabled"])
    }

    // MARK: What the section leads with

    func testEnabledWithoutIdleCyclesIsOff() throws {
        let s = try status(#"{"available":true,"enabled":true,"stages":{"orchestrator":false}}"#)
        XCTAssertEqual(MemorySleep.shown(s, disputesNeedingYou: 2), .off)
    }

    func testOrderRunningThenConflictsThenFailedThenDebtThenReport() throws {
        XCTAssertEqual(MemorySleep.shown(try onStatus(lastState: "running"), disputesNeedingYou: 1), .sleeping)
        XCTAssertEqual(MemorySleep.shown(try onStatus(lastState: "failed"), disputesNeedingYou: 1), .disputes)
        XCTAssertEqual(MemorySleep.shown(try onStatus(lastState: "failed", debt: 3), disputesNeedingYou: 0), .error)
        XCTAssertEqual(MemorySleep.shown(try onStatus(lastState: "finished", debt: 1), disputesNeedingYou: 0), .debt)
        XCTAssertEqual(MemorySleep.shown(try onStatus(lastState: "finished"), disputesNeedingYou: 0), .report)
        XCTAssertEqual(MemorySleep.shown(try onStatus(), disputesNeedingYou: 0), .empty)
    }

    // MARK: Failed calls

    func testA404OnStatusMeansTheRuntimeIsTooOld() {
        let err = MemorySleepError.from(status: 404, data: Data(#"{"error":"Not Found"}"#.utf8))
        XCTAssertEqual(MemorySleep.unavailable(from: err), .oldRuntime)
    }

    func testAStatusAnswerThatIsNotSleepMeansTheRuntimeIsTooOld() {
        let decoding = APIError.decoding(DecodingError.dataCorrupted(.init(codingPath: [], debugDescription: "html")))
        XCTAssertEqual(MemorySleep.unavailable(from: decoding), .oldRuntime)
    }

    func testRelayAnswersSortIntoNoComputerAndOffline() {
        let none = MemorySleepError.from(status: 503, data: Data(#"{"error":"runtime_unavailable"}"#.utf8))
        XCTAssertEqual(MemorySleep.unavailable(from: none), .noComputer)
        XCTAssertEqual(none.message, "Memory still works. Sleep runs on your computer: open Allternit Desktop or pair one to use it.")
        let asleep = MemorySleepError.from(status: 503, data: Data(#"{"error":"runtime_offline"}"#.utf8))
        XCTAssertEqual(MemorySleep.unavailable(from: asleep), .computerOffline)
    }

    func testNoPairedRuntimeOnThePhoneIsNoComputer() throws {
        let routeRefusal = APIError.httpError(statusCode: 503, message: "No paired Allternit runtime.")
        XCTAssertEqual(try MemorySleepError.from(transport: routeRefusal).kind, .noComputer)
        XCTAssertEqual(try MemorySleepError.from(transport: RuntimeRelayError.unreachable).kind, .computerOffline)
    }

    func testARefusedStageReadsInOurWords() {
        let body = #"{"accepted":false,"reason":"sleep_disabled","stage":"disputes","error":"stage disputes is off"}"#
        let err = MemorySleepError.from(status: 409, data: Data(body.utf8))
        XCTAssertEqual(err.kind, .refused)
        XCTAssertEqual(err.stage, "disputes")
        XCTAssertEqual(err.message, "That part of Sleep is off. Turn it on under \"What Sleep may do\".")
    }

    func testUndoBlockedByLaterEditsListsTheFiles() {
        let body = #"{"undone":false,"conflicts":["notes/home.md","people/sam.md"],"error":"You edited lines the Dream changed after it ran, so this cycle was not undone. Nothing was changed."}"#
        let err = MemorySleepError.from(status: 409, data: Data(body.utf8))
        XCTAssertEqual(err.kind, .conflict)
        XCTAssertEqual(err.conflicts, ["notes/home.md", "people/sam.md"])
        XCTAssertTrue(err.message.hasPrefix("You edited lines the Dream changed"))
    }

    func testAlreadyUndoneIsKnownByItsReason() {
        let err = MemorySleepError.from(status: 409, data: Data(#"{"undone":false,"error":"already undone"}"#.utf8))
        XCTAssertEqual(err.reason, "already_undone")
    }

    // MARK: Report

    func testReportLinesSaySkippedForTheLimitAndWhatItCost() throws {
        let json = #"{"spend_usd":0.004,"stages":[{"stage":"dream","outcome":"skipped","reason":"spend_cap"},{"stage":"capture","outcome":"ran","counts":{"captured":2}}]}"#
        let report = try JSONDecoder().decode(SleepCycleReport.self, from: Data(json.utf8))
        XCTAssertEqual(MemorySleep.reportLines(report, openConflicts: 1), [
            "Skipped dreaming: daily spend limit",
            "Saved 2 things that were still open",
            "1 conflict waiting for you",
            "Spent <$0.01",
        ])
    }

    func testCannotUndoNotesReadAsSentences() {
        XCTAssertEqual(MemorySleep.cannotUndoLine("consolidation: merges are kept"), "Merged and faded memories stay as they are.")
        XCTAssertEqual(MemorySleep.cannotUndoLine("rem: new links stay"), "New links stay.")
    }

    func testAnUndoneOrRunningCycleHasNoUndo() throws {
        let done = try XCTUnwrap(try onStatus(lastState: "finished").lastCycle)
        XCTAssertTrue(MemorySleep.canUndo(done, report: nil))
        let running = try XCTUnwrap(try onStatus(lastState: "running").lastCycle)
        XCTAssertFalse(MemorySleep.canUndo(running, report: nil))
        let blocked = SleepCycleReport(undo: SleepUndoInfo(available: false))
        XCTAssertFalse(MemorySleep.canUndo(done, report: blocked))
    }
}
