import XCTest
@testable import Allternit

/// Port of the web `overview-select.test.ts` (ai #1151).
final class OverviewSelectTests: XCTestCase {
    private func started() -> OverviewSelectState { reduceOverviewSelect(INITIAL_OVERVIEW_SELECT, .start) }

    func testToggleOnlyWhileSelecting() {
        XCTAssertEqual(reduceOverviewSelect(INITIAL_OVERVIEW_SELECT, .toggle("a")), INITIAL_OVERVIEW_SELECT)
        let s = reduceOverviewSelect(reduceOverviewSelect(started(), .toggle("a")), .toggle("b"))
        XCTAssertEqual(s.selected, ["a", "b"])
        XCTAssertEqual(reduceOverviewSelect(s, .toggle("a")).selected, ["b"])
    }

    func testSelectAllTwiceClears() {
        let once = reduceOverviewSelect(started(), .selectAll(["a", "b"]))
        XCTAssertEqual(once.selected, ["a", "b"])
        XCTAssertEqual(reduceOverviewSelect(once, .selectAll(["a", "b"])).selected, [])
    }

    func testViewChangeDropsTicksSameViewKeeps() {
        let s = reduceOverviewSelect(started(), .toggle("a"))
        XCTAssertEqual(reduceOverviewSelect(s, .view(.all)), s)
        let p = reduceOverviewSelect(s, .view(.pinned))
        XCTAssertEqual(p.view, .pinned)
        XCTAssertTrue(p.selecting)
        XCTAssertEqual(p.selected, [])
    }

    func testVisibleDropsGoneRowsAndDoneLeaves() {
        let s = reduceOverviewSelect(reduceOverviewSelect(started(), .toggle("a")), .toggle("b"))
        XCTAssertEqual(reduceOverviewSelect(s, .visible(["b"])).selected, ["b"])
        let d = reduceOverviewSelect(s, .done)
        XCTAssertFalse(d.selecting)
        XCTAssertEqual(d.selected, [])
    }

    func testBulkPinAction() {
        XCTAssertEqual(bulkPinAction([], pinnedIds: ["a"]), .pin)
        XCTAssertEqual(bulkPinAction(["a", "b"], pinnedIds: ["a"]), .pin)
        XCTAssertEqual(bulkPinAction(["a"], pinnedIds: ["a", "b"]), .unpin)
    }

    func testLabels() {
        XCTAssertEqual(OVERVIEW_VIEW_LABEL[.archived], "Archived")
    }
}
