import XCTest
@testable import Allternit

/// Picker catalog (docs/contracts/picker-catalog, fixtures copied to
/// Tests/Fixtures/PickerCatalog): the iOS model list reads lock state from the
/// server and falls back to today's path on an old server.
@MainActor
final class PickerCatalogTests: XCTestCase {
    private func fixture(_ name: String) throws -> Data {
        let url = try XCTUnwrap(Bundle(for: Self.self).url(forResource: "picker-catalog.\(name)", withExtension: "json"),
                                "missing fixture \(name)")
        return try Data(contentsOf: url)
    }

    private func body(_ name: String) throws -> PickerCatalogBody {
        try JSONDecoder().decode(PickerCatalogBody.self, from: fixture(name))
    }

    private let rows = ["claude-sonnet-5.5", "gpt-6.1-sol", "deepseek-v4.1-flash", "glm-5.3-flash"].map {
        SetupModelRow(id: $0, name: $0)
    }

    func testFundedAllOk() throws {
        let b = try body("funded")
        XCTAssertEqual(b.cloudModels.count, 4)
        XCTAssertTrue(b.cloudModels.allSatisfy { $0.access.state == .ok })
    }

    func testNoCreditsLocksWithReasonAndAction() throws {
        let store = ModelStore(defaults: UserDefaults(suiteName: "picker-catalog-tests")!)
        store.applyPickerCatalog(try body("no-credits"))
        XCTAssertTrue(store.isCloudLocked("allternit/gpt-6.1-sol"))
        XCTAssertEqual(store.lockReason(for: "allternit/gpt-6.1-sol"), "No credits left")
        XCTAssertEqual(store.pickerAccess(for: "allternit/gpt-6.1-sol")?.action, .addCredits)
    }

    func testSignedOutRoutesToSignIn() throws {
        let store = ModelStore(defaults: UserDefaults(suiteName: "picker-catalog-tests")!)
        store.applyPickerCatalog(try body("signed-out"))
        XCTAssertTrue(store.isCloudLocked("allternit/claude-sonnet-5.5"))
        XCTAssertEqual(store.pickerAccess(for: "allternit/claude-sonnet-5.5")?.action, .signIn)
    }

    func testHiddenRowIsNotListed() throws {
        let access = try body("hidden-model").accessById
        let listed = ModelStore.cloudChatModels(rows, access: access).map(\.id)
        XCTAssertFalse(listed.contains("allternit/glm-5.3-flash"))
        XCTAssertEqual(listed.count, 3)
        XCTAssertEqual(ModelStore.cloudChatModels(rows).count, 4)
    }

    func testFundedIsNotLockedAndOtherIdsFallBack() throws {
        let store = ModelStore(defaults: UserDefaults(suiteName: "picker-catalog-tests")!)
        store.applyPickerCatalog(try body("funded"))
        XCTAssertFalse(store.isCloudLocked("allternit/claude-sonnet-5.5"))
        XCTAssertNil(store.pickerAccess(for: "allternit/unknown"))
        XCTAssertFalse(store.isCloudLocked("claude-sonnet-5-5"), "non-Cloud ids are never Cloud-locked")
    }

    func testInterpretStatuses() throws {
        let data = try fixture("funded")
        guard case let .fresh(b, etag) = PickerCatalogClient.interpret(status: 200, data: data, etag: "W/\"1\"") else {
            return XCTFail("expected fresh")
        }
        XCTAssertEqual(b.cloudModels.count, 4)
        XCTAssertEqual(etag, "W/\"1\"")
        XCTAssertEqual(PickerCatalogClient.interpret(status: 304, data: Data(), etag: nil), .notModified)
        XCTAssertEqual(PickerCatalogClient.interpret(status: 404, data: Data(), etag: nil), .unsupported)
        XCTAssertEqual(PickerCatalogClient.interpret(status: 200, data: Data("{}".utf8), etag: nil), .unsupported)
        XCTAssertEqual(PickerCatalogClient.interpret(status: 500, data: Data(), etag: nil), .failed)
    }

    func testUnknownStateDegradesToOk() throws {
        let json = #"{"cloudModels":[{"id":"allternit/x","name":"X","provider":"p","access":{"state":"future","action":"teleport"}}]}"#
        let b = try JSONDecoder().decode(PickerCatalogBody.self, from: Data(json.utf8))
        XCTAssertEqual(b.cloudModels[0].access.state, .ok)
        XCTAssertNil(b.cloudModels[0].access.action)
    }
}
