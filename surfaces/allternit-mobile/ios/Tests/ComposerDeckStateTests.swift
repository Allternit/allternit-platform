import XCTest
@testable import Allternit

/// The folded deck band's rules (phone calm pass step 21, contract C1 + D6),
/// same cases as the web's `deck-open.test.ts`.
final class ComposerDeckStateTests: XCTestCase {
    func testOpenAtRest() {
        XCTAssertTrue(deckOpen(folded: false))
    }

    func testFoldedStaysFolded() {
        XCTAssertFalse(deckOpen(folded: true))
    }

    func testDefaultsKey() {
        XCTAssertEqual(deckFoldedDefaultsKey, "allternit.composer.deckFolded")
    }

    func testSwipeUp24Opens() {
        XCTAssertTrue(nextSwipedOpen(current: false, startY: 300, endY: 276))
        XCTAssertTrue(nextSwipedOpen(current: false, startY: 300, endY: 100))
    }

    func testShortSwipeChangesNothing() {
        XCTAssertFalse(nextSwipedOpen(current: false, startY: 300, endY: 277))
        XCTAssertTrue(nextSwipedOpen(current: true, startY: 300, endY: 310))
    }

    func testSwipeDown24Closes() {
        XCTAssertFalse(nextSwipedOpen(current: true, startY: 300, endY: 324))
    }
}
