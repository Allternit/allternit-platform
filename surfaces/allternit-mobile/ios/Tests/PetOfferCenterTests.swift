import XCTest
@testable import Allternit

/// The matcher must give every contract fixture its kind, and the gate must
/// keep offers rare: switch, visible chat, mute, cooldown, same line.
@MainActor
final class PetOfferCenterTests: XCTestCase {
    private func makeCenter() -> PetOfferCenter {
        let suite = "petoffers.tests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defaults.removePersistentDomain(forName: suite)
        return PetOfferCenter(defaults: defaults)
    }

    private func failure(_ text: String = "Stream dropped", bot: String? = "bot-1", session: String? = "s-1") -> PetOfferFailure {
        PetOfferFailure(text: text, botId: bot, sessionId: session)
    }

    func testEveryContractFixtureGetsItsKind() {
        for fx in PetOfferContract.fixtures {
            let hit = PetOfferCenter.match(text: fx.text, dialog: fx.dialog)
            XCTAssertEqual(hit?.kind, fx.expected, "fixture: \(fx.name)")
            if let rule = fx.rule { XCTAssertEqual(hit?.ruleId, rule, "fixture rule: \(fx.name)") }
        }
    }

    func testMatchedLineIsTheWholeTrimmedLineAndCut() {
        let hit = PetOfferCenter.match(text: "ok\r\n   error TS2322: bad type  \r\nnext")
        XCTAssertEqual(hit?.line, "error TS2322: bad type")
        let long = "error TS2322: " + String(repeating: "x", count: 400)
        XCTAssertEqual(PetOfferCenter.match(text: long)?.line.count, PetOfferContract.lineMaxChars)
    }

    func testOfferShownThenSwitchOffBlocksAndClears() {
        let c = makeCenter()
        XCTAssertNotNil(c.report(failure: failure()))
        XCTAssertNotNil(c.current)
        c.isEnabled = false
        XCTAssertNil(c.current)
        XCTAssertNil(c.report(failure: failure("Different"), now: Date().addingTimeInterval(9999)))
    }

    func testFailureInTheVisibleChatMakesNoOffer() {
        let c = makeCenter()
        c.visibleSessionId = "s-1"
        XCTAssertNil(c.report(failure: failure()))
        XCTAssertNotNil(c.report(failure: failure(session: "s-2")))
    }

    func testCooldownAndSameLine() {
        let c = makeCenter()
        let t0 = Date()
        XCTAssertNotNil(c.report(failure: failure("A failed"), now: t0))
        XCTAssertNil(c.report(failure: failure("B failed"), now: t0.addingTimeInterval(60)))
        let later = t0.addingTimeInterval(Double(PetOfferContract.SettingsDefault.cooldownMs) / 1000 + 1)
        XCTAssertNil(c.report(failure: failure("A failed"), now: later))
        XCTAssertNotNil(c.report(failure: failure("B failed"), now: later))
    }

    func testMuteBotAndChatSurviveInTheStore() {
        let c = makeCenter()
        c.muteBot("bot-1")
        XCTAssertTrue(c.isBotMuted("bot-1"))
        XCTAssertNil(c.report(failure: failure(bot: "bot-1", session: "s-9")))
        c.muteChat("s-5")
        XCTAssertNil(c.report(failure: failure(bot: "bot-2", session: "s-5")))
        XCTAssertNotNil(c.report(failure: failure(bot: "bot-2", session: "s-6")))
    }

    func testPrefillNamesTheLineAndNeverSends() {
        let c = makeCenter()
        let offer = c.report(failure: failure("Upstream 502\nmore detail"))!
        XCTAssertEqual(PetOfferCenter.prefill(for: offer), "This failed: Upstream 502. Find the cause and fix it.")
    }
}
