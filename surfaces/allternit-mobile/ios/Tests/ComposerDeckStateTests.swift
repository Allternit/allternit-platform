import XCTest
@testable import Allternit

/// The folded deck band's rules (phone calm pass step 21, contract C1 + D6),
/// same cases as the web's `deck-open.test.ts`.
final class ComposerDeckStateTests: XCTestCase {
    func testFoldedAtRest() {
        XCTAssertFalse(deckOpen(focused: false, hasDraft: false, swipedOpen: false, modeArmed: false))
    }

    func testEachConditionOpensIt() {
        XCTAssertTrue(deckOpen(focused: true, hasDraft: false, swipedOpen: false, modeArmed: false))
        XCTAssertTrue(deckOpen(focused: false, hasDraft: true, swipedOpen: false, modeArmed: false))
        XCTAssertTrue(deckOpen(focused: false, hasDraft: false, swipedOpen: true, modeArmed: false))
        XCTAssertTrue(deckOpen(focused: false, hasDraft: false, swipedOpen: false, modeArmed: true))
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

    func testTipShowsOnceAndNeverBesideTheChip() {
        XCTAssertTrue(ComposerSwipeTipState.shouldShow(seen: false, deckIsOpen: false, chipVisible: false, hasDeck: true))
        XCTAssertFalse(ComposerSwipeTipState.shouldShow(seen: true, deckIsOpen: false, chipVisible: false, hasDeck: true))
        XCTAssertFalse(ComposerSwipeTipState.shouldShow(seen: false, deckIsOpen: false, chipVisible: true, hasDeck: true))
        XCTAssertFalse(ComposerSwipeTipState.shouldShow(seen: false, deckIsOpen: true, chipVisible: false, hasDeck: true))
        XCTAssertFalse(ComposerSwipeTipState.shouldShow(seen: false, deckIsOpen: false, chipVisible: false, hasDeck: false))
    }

    func testTipTextMatchesWeb() {
        XCTAssertEqual(ComposerSwipeTipState.text, "Swipe the composer up for modes, plugins and artifacts")
    }
}
