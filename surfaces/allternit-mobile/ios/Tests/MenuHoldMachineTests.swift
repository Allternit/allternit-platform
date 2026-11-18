import XCTest
@testable import Allternit

/// ☰ menu hold (phone calm step 24, contract C8, D4, D11). Port of the web
/// `menu-hold.test.ts` (ai #1149) so both surfaces keep the same rules,
/// including the random-walk invariant: the fan / shelf and the drawer are
/// never open together.
final class MenuHoldMachineTests: XCTestCase {
    private func run(_ events: [MenuHoldEvent], from: MenuHoldState = .initial) -> MenuHoldResult {
        var state = from
        var effects: [MenuHoldEffect] = []
        for event in events {
            let next = reduceMenuHold(state, event)
            state = next.state
            effects += next.effects
        }
        return MenuHoldResult(state: state, effects: effects)
    }

    private func drawerOpenState() -> MenuHoldState {
        var s = MenuHoldState.initial
        s.drawerOpen = true
        return s
    }

    private func lockedState() -> MenuHoldState {
        var s = MenuHoldState.initial
        s.lockedMode = true
        return s
    }

    private let toShelf: [MenuHoldEvent] = [.press, .holdTimer, .move(.mode(.bots)), .botsHoldTimer]

    // MARK: - Tap and hold on ☰

    func testATapOpensTheDrawerAndNeverTheFan() {
        let r = run([.press, .release])
        XCTAssertTrue(r.state.drawerOpen)
        XCTAssertEqual(r.state.overlay, .none)
        XCTAssertTrue(r.effects.contains(.openDrawer))
    }

    func testAHoldOpensTheFanAndReleaseOnACardSwitchesMode() {
        let held = run([.press, .holdTimer])
        XCTAssertEqual(held.state.overlay, .fan)
        XCTAssertTrue(held.effects.contains(.haptic(.medium)))
        let r = run([.move(.mode(.code)), .release], from: held.state)
        XCTAssertEqual(r.state.overlay, .none)
        XCTAssertFalse(r.state.drawerOpen)
        XCTAssertTrue(r.effects.contains(.switchMode(.code)))
        XCTAssertTrue(r.effects.contains(.haptic(.light)))
        XCTAssertTrue(r.effects.contains(.haptic(.selection)))
    }

    func testSelectionHapticFiresOncePerCardChangeNotPerMove() {
        let r = run([.press, .holdTimer, .move(.mode(.home)), .move(.mode(.home)), .move(.mode(.code))])
        XCTAssertEqual(r.effects.filter { $0 == .haptic(.selection) }.count, 2)
    }

    func testAHoldReleasedOnNothingKeepsTheFanOpenForATap() {
        let kept = run([.press, .holdTimer, .release])
        XCTAssertEqual(kept.state.overlay, .fan)
        XCTAssertFalse(kept.effects.contains(.openDrawer))
        let picked = run([.pick(.mode(.aci))], from: kept.state)
        XCTAssertEqual(picked.state.overlay, .none)
        XCTAssertTrue(picked.effects.contains(.switchMode(.aci)))
    }

    func testATapOutsideClosesTheKeptOpenFan() {
        XCTAssertEqual(run([.press, .holdTimer, .release, .outsideTap]).state.overlay, .none)
    }

    func testMovingBeforeTheHoldFiresCancels() {
        let r = run([.press, .cancel, .holdTimer, .release])
        XCTAssertEqual(r.state.overlay, .none)
        XCTAssertFalse(r.state.drawerOpen)
        XCTAssertFalse(r.effects.contains(.openDrawer))
    }

    // MARK: - The drawer and the fan are never both open

    func testAHoldIsIgnoredWhileTheDrawerIsOpenAndTheReleaseClosesIt() {
        let r = run([.press, .holdTimer, .release], from: drawerOpenState())
        XCTAssertEqual(r.state.overlay, .none)
        XCTAssertFalse(r.effects.contains(.startHoldTimer))
        XCTAssertTrue(r.effects.contains(.closeDrawer))
    }

    func testOpeningTheDrawerClosesTheFanAndTheShelf() {
        let fan = run([.press, .holdTimer, .release]).state
        let afterFan = run([.drawerOpened], from: fan).state
        XCTAssertEqual(afterFan.overlay, .none)
        XCTAssertTrue(afterFan.drawerOpen)
        let shelf = run(toShelf).state
        XCTAssertEqual(shelf.overlay, .shelf)
        XCTAssertEqual(run([.drawerOpened], from: shelf).state.overlay, .none)
    }

    func testD4DrawerModeIconClosesTheDrawerThenOpensTheFan() {
        let r = run([.openFan], from: drawerOpenState())
        XCTAssertEqual(r.state.overlay, .fan)
        XCTAssertFalse(r.state.drawerOpen)
        XCTAssertTrue(r.state.openedByAction)
        XCTAssertTrue(r.effects.contains(.closeDrawer))
    }

    func testNoSequenceOfEventsLeavesBothOpen() {
        let events: [MenuHoldEvent] = [
            .press, .release, .cancel, .holdTimer, .botsHoldTimer,
            .move(.mode(.bots)), .move(.bot("b1")), .move(nil),
            .pick(.mode(.home)), .openFan, .openShelf, .drawerOpened, .drawerClosed, .outsideTap,
        ]
        // Deterministic pseudo-random walks (the web test's LCG).
        var seed: Int64 = 7
        func nextIndex() -> Int {
            seed = (seed * 1_103_515_245 + 12_345) % 2_147_483_648
            return Int(seed % Int64(events.count))
        }
        for walk in 0..<300 {
            var state = MenuHoldState.initial
            state.lockedMode = walk % 5 == 0
            for _ in 0..<40 {
                state = reduceMenuHold(state, events[nextIndex()]).state
                XCTAssertFalse(state.overlay != .none && state.drawerOpen, "walk \(walk): overlay \(state.overlay) with the drawer open")
            }
        }
    }

    func testTheViewNeverShowsAnOverlayWhileTheDrawerIsOpen() {
        XCTAssertEqual(menuOverlayVisible(overlay: .fan, drawerOpen: true), .none)
        XCTAssertEqual(menuOverlayVisible(overlay: .shelf, drawerOpen: false), .shelf)
    }

    // MARK: - Bots shelf

    func testStayingOnBotsStartsTheTimerAndLeavingClearsIt() {
        let on = run([.press, .holdTimer, .move(.mode(.bots))])
        XCTAssertTrue(on.effects.contains(.startBotsHoldTimer))
        let off = run([.move(.mode(.code))], from: on.state)
        XCTAssertTrue(off.effects.contains(.clearBotsHoldTimer))
        XCTAssertEqual(run([.botsHoldTimer], from: off.state).state.overlay, .fan)
    }

    func testTheTimerOnBotsOpensTheShelfAndReleaseOnABotOpensItsMainChat() {
        let shelf = run(toShelf)
        XCTAssertEqual(shelf.state.overlay, .shelf)
        XCTAssertNil(shelf.state.target)
        let r = run([.move(.bot("drop")), .release], from: shelf.state)
        XCTAssertEqual(r.state.overlay, .none)
        XCTAssertTrue(r.effects.contains(.openBot("drop")))
        XCTAssertFalse(r.effects.contains { if case .switchMode = $0 { return true } else { return false } })
    }

    func testReleasingOnBotsBeforeTheShelfOpensSwitchesToBotsMode() {
        let r = run([.press, .holdTimer, .move(.mode(.bots)), .release])
        XCTAssertTrue(r.effects.contains(.switchMode(.bots)))
    }

    func testTheShelfIgnoresModeTargetsAndTheFanIgnoresBotTargets() {
        let shelf = run(toShelf).state
        XCTAssertNil(run([.move(.mode(.code))], from: shelf).state.target)
        let fan = run([.press, .holdTimer]).state
        XCTAssertNil(run([.move(.bot("x"))], from: fan).state.target)
    }

    func testLockedModeHoldGoesStraightToTheShelf() {
        XCTAssertEqual(run([.press, .holdTimer], from: lockedState()).state.overlay, .shelf)
        XCTAssertEqual(run([.openFan], from: lockedState()).state.overlay, .shelf)
    }

    func testCreateABotIsAShelfTarget() {
        let r = run(toShelf + [.move(.bot(MenuHold.createBotId)), .release])
        XCTAssertTrue(r.effects.contains(.openBot(MenuHold.createBotId)))
    }

    /// iOS only: the ☰ VoiceOver action "Open your bots".
    func testOpenShelfOpensTheShelfAndClosesTheDrawer() {
        let r = run([.openShelf], from: drawerOpenState())
        XCTAssertEqual(r.state.overlay, .shelf)
        XCTAssertFalse(r.state.drawerOpen)
        XCTAssertTrue(r.effects.contains(.closeDrawer))
    }

    // MARK: - Target keys and shelf helpers

    func testTargetKeysRoundTripAndRejectJunk() {
        XCTAssertEqual(parseTargetKey(targetKey(.mode(.aci))), .mode(.aci))
        XCTAssertEqual(parseTargetKey(targetKey(.bot("a:b"))), .bot("a:b"))
        XCTAssertNil(parseTargetKey("mode:chat"))
        XCTAssertNil(parseTargetKey("nope"))
        XCTAssertNil(parseTargetKey("bot:"))
    }

    func testDockMagnifyValues() {
        XCTAssertEqual(shelfMagnify(distance: 0, reduced: false).scale, 1.55)
        XCTAssertEqual(shelfMagnify(distance: 1, reduced: false).scale, 1.15)
        XCTAssertEqual(shelfMagnify(distance: 2, reduced: false).scale, 1)
        XCTAssertEqual(shelfMagnify(distance: 0, reduced: true).scale, 1, "reduced motion: no magnify")
        XCTAssertEqual(shelfMagnify(distance: nil, reduced: false).scale, 1)
    }

    func testShelfLabelsHaveNoCodes() {
        XCTAssertEqual(shelfPositionLabel(name: "Drop", index: 1, count: 6), "Drop · 2 of 6")
        XCTAssertEqual(botShelfStatusLine(waiting: true, working: true, activityLabel: "Running", offline: false, isMain: false), "Waiting for you")
        XCTAssertEqual(botShelfStatusLine(waiting: false, working: true, activityLabel: "Running", offline: false, isMain: true), "Main bot · Working · Running")
        XCTAssertEqual(botShelfStatusLine(waiting: false, working: false, activityLabel: nil, offline: true, isMain: false), "Offline")
        XCTAssertEqual(botShelfStatusLine(waiting: false, working: false, activityLabel: nil, offline: false, isMain: false), "Idle")
        XCTAssertEqual(menuBadgeText(12), "9+")
    }

    func testShelfColumnReadsByColumnWithSlack() {
        let shelf = CGRect(x: 12, y: 100, width: 12 * 2 + 50 * 4, height: 64)
        XCTAssertEqual(shelfColumn(at: CGPoint(x: 30, y: 130), shelf: shelf, inset: 12, count: 4), 0)
        XCTAssertEqual(shelfColumn(at: CGPoint(x: 12 + 12 + 125, y: 130), shelf: shelf, inset: 12, count: 4), 2)
        // A finger that drifts above the glass still scrubs; far away does not.
        XCTAssertEqual(shelfColumn(at: CGPoint(x: 12 + 12 + 175, y: 40), shelf: shelf, inset: 12, count: 4), 3)
        XCTAssertNil(shelfColumn(at: CGPoint(x: 400, y: 130), shelf: shelf, inset: 12, count: 4))
        XCTAssertNil(shelfColumn(at: CGPoint(x: 30, y: 0), shelf: shelf, inset: 12, count: 4))
    }
}
