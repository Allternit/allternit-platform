import XCTest
@testable import Allternit

/// The computer chip above the composer (phone calm pass step 20, contract
/// C4): which line shows for which computer state, in the contract's order,
/// and the ✕ dismissal keyed to the state (web `computerChipState` /
/// `chipShowsAsDot`).
final class ComputerChipTests: XCTestCase {
    private func state(_ input: ComputerChipInput) -> ComputerChipState { computerChipState(input) }

    // MARK: - States and copy

    func testNothingWhenAComputerAnswers() {
        XCTAssertEqual(state(ComputerChipInput(runtimeAvailable: true, paired: true)), ComputerChipState.hidden)
    }

    func testSignedOutShowsNothingEvenWithoutAComputer() {
        let input = ComputerChipInput(signedOut: true, gate: true, runtimeAvailable: false, readRefused: true)
        XCTAssertEqual(state(input).kind, ComputerChipKind.none)
    }

    func testNoComputerPairedReadsConnect() {
        let s = state(ComputerChipInput(runtimeAvailable: false, paired: false))
        XCTAssertEqual(s.kind, .connect)
        XCTAssertEqual(s.text, "No computer connected")
        XCTAssertEqual(s.actionLabel, "Connect")
        XCTAssertEqual(s.action, .connect)
    }

    func testPairedButSilentReadsFix() {
        let s = state(ComputerChipInput(runtimeAvailable: false, paired: true))
        XCTAssertEqual(s.kind, .fix)
        XCTAssertEqual(s.text, "Can't reach your computer")
        XCTAssertEqual(s.actionLabel, "Fix")
        XCTAssertEqual(s.action, .fix)
    }

    func testWakingHasProgressAndNoButton() {
        let s = state(ComputerChipInput(runtimeAvailable: false, waking: true, paired: true))
        XCTAssertEqual(s.kind, .waking)
        XCTAssertEqual(s.text, "Waking your computer…")
        XCTAssertNil(s.actionLabel)
        XCTAssertNil(s.action)
        XCTAssertTrue(s.showsProgress)
    }

    func testGateOpensTheGateSheet() {
        let s = state(ComputerChipInput(gate: true, runtimeAvailable: false))
        XCTAssertEqual(s.kind, .gate)
        XCTAssertEqual(s.text, "No computer connected")
        XCTAssertEqual(s.action, .gate)
    }

    func testReadRefusalWithNoComputerReadsConnectWhileRelayStillUnknown() {
        // Relay not fetched yet (counts as available) but a read was refused.
        let s = state(ComputerChipInput(runtimeAvailable: true, paired: false, readRefused: true))
        XCTAssertEqual(s.kind, .connect)
    }

    func testReadRefusalIgnoredOnceAComputerIsPaired() {
        XCTAssertEqual(state(ComputerChipInput(runtimeAvailable: true, paired: true, readRefused: true)), ComputerChipState.hidden)
    }

    func testRunwayOnlyWhenNothingElseIsWrong() {
        let runway = ComputeGateCopy.lowRunway2
        XCTAssertEqual(state(ComputerChipInput(runway: runway, runtimeAvailable: true, paired: true)).kind, .runway)
        XCTAssertEqual(state(ComputerChipInput(runway: runway, runtimeAvailable: false, paired: true)).kind, .fix)
    }

    // MARK: - Order (most urgent first)

    func testOrderSuspendedBeatsGateBeatsStartingBeatsWaking() {
        var input = ComputerChipInput(gate: true, suspended: true, starting: true, runtimeAvailable: false, waking: true)
        XCTAssertEqual(state(input).kind, .suspended)
        input.suspended = false
        XCTAssertEqual(state(input).kind, .gate)
        input.gate = false
        XCTAssertEqual(state(input).kind, .starting)
        input.starting = false
        XCTAssertEqual(state(input).kind, .waking)
        input.waking = false
        XCTAssertEqual(state(input).kind, .connect)
    }

    // MARK: - Calm copy

    func testNoChipTextCarriesAStatusOrCode() {
        let inputs = [
            ComputerChipInput(runtimeAvailable: false),
            ComputerChipInput(runtimeAvailable: false, paired: true),
            ComputerChipInput(waking: true),
            ComputerChipInput(gate: true),
            ComputerChipInput(suspended: true),
            ComputerChipInput(starting: true),
        ]
        for input in inputs {
            let text = state(input).text
            XCTAssertFalse(text.isEmpty)
            XCTAssertNil(text.range(of: #"\b[1-5]\d\d\b"#, options: .regularExpression), text)
            XCTAssertFalse(text.lowercased().contains("compute_required"), text)
        }
    }

    // MARK: - ✕ to a dot, keyed to the state

    func testDotOnlyForTheDismissedState() {
        let noComputer = state(ComputerChipInput(runtimeAvailable: false))
        let cantReach = state(ComputerChipInput(runtimeAvailable: false, paired: true))
        XCTAssertTrue(chipShowsAsDot(key: noComputer.key, dismissed: noComputer.key))
        // A different state brings the chip back.
        XCTAssertFalse(chipShowsAsDot(key: cantReach.key, dismissed: noComputer.key))
        XCTAssertFalse(chipShowsAsDot(key: noComputer.key, dismissed: nil))
        // Nothing to say: no dot.
        XCTAssertFalse(chipShowsAsDot(key: ComputerChipState.hidden.key, dismissed: "none"))
    }

    // MARK: - -calm-chip fixtures

    func testLaunchArgumentFixtures() {
        XCTAssertEqual(computerChipFixture("none"), ComputerChipState.hidden)
        XCTAssertEqual(computerChipFixture("noComputer")?.kind, .connect)
        XCTAssertEqual(computerChipFixture("cantReach")?.kind, .fix)
        XCTAssertEqual(computerChipFixture("waking")?.kind, .waking)
        XCTAssertEqual(computerChipFixture("gate")?.kind, .gate)
        XCTAssertNil(computerChipFixture("bogus"))
    }
}
