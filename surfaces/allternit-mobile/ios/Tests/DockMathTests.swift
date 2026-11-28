import XCTest
@testable import Allternit

/// Bottom room for docking titles: a page that overflows by less than the dock
/// distance must still scroll far enough to finish the dock (TestFlight
/// feedback 2026-11-27, ACI Computers: "COMPUTER" docked, "S" stuck mid-flight).
final class DockMathTests: XCTestCase {
    private let dock = PolishTokens.dock

    func testShortOverflowGetsRoomUpToTheDockDistance() {
        XCTAssertEqual(DockMath.bottomRoom(maxScroll: 100), dock - 100)
        XCTAssertEqual(DockMath.bottomRoom(maxScroll: 2), dock - 2)
        XCTAssertEqual(DockMath.bottomRoom(maxScroll: dock - 0.5), 1)
    }

    func testNoOverflowGetsNoRoomSoTheTitleStaysBig() {
        XCTAssertEqual(DockMath.bottomRoom(maxScroll: 0), 0)
        XCTAssertEqual(DockMath.bottomRoom(maxScroll: -240), 0)
        // Up to 1 pt of rounding is not overflow.
        XCTAssertEqual(DockMath.bottomRoom(maxScroll: 0.6), 0)
    }

    func testPagesThatAlreadyScrollTheDockDistanceGetNoRoom() {
        XCTAssertEqual(DockMath.bottomRoom(maxScroll: dock), 0)
        XCTAssertEqual(DockMath.bottomRoom(maxScroll: 900), 0)
    }

    func testEveryLetterLandsAtMaxScrollOnceTheRoomIsAdded() {
        let letters = 9  // "COMPUTERS"
        for maxScroll in stride(from: CGFloat(1.5), to: dock, by: 7.25) {
            let reach = maxScroll + DockMath.bottomRoom(maxScroll: maxScroll)
            let f = DockMath.frame(y: reach, n: letters, reduced: false)
            XCTAssertTrue(f.docked, "maxScroll \(maxScroll) reaches \(reach)")
            XCTAssertEqual(f.landed, letters, "maxScroll \(maxScroll)")
            XCTAssertEqual(DockMath.progress(t: f.t, n: letters, letter: letters - 1), 1,
                           "last letter still in flight at maxScroll \(maxScroll)")
        }
    }

    func testWithoutRoomTheLastLetterStaysInFlight() {
        // The bug: a page that scrolls 150 pt of a 170 pt dock leaves "S" mid-air.
        let f = DockMath.frame(y: 150, n: 9, reduced: false)
        XCTAssertFalse(f.docked)
        XCTAssertLessThan(DockMath.progress(t: f.t, n: 9, letter: 8), 1)
    }
}
