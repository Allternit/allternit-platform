import XCTest
@testable import Allternit

/// Teach's recordings vanished on the phone: `GET /contracts` has no
/// `endpoints` (db.rs `list_capture_contracts`), the decoder required it, and
/// the caller swallowed the error.
final class TeachClientDecodingTests: XCTestCase {
    func testContractListRowWithoutEndpointsDecodes() throws {
        let row = #"{"id":"c1","domain":"orders.example","source":"upload","derived_at":"2026-11-01T00:00:00Z"}"#
        let contract = try JSONDecoder().decode(TeachContract.self, from: Data(row.utf8))
        XCTAssertEqual(contract.domain, "orders.example")
        XCTAssertTrue(contract.endpoints.isEmpty)
    }

    func testContractDetailKeepsItsEndpoints() throws {
        let row = #"{"id":"c1","domain":"orders.example","endpoints":[{"id":"e1","method":"GET"},{"id":"e2"}]}"#
        XCTAssertEqual(try JSONDecoder().decode(TeachContract.self, from: Data(row.utf8)).endpoints.count, 2)
    }

    func testSkillRowAsTheServerSendsItBecomesASkill() throws {
        let row = #"{"id":"s1","title":"Reorder flour","intent":"","steps":[{"id":"a","kind":"ui","text":"Open {{site}}"}],"inputs":[],"source":"rules","redactions":3,"created_at":"2026-11-01","updated_at":"2026-11-02","workflow":{"version":1,"steps":[{"id":"w1","kind":"goto"}]}}"#
        let raw = try JSONDecoder().decode([String: JSONValue].self, from: Data(row.utf8))
        let skill = try XCTUnwrap(TeachSkill(raw: raw))
        XCTAssertEqual(skill.title, "Reorder flour")
        XCTAssertEqual(skill.redactions, 3)
        XCTAssertEqual(skill.workflowStepCount, 1)
    }

    func testPlaceholdersSplitOutOfAStep() {
        let parts = TeachPlaceholders.split("Open {{ site }} and pick {{profile.week}}")
        XCTAssertEqual(parts.compactMap(\.name), ["site", "profile.week"])
        XCTAssertEqual(parts.first?.text, "Open ")
    }
}
