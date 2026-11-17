import XCTest
@testable import Allternit

/// ShortcutBridge: the pending-run map, the timeout, the x-callback / App
/// Intent answers and the contract envelope (CONTRACT-phone-use §3.1, §4).
@MainActor
final class ShortcutBridgeTests: XCTestCase {
    private var defaults: UserDefaults!
    private var opened: [URL] = []

    override func setUp() async throws {
        defaults = UserDefaults(suiteName: "ShortcutBridgeTests.\(UUID().uuidString)")
        opened = []
    }

    private func bridge(opens: Bool = true) -> ShortcutBridge {
        ShortcutBridge(defaults: defaults) { [weak self] url in
            self?.opened.append(url)
            return opens
        }
    }

    /// The envelope is `[String: Any]`, which a Task can't return as is.
    private final class Envelope: @unchecked Sendable {
        let value: [String: Any]
        init(_ value: [String: Any]) { self.value = value }
    }

    /// Starts a run and waits until it is pending; returns its task and request id.
    private func start(_ b: ShortcutBridge, name: String = "Send standup",
                       timeout: Int = 30) async throws -> (Task<Envelope, Never>, String) {
        let task = Task { Envelope(await b.run(name: name, input: nil, timeoutSeconds: timeout)) }
        for _ in 0..<200 where b.pendingCount == 0 { await Task.yield() }
        let id = try XCTUnwrap(b.pendingIds.first)
        return (task, id)
    }

    private func callback(_ id: String, _ rest: String) -> URL {
        URL(string: "allternit://shortcut-result?request=\(id)&\(rest)")!
    }

    func testXSuccessAnswersThePendingRunWithItsJSONOutput() async throws {
        let b = bridge()
        let (task, id) = try await start(b)
        let result = "%7B%22sent%22%3Atrue%7D" // {"sent":true}
        XCTAssertTrue(b.handle(url: callback(id, "status=success&result=\(result)")))
        let envelope = await task.value.value
        XCTAssertEqual(envelope["ok"] as? Bool, true)
        XCTAssertEqual(envelope["route"] as? String, "official")
        XCTAssertEqual(envelope["verified"] as? Bool, true)
        let detail = try XCTUnwrap(envelope["detail"] as? [String: Any])
        XCTAssertEqual((detail["output"] as? [String: Any])?["sent"] as? Bool, true)
        XCTAssertEqual(detail["returned_via"] as? String, "x_callback")
        XCTAssertEqual(b.pendingCount, 0)
        XCTAssertEqual(b.knownShortcuts, ["Send standup"])
    }

    func testTheRunURLCarriesCallbacksThatSurviveTheirOwnQueryCharacters() throws {
        let url = try XCTUnwrap(ShortcutBridge.runURL(name: "Send standup", text: "a&b=c?", requestId: "r1"))
        XCTAssertEqual(url.scheme, "shortcuts")
        XCTAssertEqual(url.host, "x-callback-url")
        XCTAssertEqual(url.path, "/run-shortcut")
        let items = try XCTUnwrap(URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems)
        func value(_ name: String) -> String? { items.first { $0.name == name }?.value }
        XCTAssertEqual(items.count, 6)
        XCTAssertEqual(value("name"), "Send standup")
        XCTAssertEqual(value("input"), "text")
        XCTAssertEqual(value("text"), "a&b=c?")
        XCTAssertEqual(value("x-success"), "allternit://shortcut-result?request=r1&status=success")
        XCTAssertEqual(value("x-cancel"), "allternit://shortcut-result?request=r1&status=cancel")
    }

    func testAnIntentAnswerWithNoIdGoesToTheOnlyRunAndIsDroppedWhenSeveralWait() async throws {
        let b = bridge()
        let (first, _) = try await start(b, name: "One")
        XCTAssertTrue(b.deliver(result: "done", requestId: nil))
        let envelope = await first.value.value
        let detail = try XCTUnwrap(envelope["detail"] as? [String: Any])
        XCTAssertEqual(detail["output"] as? String, "done")
        XCTAssertEqual(detail["returned_via"] as? String, "app_intent")

        let (a, idA) = try await start(b, name: "A")
        let (c, idC) = try await start(b, name: "C")
        XCTAssertFalse(b.deliver(result: "?", requestId: nil), "two runs waiting: an answer with no id can't be placed")
        XCTAssertEqual(b.pendingCount, 2)
        XCTAssertTrue(b.deliver(result: "a", requestId: idA.uppercased()))
        XCTAssertTrue(b.deliver(result: "c", requestId: idC))
        _ = await (a.value, c.value)
        XCTAssertEqual(b.pendingCount, 0)
    }

    func testACallbackForAnUnknownIdIsSwallowedAndLeavesTheRunWaiting() async throws {
        let b = bridge()
        let (task, id) = try await start(b)
        XCTAssertTrue(b.handle(url: callback("not-a-pending-id", "status=success&result=fake")))
        XCTAssertTrue(b.handle(url: URL(string: "allternit://shortcut-result?status=success")!), "no id: still ours, opens nothing")
        XCTAssertFalse(b.handle(url: URL(string: "allternit://inbox")!), "other links go to the deep link router")
        XCTAssertEqual(b.pendingCount, 1)
        // The first answer wins; a later duplicate does nothing.
        XCTAssertTrue(b.deliver(result: "real", requestId: id))
        XCTAssertTrue(b.handle(url: callback(id, "status=success&result=late")))
        let answered = await task.value.value
        let detail = try XCTUnwrap(answered["detail"] as? [String: Any])
        XCTAssertEqual(detail["output"] as? String, "real")
    }

    func testNoAnswerBeforeTheTimeoutAsksThePersonToAddTheShortcut() async throws {
        let b = bridge()
        let (task, _) = try await start(b, name: "Allternit Slack", timeout: 1)
        let envelope = await task.value.value
        XCTAssertEqual(envelope["ok"] as? Bool, false)
        XCTAssertEqual(envelope["verified"] as? Bool, false)
        let needs = try XCTUnwrap(envelope["needs_you"] as? [String: Any])
        XCTAssertEqual(needs["code"] as? String, "SHORTCUT_NOT_INSTALLED")
        XCTAssertEqual(needs["message"] as? String,
                       "Add the \"Allternit Slack\" shortcut in Allternit Settings > Device Access > iPhone actions.")
        XCTAssertEqual((envelope["detail"] as? [String: Any])?["error"] as? String, "timeout")
        XCTAssertEqual(b.pendingCount, 0)
        XCTAssertEqual(opened.count, 1)
    }

    func testShortcutsNotOpeningFailsWithoutWaitingForTheTimeout() async throws {
        let b = bridge(opens: false)
        let envelope = await b.run(name: "Morning", input: nil, timeoutSeconds: 30)
        XCTAssertEqual((envelope["detail"] as? [String: Any])?["error"] as? String, "shortcuts_unavailable")
        XCTAssertNil(envelope["needs_you"])
        XCTAssertEqual(b.pendingCount, 0)
    }

    func testXErrorAndCancelMapToTheContractEnvelope() async throws {
        let b = bridge()
        let (missing, id) = try await start(b, name: "Ghost")
        b.handle(url: callback(id, "status=error&errorMessage=The%20shortcut%20couldn%E2%80%99t%20be%20found."))
        let gone = await missing.value.value
        XCTAssertEqual((gone["needs_you"] as? [String: Any])?["code"] as? String, "SHORTCUT_NOT_INSTALLED")

        let failed = ShortcutBridge.envelope(.failed("Slack is signed out"), name: "Allternit Slack")
        XCTAssertNil(failed["needs_you"])
        XCTAssertEqual((failed["detail"] as? [String: Any])?["error"] as? String, "shortcut_failed")
        XCTAssertEqual((failed["detail"] as? [String: Any])?["message"] as? String,
                       "The \"Allternit Slack\" shortcut stopped: Slack is signed out")

        let (cancelled, cid) = try await start(b, name: "Morning")
        b.handle(url: callback(cid, "status=cancel"))
        let stopped = await cancelled.value.value
        XCTAssertEqual((stopped["detail"] as? [String: Any])?["error"] as? String, "cancelled")
    }

    func testOnlyAllternitShortcutsGetTheRequestIdInTheirJSONInput() throws {
        guard case .text(let own) = ShortcutBridge.inputText(["action": "notes.create"], name: "Allternit Actions", requestId: "r9") else {
            return XCTFail("object input should become JSON text")
        }
        let ownObject = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(own.utf8)) as? [String: Any])
        XCTAssertEqual(ownObject["request_id"] as? String, "r9")
        XCTAssertEqual(ownObject["action"] as? String, "notes.create")

        XCTAssertEqual(ShortcutBridge.inputText(["a": 1], name: "My Morning", requestId: "r9"), .text("{\"a\":1}"))
        XCTAssertEqual(ShortcutBridge.inputText("hi", name: "Allternit Actions", requestId: "r9"), .text("hi"))
        XCTAssertEqual(ShortcutBridge.inputText(nil, name: "x", requestId: "r9"), ShortcutBridge.InputText.none)
        XCTAssertEqual(ShortcutBridge.inputText(String(repeating: "x", count: 40_000), name: "x", requestId: "r"), .tooLarge)
    }
}
