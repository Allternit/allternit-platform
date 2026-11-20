import XCTest
@testable import Allternit

/// Connect kit (CONTRACT-connections-2026-11 §3): how cloud and computer rows
/// become one list, and how a row from either server decodes.
final class ConnectionsMergeTests: XCTestCase {
    private let known: Set<String> = ["codex", "chatgpt", "openai-key"]

    func testComputerRowWinsOnlyWhenConnected() {
        let cloud = [ConnectionWire(id: "chatgpt", state: .off, available: false, source: "cloud"),
                     ConnectionWire(id: "openai-key", state: .on, found: "Key works", source: "cloud")]
        let runtime = [ConnectionWire(id: "chatgpt", state: .on, found: "ChatGPT Plus", source: "runtime:mac-1"),
                       ConnectionWire(id: "openai-key", state: .needs, source: "runtime:mac-1")]
        let merged = ConnectionsMerge.merge(cloud: cloud, runtime: runtime, known: known)
        XCTAssertEqual(merged["chatgpt"]?.found, "ChatGPT Plus")
        XCTAssertEqual(merged["chatgpt"]?.runtimeId, "mac-1")
        // A computer row that isn't on never hides the account's working key.
        XCTAssertEqual(merged["openai-key"]?.state, .on)
        XCTAssertEqual(merged["openai-key"]?.source, "cloud")
    }

    func testComputerOnlyRowsAreKeptAndUnknownIdsDropped() {
        let runtime = [ConnectionWire(id: "codex", state: .needs, reasonCode: "login_expired", source: "runtime:mac-1"),
                       ConnectionWire(id: "some-new-thing", state: .on, source: "runtime:mac-1")]
        let merged = ConnectionsMerge.merge(cloud: [], runtime: runtime, known: known)
        XCTAssertEqual(merged["codex"]?.state, .needs)
        XCTAssertNil(merged["some-new-thing"])
    }

    func testRowDecodesBothTermShapesAndUnknownStates() throws {
        let json = """
        {"version":1,"connections":[
          {"id":"claude","group":"brains","state":"off","runs_on":"runner","available":false,
           "unavailable_reason":"Not available yet. Use a key for now.","fallbacks":["anthropic-key"],
           "terms":{"status":"pending"},"source":"cloud"},
          {"id":"codex","state":"sleeping","terms":"cleared","fallbacks":null,"source":"runtime:mac-1"}
        ]}
        """
        let rows = try JSONDecoder().decode(ConnectionsEnvelope.self, from: Data(json.utf8)).connections
        XCTAssertEqual(rows[0].terms, "pending")
        XCTAssertFalse(rows[0].available)
        XCTAssertEqual(rows[0].fallbacks, ["anthropic-key"])
        XCTAssertEqual(rows[1].terms, "cleared")
        XCTAssertEqual(rows[1].state, .off, "a state outside the four words reads as Not connected")
        XCTAssertEqual(rows[1].fallbacks, [])
    }

    func testRowsWithoutAServerAnswerNeverOfferADeadConnect() {
        for id in ["claude", "grok", "kimi", "copilot"] {
            let row = ConnectionsCatalog.fallbackRow(id, computerOnline: true)
            XCTAssertFalse(row.available, id)
            XCTAssertFalse(row.fallbacks.isEmpty, id)
        }
        // Contract §9.9: on iOS ChatGPT signs in on the phone, so it never
        // waits for a computer or the server.
        XCTAssertTrue(ConnectionsCatalog.fallbackRow("chatgpt", computerOnline: false).available)
        XCTAssertTrue(ConnectionsCatalog.fallbackRow("chatgpt", computerOnline: true).available)
        XCTAssertEqual(ConnectionsCatalog.fallbackRow("codex", computerOnline: false).fallbacks, ["this-mac"])
    }

    func testLoginPayloadOpensTheCodeLinkWhenTheVendorHasOne() throws {
        let json = #"{"login":{"kind":"device_code","code":"KQ7M-24TX","url":"https://accounts.x.ai/oauth2/device","url_with_code":"https://accounts.x.ai/oauth2/device?user_code=KQ7M-24TX","expires_at":"2026-11-13T20:15:00Z","poll":true}}"#
        let result = try JSONDecoder().decode(ConnConnectResult.self, from: Data(json.utf8))
        XCTAssertEqual(result.login?.kind, .deviceCode)
        XCTAssertEqual(result.login?.openURL?.absoluteString, "https://accounts.x.ai/oauth2/device?user_code=KQ7M-24TX")
        XCTAssertNotNil(result.login?.expiry)
    }

    /// Contract §11: a catalog page decodes leniently, and More never lists
    /// what the page already shows or a way this app can't run.
    func testCatalogPageDecodesAndHidesListedEntries() throws {
        let json = """
        {"version":1,"group":"apps","q":"","total":40,"next_cursor":"24",
         "counts":{"brains":11,"apps":1190,"channels":0},
         "entries":[
          {"id":"airtable","group":"apps","name":"Airtable","category":"Project management","ways":["oauth"],
           "runs_on":"computer","connector_id":"airtable","key_page":null,"logo":null,"available":false,
           "unavailable_reason":"Runs on a computer.","fallbacks":["this-mac"]},
          {"id":"gmail","group":"apps","name":"Gmail","ways":["oauth"]},
          {"id":"google-key","group":"brains","name":"Google AI Studio","ways":["paste"]},
          {"id":"teleport","group":"apps","name":"Teleport","ways":["warp"],"fallbacks":null}
         ]}
        """
        let page = try JSONDecoder().decode(ConnCatalogPage.self, from: Data(json.utf8))
        XCTAssertEqual(page.total, 40)
        XCTAssertEqual(page.nextCursor, "24")
        XCTAssertEqual(page.counts["apps"], 1190)
        XCTAssertEqual(page.entries[0].unavailableReason, "Runs on a computer.")
        XCTAssertEqual(page.entries[0].fallbacks, ["this-mac"])
        XCTAssertFalse(page.entries[0].available)
        XCTAssertEqual(page.entries[3].fallbacks, [])

        // The page lists Gmail, and the Gemini row is the AI Studio key.
        let extras = ConnectionsMerge.catalogExtras(page.entries, listed: ["gmail", "gemini", "google-key"])
        XCTAssertEqual(extras.map(\.id), ["airtable"], "listed ids and unknown ways are hidden")
    }
}
