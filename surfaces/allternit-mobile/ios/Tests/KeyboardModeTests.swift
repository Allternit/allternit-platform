import BTHIDCore
import XCTest
@testable import Allternit

/// Keyboard mode (Phone Use `bt-apple`, contract §5, §9.2): what
/// `hid.pair.status` answers for each mode and Bluetooth state, and how the
/// `hid.*` commands turn into reports and typed errors Desktop maps
/// (phone-bt-apple.ts).
@MainActor
final class KeyboardModeTests: XCTestCase {
    private final class FakeLink: KeyboardLink {
        var linkState: KeyboardLinkState
        var sent: [HIDOutput] = []
        var failure: BTHIDError?

        init(_ state: KeyboardLinkState) { linkState = state }

        func send(_ outputs: [HIDOutput], timeout: Double) async throws {
            if let failure { throw failure }
            sent += outputs
        }
    }

    private static let connected = KeyboardLinkState(availability: .ready, connected: true, advertising: true, message: nil)

    private func published(subscribers: Int = 0) -> HIDPeripheralStatus {
        HIDPeripheralStatus(radio: .on, authorization: .allowed, servicesReady: true, advertising: true, subscribers: subscribers)
    }

    private func code(_ work: () async throws -> Any) async -> String? {
        do {
            _ = try await work()
            return nil
        } catch let e as AppPhoneRPCError {
            return e.code
        } catch {
            return "other: \(error)"
        }
    }

    // MARK: - State mapping

    func testOffIsUnavailableWithTheTurnOnSentence() {
        let s = KeyboardLinkState.from(mode: .off, status: HIDPeripheralStatus())
        XCTAssertEqual(s.availability, .off)
        XCTAssertEqual(s.json["available"] as? Bool, false)
        XCTAssertEqual(s.json["message"] as? String, KeyboardLinkState.offMessage)
        XCTAssertNil(s.json["needs_you"])
    }

    func testPublishedWithoutHostNeedsPairing() {
        let s = KeyboardLinkState.from(mode: .on, status: published())
        XCTAssertEqual(s.availability, .ready)
        XCTAssertEqual(s.json["available"] as? Bool, true)
        XCTAssertEqual(s.json["connected"] as? Bool, false)
        XCTAssertEqual(s.json["needs_you"] as? String, "BT_NOT_PAIRED")
    }

    func testSubscribedHostIsPairedAndConnected() {
        let s = KeyboardLinkState.from(mode: .on, status: published(subscribers: 1))
        XCTAssertEqual(s.json["paired"] as? Bool, true)
        XCTAssertEqual(s.json["connected"] as? Bool, true)
        XCTAssertNil(s.json["needs_you"])
        XCTAssertNil(s.json["message"])
    }

    func testRefusedServiceSaysNotAvailableOnThisDevice() {
        var st = published()
        st.refused = true
        XCTAssertEqual(KeyboardLinkState.from(mode: .on, status: st).availability, .refused)
        let failed = KeyboardLinkState.from(mode: .failed(.refused("")), status: HIDPeripheralStatus())
        XCTAssertEqual(failed.availability, .refused)
        XCTAssertTrue(failed.message?.hasPrefix("Not available on this iPhone or iPad") == true)
    }

    func testBluetoothOffAndPermissionWhileOn() {
        var st = published()
        st.radio = .off
        XCTAssertEqual(KeyboardLinkState.from(mode: .on, status: st).availability, .bluetoothOff)
        st.radio = .on
        st.authorization = .denied
        XCTAssertEqual(KeyboardLinkState.from(mode: .on, status: st).availability, .noPermission)
        XCTAssertEqual(KeyboardLinkState.from(mode: .failed(BTHIDError("unauthorized", "x")), status: HIDPeripheralStatus()).availability, .noPermission)
    }

    func testOnButUnpublishedIsStartingNotReady() {
        var st = published()
        st.servicesReady = false
        XCTAssertEqual(KeyboardLinkState.from(mode: .on, status: st).availability, .starting)
    }

    // MARK: - Commands

    func testStatusAnswersEvenWhenOff() async throws {
        let link = FakeLink(KeyboardLinkState.from(mode: .off, status: HIDPeripheralStatus()))
        let r = try await KeyboardCommands(link: link).handle(method: "hid.pair.status", params: [:])
        XCTAssertEqual(r["availability"] as? String, "off")
        XCTAssertEqual(r["sender"] as? String, "bt-apple")
    }

    func testInputWhenOffOrNotConnectedIsTyped() async {
        let off = KeyboardCommands(link: FakeLink(KeyboardLinkState.from(mode: .off, status: HIDPeripheralStatus())))
        let offCode = await code { try await off.handle(method: "hid.pointer.move", params: ["dx": 5, "dy": 5]) }
        XCTAssertEqual(offCode, "hid_unavailable")
        let waiting = KeyboardCommands(link: FakeLink(KeyboardLinkState.from(mode: .on, status: published())))
        let waitingCode = await code { try await waiting.handle(method: "hid.key", params: ["usage": 4, "down": true]) }
        XCTAssertEqual(waitingCode, "bt_not_paired")
    }

    func testMoveIsSplitIntoReportsThatAddUp() async throws {
        let link = FakeLink(Self.connected)
        _ = try await KeyboardCommands(link: link).handle(method: "hid.pointer.move", params: ["dx": 300, "dy": -40])
        XCTAssertEqual(link.sent.count, 3)
        var dx = 0
        var dy = 0
        for case .mouse(let d) in link.sent {
            dx += Int(Int8(bitPattern: d[1]))
            dy += Int(Int8(bitPattern: d[2]))
        }
        XCTAssertEqual(dx, 300)
        XCTAssertEqual(dy, -40)
    }

    func testClickIsPressThenReleaseAndMiddleIsRefused() async throws {
        let link = FakeLink(Self.connected)
        let c = KeyboardCommands(link: link)
        _ = try await c.handle(method: "hid.pointer.click", params: [:])
        XCTAssertEqual(link.sent, [.mouse(Data([1, 0, 0, 0])), .mouse(Data([0, 0, 0, 0]))])
        let middle = await code { try await c.handle(method: "hid.pointer.click", params: ["button": "middle"]) }
        XCTAssertEqual(middle, "invalid_params")
    }

    func testBadParamsAreRefusedBeforeSending() async {
        let link = FakeLink(Self.connected)
        let c = KeyboardCommands(link: link)
        let usage = await code { try await c.handle(method: "hid.key", params: ["usage": 0x200, "down": true]) }
        XCTAssertEqual(usage, "invalid_params")
        let down = await code { try await c.handle(method: "hid.key", params: ["usage": 4, "down": 1]) }
        XCTAssertEqual(down, "invalid_params")
        let fraction = await code { try await c.handle(method: "hid.pointer.move", params: ["dx": 1.5, "dy": 0]) }
        XCTAssertEqual(fraction, "invalid_params")
        let emoji = await code { try await c.handle(method: "hid.type", params: ["text": "hi 👋"]) }
        XCTAssertEqual(emoji, "bad_params")
        let unknown = await code { try await c.handle(method: "hid.pair", params: [:]) }
        XCTAssertEqual(unknown, "unknown_method")
        XCTAssertTrue(link.sent.isEmpty)
    }

    func testTypeAnswersSentCount() async throws {
        let link = FakeLink(Self.connected)
        let r = try await KeyboardCommands(link: link).handle(method: "hid.type", params: ["text": "Hi"])
        XCTAssertEqual(r["sent"] as? Int, 2)
        // Each character is pressed and let go.
        XCTAssertEqual(link.sent.count, 4)
    }

    func testLostHostDuringSendIsNotPaired() async {
        let link = FakeLink(Self.connected)
        link.failure = BTHIDError("not_connected", "gone")
        let c = KeyboardCommands(link: link)
        let lost = await code { try await c.handle(method: "hid.scroll", params: ["lines": 3]) }
        XCTAssertEqual(lost, "bt_not_paired")
        link.failure = BTHIDError("send_timeout", "slow")
        let slow = await code { try await c.handle(method: "hid.scroll", params: ["lines": 3]) }
        XCTAssertEqual(slow, "hid_failed")
    }

    /// Contract §9.2: a held key is let go when the lane drops.
    func testReleaseAllLetsGoOfHeldKeyOnlyWhenSomethingIsHeld() async throws {
        let link = FakeLink(Self.connected)
        let c = KeyboardCommands(link: link)
        await c.releaseAll()
        XCTAssertTrue(link.sent.isEmpty)
        _ = try await c.handle(method: "hid.key", params: ["usage": 4, "modifiers": 8, "down": true])
        link.sent = []
        await c.releaseAll()
        XCTAssertEqual(link.sent, [.keyboard(Data(count: 8)), .mouse(Data(count: 4))])
        XCTAssertEqual(c.input, HIDInputState())
    }

    /// The corner sweep must not drag: a held button is let go first.
    func testResetLetsGoOfHeldButtonBeforeTheSweep() async throws {
        let link = FakeLink(Self.connected)
        let c = KeyboardCommands(link: link)
        _ = try await c.handle(method: "hid.pointer.click", params: ["down": true, "up": false])
        link.sent = []
        _ = try await c.handle(method: "hid.pointer.reset", params: [:])
        XCTAssertEqual(link.sent.first, .mouse(Data([0, 0, 0, 0])))
        XCTAssertEqual(link.sent.count, 1 + HIDReports.cornerSteps)
        XCTAssertTrue(link.sent.dropFirst().allSatisfy { $0 == .mouse(Data([0, 0x81, 0x81, 0])) })
    }
}
