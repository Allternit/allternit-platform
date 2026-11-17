import BTHIDCore
import Foundation
import XCTest

final class ReportMapTests: XCTestCase {
    /// The Android sender's descriptor (HidReports.DESCRIPTOR, PR #2490). Both senders must
    /// describe the same device, so the Desktop core treats them the same.
    static let androidDescriptor: [UInt8] = [
        0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x85, 0x01, 0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15, 0x00,
        0x25, 0x01, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02, 0x95, 0x01, 0x75, 0x08, 0x81, 0x01, 0x95, 0x05,
        0x75, 0x01, 0x05, 0x08, 0x19, 0x01, 0x29, 0x05, 0x91, 0x02, 0x95, 0x01, 0x75, 0x03, 0x91, 0x01,
        0x95, 0x06, 0x75, 0x08, 0x15, 0x00, 0x25, 0x65, 0x05, 0x07, 0x19, 0x00, 0x29, 0x65, 0x81, 0x00,
        0xC0,
        0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x85, 0x02, 0x09, 0x01, 0xA1, 0x00, 0x05, 0x09, 0x19, 0x01,
        0x29, 0x03, 0x15, 0x00, 0x25, 0x01, 0x95, 0x03, 0x75, 0x01, 0x81, 0x02, 0x95, 0x01, 0x75, 0x05,
        0x81, 0x01, 0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x09, 0x38, 0x15, 0x81, 0x25, 0x7F, 0x75, 0x08,
        0x95, 0x03, 0x81, 0x06, 0xC0, 0xC0,
    ]

    func testSameBytesAsTheAndroidSender() {
        XCTAssertEqual(HIDReportMap.bytes, Self.androidDescriptor)
    }

    /// Walks the short items (HID 1.11 §6.2.2.2): collections balance, and the input bits per
    /// report ID add up to the report sizes the encoder writes.
    func testInputReportSizesMatchTheEncoder() {
        var bits: [UInt8: Int] = [:]
        var size = 0, count = 0, id: UInt8 = 0, depth = 0, i = 0
        let b = HIDReportMap.bytes
        while i < b.count {
            let prefix = b[i]
            let len = Int(prefix & 0x03)
            let data = len > 0 ? Int(b[i + 1]) : 0
            switch prefix & 0xFC {
            case 0x74: size = data
            case 0x94: count = data
            case 0x84: id = UInt8(data)
            case 0xA0: depth += 1
            case 0xC0: depth -= 1
            case 0x80: bits[id, default: 0] += size * count
            default: break
            }
            i += 1 + len
        }
        XCTAssertEqual(depth, 0)
        XCTAssertEqual(bits[HIDReportID.keyboard], HIDReports.keyboardSize * 8)
        XCTAssertEqual(bits[HIDReportID.mouse], HIDReports.mouseSize * 8)
    }
}

final class ReportTests: XCTestCase {
    func testKeyboardReportLayout() {
        XCTAssertEqual([UInt8](HIDReports.keyboard(modifiers: Modifier.cmd, keys: [0x2C])), [0x08, 0, 0x2C, 0, 0, 0, 0, 0])
        // A seventh key is dropped, not written over the end.
        XCTAssertEqual([UInt8](HIDReports.keyboard(modifiers: 0, keys: [1, 2, 3, 4, 5, 6, 7])), [0, 0, 1, 2, 3, 4, 5, 6])
    }

    func testMouseDeltasAreClampedAndSigned() {
        XCTAssertEqual([UInt8](HIDReports.mouse(buttons: 1, dx: 500, dy: -500, wheel: -3)), [1, 0x7F, 0x81, 0xFD])
        XCTAssertEqual([UInt8](HIDReports.mouse(buttons: 0xFF, dx: 0, dy: 0, wheel: 0)), [0x07, 0, 0, 0])
    }

    func testMovesSplitIntoStepsThatAddUpExactly() {
        let steps = HIDReports.moveSteps(dx: 300, dy: -50)
        XCTAssertEqual(steps.count, 3)
        XCTAssertEqual(steps.map(\.0).reduce(0, +), 300)
        XCTAssertEqual(steps.map(\.1).reduce(0, +), -50)
        XCTAssertTrue(steps.allSatisfy { abs($0.0) <= 127 && abs($0.1) <= 127 })
        XCTAssertTrue(HIDReports.moveSteps(dx: 0, dy: 0).isEmpty)
    }

    func testScrollDownIsANegativeWheel() {
        let s = HIDInputState()
        XCTAssertEqual(s.scroll(lines: 3), [.mouse(Data([0, 0, 0, 0xFD]))])
        XCTAssertEqual(s.scroll(lines: -200).count, 2) // 127 + 73
        XCTAssertTrue(s.scroll(lines: 0).isEmpty)
    }

    func testCornerResetGoesPastTheCoresReach() {
        let r = HIDInputState().cornerReset()
        XCTAssertGreaterThan(r.count * HIDReports.maxStep, 20_000)
        XCTAssertEqual(r.first, .mouse(Data([0, 0x81, 0x81, 0])))
    }

    func testHeldButtonRidesAlongOnMovesUntilReleased() {
        var s = HIDInputState()
        XCTAssertEqual(s.button(.left, down: true, up: false), [.mouse(Data([1, 0, 0, 0]))])
        XCTAssertEqual(s.move(dx: 10, dy: 0), [.mouse(Data([1, 10, 0, 0]))])
        XCTAssertEqual(s.button(.left, down: false, up: true), [.mouse(Data([0, 0, 0, 0]))])
        // A click is press then release.
        XCTAssertEqual(s.button(.right, down: true, up: true), [.mouse(Data([2, 0, 0, 0])), .mouse(Data([0, 0, 0, 0]))])
    }

    func testKeyDownUpAndReleaseAll() throws {
        var s = HIDInputState()
        XCTAssertEqual(try s.key(usage: 0x2C, modifiers: 8, down: true), [.keyboard(Data([8, 0, 0x2C, 0, 0, 0, 0, 0]))])
        XCTAssertEqual(try s.key(usage: 0x2C, modifiers: 8, down: false), [.keyboard(Data(count: 8))])
        _ = try s.key(usage: 0x04, modifiers: 0, down: true)
        XCTAssertEqual(try s.key(usage: 0, modifiers: 0, down: false), [.keyboard(Data(count: 8))])
        XCTAssertThrowsError(try s.key(usage: 0xE0, modifiers: 0, down: true))
        XCTAssertThrowsError(try s.key(usage: 4, modifiers: 256, down: true))
    }

    func testTypingPressesAndReleasesEachCharacter() {
        var s = HIDInputState()
        let out = s.type("Hi!")
        XCTAssertEqual(out, [
            .keyboard(Data([2, 0, 0x0B, 0, 0, 0, 0, 0])), .keyboard(Data(count: 8)),
            .keyboard(Data([0, 0, 0x0C, 0, 0, 0, 0, 0])), .keyboard(Data(count: 8)),
            .keyboard(Data([2, 0, 0x1E, 0, 0, 0, 0, 0])), .keyboard(Data(count: 8)),
        ])
        XCTAssertNil(s.type("café"))
        XCTAssertNil(s.type("a\nb"))
    }

    func testUsLayoutMatchesTheDesktopTable() {
        XCTAssertEqual(USLayout.stroke("0"), KeyStroke(usage: 0x27, modifiers: 0))
        XCTAssertEqual(USLayout.stroke("9"), KeyStroke(usage: 0x26, modifiers: 0))
        XCTAssertEqual(USLayout.stroke("?"), KeyStroke(usage: 0x38, modifiers: Modifier.shift))
        XCTAssertEqual(USLayout.stroke(" "), KeyStroke(usage: 0x2C, modifiers: 0))
        XCTAssertEqual(USLayout.untypable("ok é"), "é")
    }
}

final class GATTLayoutTests: XCTestCase {
    func testHidServiceUsesTheFullBaseUUID() {
        XCTAssertEqual(GATTLayout.hid.uuid, "00001812-0000-1000-8000-00805F9B34FB")
        XCTAssertEqual(bluetoothBaseUUID(0x2A4D), "00002A4D-0000-1000-8000-00805F9B34FB")
        XCTAssertTrue(GATTLayout.hid.required)
        XCTAssertEqual(GATTLayout.services.first, GATTLayout.hid)
    }

    func testEveryReportHasItsReferenceAndNeedsEncryption() {
        let reports = GATTLayout.hid.characteristics.filter { $0.uuid == bluetoothBaseUUID(GATTNumber.report) }
        XCTAssertEqual(reports.map(\.reportReference), [[1, 1], [2, 1], [1, 2]])
        XCTAssertTrue(reports.allSatisfy(\.encrypted))
        XCTAssertEqual(reports.filter(\.notify).map(\.role), [.keyboardInput, .mouseInput])
        // The Report Map read is what makes the host bond.
        XCTAssertTrue(GATTLayout.hid.characteristics.first { $0.role == .reportMap }!.encrypted)
    }

    func testFixedValues() {
        XCTAssertEqual(GATTLayout.fixedValue(.hidInformation), [0x11, 0x01, 0x00, 0x02])
        XCTAssertEqual(GATTLayout.fixedValue(.reportMap), HIDReportMap.bytes)
        XCTAssertEqual(GATTLayout.fixedValue(.pnpID)?.count, 7)
        XCTAssertNil(GATTLayout.fixedValue(.keyboardInput))
    }

    func testLongReadsByOffset() {
        let v: [UInt8] = [1, 2, 3]
        XCTAssertEqual(readSlice(v, offset: 0), .value([1, 2, 3]))
        XCTAssertEqual(readSlice(v, offset: 2), .value([3]))
        XCTAssertEqual(readSlice(v, offset: 3), .value([]))
        XCTAssertEqual(readSlice(v, offset: 4), .invalidOffset)
    }

    func testRadioStatesBecomeSentences() {
        XCTAssertNil(radioError(.on))
        XCTAssertEqual(radioError(.off)?.code, "bluetooth_off")
        XCTAssertEqual(radioError(.unauthorized)?.code, "unauthorized")
        XCTAssertEqual(radioError(nil)?.code, "bluetooth_unknown")
        XCTAssertTrue(BTHIDError.refused("").message.contains("Apple may have changed this"))
    }
}
