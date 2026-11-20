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

/// App Maps: the run-detail fold, the task-card line and the apps list states.
final class AppMapsTests: XCTestCase {
    private func step(_ n: Int, _ label: String, ok: Bool? = true, map: Bool = false) -> RunStep {
        RunStep(n: n, label: label, ok: ok, map: map)
    }

    func testConsecutiveMapStepsFoldIntoOneRow() {
        let rows = RunStepRow.fold([
            step(1, "open Settings"),
            step(2, "go to Network & internet", map: true),
            step(3, "go to Calls & SMS", map: true),
            step(4, "tap Wi-Fi calling"),
        ])
        XCTAssertEqual(rows.count, 3)
        XCTAssertEqual(rows[1], .map(first: 2, last: 3, destination: "Calls & SMS", count: 2))
        XCTAssertEqual(rows[1].mapLabel, "2–3. Went to Calls & SMS")
    }

    func testFailedMapStepNeverFoldsAndOldRunsAreUnchanged() {
        let failed = RunStepRow.fold([step(1, "go to X", map: true), step(2, "go to Y", ok: false, map: true), step(3, "go to Z", map: true)])
        XCTAssertEqual(failed.count, 3)
        let plain = [step(1, "a"), step(2, "b")]
        XCTAssertEqual(RunStepRow.fold(plain), plain.map { RunStepRow.step($0) })
        XCTAssertEqual(RunStepRow.map(first: 1, last: 1, destination: "Wi-Fi", count: 1).mapLabel, "1. Went to Wi-Fi")
    }

    func testTrailMapFlagIsParsed() {
        let steps = RunStep.list(.array([.object(["n": .number(1), "label": .string("go to X"), "map": .bool(true)])]))
        XCTAssertTrue(steps.first?.map == true)
        XCTAssertTrue(RunStep.list(any: [["n": 1, "label": "go to X"]]).first?.map == false)
    }

    func testRunMetaAndCardLine() {
        XCTAssertEqual(AppMapCopy.runMeta(mapSteps: 3, modelCalls: 2, app: "com.android.settings"), "3 from the Settings map · 2 model calls")
        XCTAssertEqual(AppMapCopy.runMeta(mapSteps: 1, modelCalls: 1, app: "Clock"), "1 from the Clock map · 1 model call")
        XCTAssertNil(AppMapCopy.runMeta(mapSteps: 0, modelCalls: 2, app: nil))
        XCTAssertNil(AppMapCopy.runMeta(mapSteps: nil, modelCalls: nil, app: nil))
        XCTAssertEqual(AppMapCopy.cardLine(mapSteps: 3, modelCalls: 2, app: "com.android.settings"), "3 steps came from the Settings map, so only 2 needed the model.")
        XCTAssertEqual(AppMapCopy.cardLine(mapSteps: 1, modelCalls: nil, app: nil), "1 step came from the app’s map.")
        XCTAssertNil(AppMapCopy.cardLine(mapSteps: 0, modelCalls: 2, app: "x"))
    }

    func testMapStateLinesAndActions() {
        let known = AppMapSummary(app: "com.android.settings", screens: 14)
        let stale = AppMapSummary(app: "com.google.android.deskclock", screens: 5, staleEdges: 3)
        XCTAssertEqual(AppMapCopy.stateLine(known), "14 screens known")
        XCTAssertEqual(AppMapCopy.stateLine(stale), "3 routes need a check")
        XCTAssertEqual(AppMapCopy.stateLine(AppMapSummary(app: "a", screens: 1, staleEdges: 1)), "1 route needs a check")
        XCTAssertEqual(AppMapCopy.stateLine(nil), "Learns as your bot uses it")
        XCTAssertEqual(AppMapCopy.stateLine(known, money: true), "Explore won’t touch money apps")
        XCTAssertEqual(AppMapCopy.stateLine(AppMapSummary(app: "a", platform: "ios", screens: 9)), "Learns as your bot uses it")
        XCTAssertEqual(AppMapCopy.exploreAction(known, money: false), "Explore again")
        XCTAssertEqual(AppMapCopy.exploreAction(stale, money: false), "Check")
        XCTAssertEqual(AppMapCopy.exploreAction(nil, money: false), "Explore")
        XCTAssertNil(AppMapCopy.exploreAction(known, money: true))
        XCTAssertNil(AppMapCopy.exploreAction(AppMapSummary(app: "a", platform: "ios", screens: 1), money: false))
        XCTAssertTrue(AppMapCopy.isMoneyApp("com.chase.sig.android"))
        XCTAssertFalse(AppMapCopy.isMoneyApp("com.android.settings"))
    }

    func testMapListDecodingDropsRowsWithoutAnApp() {
        let json = #"{"maps":[{"app":"com.android.settings","platform":"android","name":"Settings","screens":14,"stale_edges":0},{"screens":2}]}"#
        let list = AppMapSummary.list(Data(json.utf8))
        XCTAssertEqual(list.count, 1)
        XCTAssertEqual(list[0].title, "Settings")
        XCTAssertEqual(list[0].screens, 14)
    }
}
