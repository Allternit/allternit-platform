import CoreGraphics
import XCTest
@testable import MirrorCore

final class ProtocolTests: XCTestCase {
    func testParsesEachCommand() {
        XCTAssertEqual(try parseRequest(#"{"id":"1","cmd":"tap","x":10,"y":20.5}"#).result.get(), .tap(x: 10, y: 20.5))
        XCTAssertEqual(try parseRequest(#"{"id":"2","cmd":"type","text":"hi\nthere"}"#).result.get(), .type(text: "hi\nthere"))
        XCTAssertEqual(try parseRequest(#"{"cmd":"swipe","x1":1,"y1":2,"x2":3,"y2":4}"#).result.get(), .swipe(x1: 1, y1: 2, x2: 3, y2: 4, durationMs: 300))
        XCTAssertEqual(try parseRequest(#"{"cmd":"swipe","direction":"up","duration_ms":500}"#).result.get(), .swipeDirection(.up, durationMs: 500))
        XCTAssertEqual(try parseRequest(#"{"cmd":"probe","ocr":true}"#).result.get(), .probe(ocr: true))
        XCTAssertEqual(try parseRequest(#"{"cmd":"capture"}"#).result.get(), .capture(png: true, gray: false))
        XCTAssertEqual(try parseRequest(#"{"cmd":"key","combo":"cmd+space"}"#).result.get(), .key(combo: "cmd+space"))
        XCTAssertEqual(try parseRequest(#"{"cmd":"home"}"#).result.get(), .home)
        XCTAssertEqual(try parseRequest(#"{"cmd":"back"}"#).result.get(), .back)
    }

    func testKeepsTheIdForErrorsAndAcceptsNumericIds() {
        let r = parseRequest(#"{"id":7,"cmd":"nope"}"#)
        XCTAssertEqual(r.id, "7")
        XCTAssertEqual(r.result, .failure(MirrorError("unknown_command", "Unknown command nope.")))
    }

    func testRefusesBadInput() {
        func code(_ line: String) -> String? {
            if case .failure(let e) = parseRequest(line).result { return e.code }
            return nil
        }
        XCTAssertEqual(code("not json"), "bad_request")
        XCTAssertEqual(code(#"{"cmd":"tap","x":1}"#), "bad_request")
        XCTAssertEqual(code(#"{"cmd":"tap","x":true,"y":2}"#), "bad_request")
        XCTAssertEqual(code(#"{"cmd":"tap","x":"1","y":2}"#), "bad_request")
        XCTAssertEqual(code(#"{"cmd":"type","text":""}"#), "bad_request")
        XCTAssertEqual(code(#"{"cmd":"type","text":"a\u0007b"}"#), "bad_request")
        XCTAssertEqual(code("{\"cmd\":\"type\",\"text\":\"\(String(repeating: "a", count: 501))\"}"), "bad_request")
        XCTAssertEqual(code(#"{"cmd":"swipe","direction":"sideways"}"#), "bad_request")
        XCTAssertEqual(code(#"{"cmd":"swipe","x1":1,"y1":1,"x2":2,"y2":2,"duration_ms":10}"#), "bad_request")
        XCTAssertEqual(code(#"{"cmd":"key","combo":"cmd+shift"}"#), "bad_request")
        XCTAssertEqual(code(#"{"cmd":"key","combo":"cmd+a+b"}"#), "bad_request")
    }

    func testInputCommandsAreMarkedAndAllHaveTimeLimits() {
        XCTAssertTrue(Command.tap(x: 1, y: 1).isInput)
        XCTAssertFalse(Command.read.isInput)
        XCTAssertGreaterThan(Command.type(text: String(repeating: "a", count: 500)).timeLimit, Command.type(text: "a").timeLimit)
        for c: Command in [.version, .probe(ocr: true), .capture(png: true, gray: true), .read, .tap(x: 0, y: 0), .home, .back] {
            XCTAssertGreaterThan(c.timeLimit, 0)
            XCTAssertLessThanOrEqual(c.timeLimit, 30)
        }
    }

    func testReplyCarriesIdAndErrorFields() {
        let s = errorReply(id: "9", MirrorError("permission_missing", "x", extra: ["pane": "P"]))
        XCTAssertEqual(s, #"{"error":"permission_missing","id":"9","message":"x","ok":false,"pane":"P"}"#)
    }
}

final class GeometryTests: XCTestCase {
    // A 300×650 pt window at (100, 50) on a 2× display: captures are 600×1300 px.
    let g = WindowGeometry(bounds: CGRect(x: 100, y: 50, width: 300, height: 650), scale: 2)

    func testMapsPixelsToScreenPoints() throws {
        XCTAssertEqual(g.pixelWidth, 600)
        XCTAssertEqual(g.pixelHeight, 1300)
        XCTAssertEqual(try g.screenPoint(x: 0, y: 0), CGPoint(x: 100, y: 50))
        XCTAssertEqual(try g.screenPoint(x: 300, y: 650), CGPoint(x: 250, y: 375))
        XCTAssertEqual(try g.screenPoint(x: 599, y: 1299), CGPoint(x: 399.5, y: 699.5))
    }

    func testRefusesPointsOutsideTheWindow() {
        for (x, y) in [(-1.0, 10.0), (10, -0.5), (600, 10), (10, 1300), (5000, 5000), (Double.nan, 1), (1, Double.infinity)] {
            XCTAssertThrowsError(try g.screenPoint(x: x, y: y)) { error in
                XCTAssertEqual((error as? MirrorError)?.code, "outside_window", "(\(x), \(y))")
            }
        }
    }

    func testDirectionalSwipesAndBackStayInside() throws {
        for d in [Direction.up, .down, .left, .right] {
            let s = g.swipe(d)
            XCTAssertNoThrow(try g.screenPoint(x: s.0, y: s.1))
            XCTAssertNoThrow(try g.screenPoint(x: s.2, y: s.3))
        }
        XCTAssertLessThan(g.swipe(.up).3, g.swipe(.up).1, "up moves the finger up")
        let b = g.backSwipe
        XCTAssertNoThrow(try g.screenPoint(x: b.0, y: b.1))
        XCTAssertNoThrow(try g.screenPoint(x: b.2, y: b.3))
        XCTAssertLessThan(b.0, 10)
    }

    func testVisionBoxesBecomeTopLeftPixelBoxes() {
        // Bottom-left normalized box covering the top-left quarter.
        XCTAssertEqual(g.pixelBox(normalized: CGRect(x: 0, y: 0.5, width: 0.5, height: 0.5)), [0, 0, 300, 650])
        XCTAssertEqual(g.pixelBox(normalized: CGRect(x: -0.1, y: -0.1, width: 2, height: 0.2)), [0, 1170, 600, 1300])
    }

    func testCoveringWindowIgnoresInvisibleOnes() {
        let p = CGPoint(x: 200, y: 200)
        let clear = OverlappingWindow(owner: "Clear", layer: 0, alpha: 0, bounds: CGRect(x: 0, y: 0, width: 1000, height: 1000))
        let elsewhere = OverlappingWindow(owner: "Far", layer: 0, alpha: 1, bounds: CGRect(x: 900, y: 900, width: 10, height: 10))
        let over = OverlappingWindow(owner: "Notes", layer: 0, alpha: 1, bounds: CGRect(x: 150, y: 150, width: 100, height: 100))
        XCTAssertNil(coveringWindow(at: p, above: [clear, elsewhere]))
        XCTAssertEqual(coveringWindow(at: p, above: [clear, over])?.owner, "Notes")
    }
}

final class HealthTests: XCTestCase {
    let ok = ProbeFacts(installed: true, running: true, windowCount: 1, macLocked: false, onConsole: true, screenRecording: true, accessibility: true, phoneInUse: false)

    func testHealthyOnlyWhenEverythingHolds() {
        XCTAssertEqual(health(ok), .healthy)
    }

    func testEachFailureMapsToItsState() {
        var f = ok
        f.installed = false
        XCTAssertEqual(health(f), .notInstalled)
        f = ok; f.macLocked = true
        XCTAssertEqual(health(f), .macLocked)
        f = ok; f.onConsole = false
        XCTAssertEqual(health(f), .macLocked)
        f = ok; f.screenRecording = false
        XCTAssertEqual(health(f), .needsScreenRecording)
        f = ok; f.accessibility = false
        XCTAssertEqual(health(f), .needsAccessibility)
        f = ok; f.running = false
        XCTAssertEqual(health(f), .windowHidden)
        f = ok; f.windowCount = 0
        XCTAssertEqual(health(f), .windowHidden)
        f = ok; f.windowCount = 2
        XCTAssertEqual(health(f), .windowAmbiguous)
        f = ok; f.phoneInUse = true
        XCTAssertEqual(health(f), .phoneInUse)
    }

    func testLockedMacWinsOverEverythingButMissingApp() {
        let f = ProbeFacts(installed: true, running: false, windowCount: 0, macLocked: true, onConsole: true, screenRecording: false, accessibility: false)
        XCTAssertEqual(health(f), .macLocked)
    }

    func testPermissionStatesNameTheirPane() {
        XCTAssertEqual(settingsPane(for: .needsScreenRecording)?.permission, "screen_recording")
        XCTAssertTrue(settingsPane(for: .needsAccessibility)?.pane.contains("Accessibility") == true)
        XCTAssertNil(settingsPane(for: .macLocked))
    }

    func testPhoneInUseText() {
        XCTAssertTrue(saysPhoneInUse(["iPhone in Use", "Lock your iPhone to continue"]))
        XCTAssertFalse(saysPhoneInUse(["Messages", "Notes", "9:41"]))
    }
}

final class KeyTests: XCTestCase {
    func testCombos() throws {
        XCTAssertEqual(try parseCombo("cmd+space").get(), KeyStroke(keyCode: 49, flags: Modifier.command))
        XCTAssertEqual(try parseCombo("Enter").get(), KeyStroke(keyCode: 36, flags: 0))
        XCTAssertEqual(try parseCombo("cmd+shift+a").get(), KeyStroke(keyCode: 0, flags: Modifier.command | Modifier.shift))
        XCTAssertEqual(try parseCombo("home").get(), KeyStroke(keyCode: 18, flags: Modifier.command))
        XCTAssertEqual(try parseCombo("spotlight").get(), KeyStroke(keyCode: 20, flags: Modifier.command))
    }
}
