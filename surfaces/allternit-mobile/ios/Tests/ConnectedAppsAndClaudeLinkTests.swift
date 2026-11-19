import XCTest
@testable import Allternit

final class ConnectedAppsAndClaudeLinkTests: XCTestCase {
    func testGrantsListDecodes() throws {
        let json = #"""
        {"object":"list","month":"2026-11","resets_at":"2026-12-01T00:00:00Z","data":[
          {"id":"g1","app_id":"a1","app_name":"Notes Sync","app_logo_url":null,"app_homepage_url":"https://notes.test",
           "app_state":"testing","scopes":["inference","usage"],"monthly_cap_cents":1000,"max_cap_cents":5000,
           "spent_cents":123,"created_at":"2026-11-01T00:00:00Z","last_used_at":null},
          {"id":"g2","app_id":"a2","app_name":"Live App","app_logo_url":"https://x.test/l.png","app_homepage_url":null,
           "app_state":"live","scopes":["inference"],"monthly_cap_cents":250,"max_cap_cents":null,
           "spent_cents":0,"created_at":"2026-11-02T00:00:00Z","last_used_at":"2026-11-03T00:00:00Z"}]}
        """#
        let list = try JSONDecoder().decode(OAuthGrantsList.self, from: Data(json.utf8))
        XCTAssertEqual(list.resetsAt, "2026-12-01T00:00:00Z")
        XCTAssertEqual(list.data.count, 2)
        XCTAssertTrue(list.data[0].isTesting)
        XCTAssertEqual(list.data[0].maxCapCents, 5000)
        XCTAssertEqual(GrantMoney.spentLine(list.data[0]), "Spent $1.23 of $10 this month")
        XCTAssertFalse(list.data[1].isTesting)
        XCTAssertNil(list.data[1].maxCapCents)
        XCTAssertEqual(GrantMoney.text(250), "$2.50")
    }

    func testClaudeLinkStatusDecodes() throws {
        let json = #"{"state":"relink_needed","org_name":null,"linked_at":"2026-11-01T00:00:00Z","relink_by":"2027-10-20T00:00:00Z","spend_limit":{"reached":true}}"#
        let status = try JSONDecoder().decode(ClaudeLinkStatus.self, from: Data(json.utf8))
        XCTAssertEqual(status.state, "relink_needed")
        XCTAssertEqual(status.spendLimit?.reached, true)
        let bare = try JSONDecoder().decode(ClaudeLinkStatus.self, from: Data(#"{"state":"unlinked","linked_at":null,"relink_by":null,"spend_limit":null}"#.utf8))
        XCTAssertNil(bare.spendLimit)
    }

    func testLinkedClaudeModelIsPersonOnlyUnderItsOwnGroup() {
        let id = "linked/claude/claude-sonnet-5-5"
        XCTAssertTrue(PersonModels.isPersonOnly(id))
        XCTAssertEqual(PersonModels.provider(for: id), "claude-org")
        XCTAssertEqual(PersonModels.providerNames["claude-org"], "Claude organization")
        XCTAssertEqual(PersonModels.vendorName(for: id), "Claude organization")
        var row = ConnectionWire(id: "claude-org", state: .on)
        row.models = [id]
        let built = PersonModels.build(rows: [row], chatgpt: [])
        XCTAssertEqual(built.map(\.id), [id])
        XCTAssertEqual(built.first?.provider, "claude-org")
    }
}
