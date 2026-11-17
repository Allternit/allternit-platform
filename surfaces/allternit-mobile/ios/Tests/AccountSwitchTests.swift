import XCTest
@testable import Allternit

/// Account switching (phone calm step 31, plan contract C7, decisions D3 and
/// D14; web `src/lib/accounts/switch-account.ts`): the runtime device
/// credential is keyed per Clerk user and the old unkeyed item moves without
/// being lost, the cleanup runs before Clerk `setActive`, and inactive rows
/// read their plan label from the cache.
final class AccountSwitchTests: XCTestCase {
    /// In-memory stand-in for the Keychain (the edge, not our code).
    private final class MemoryStore: SecretStore {
        var items: [String: String] = [:]
        var failSaves = false
        func load(_ account: String) -> String? { items[account] }
        func save(_ value: String, for account: String) { if !failSaves { items[account] = value } }
        func delete(_ account: String) { items.removeValue(forKey: account) }
    }

    private let credential = #"{"runtimeId":"rt_1","userId":"user_A","userEmail":"a@example.com","deviceToken":"allternit_runtime_x","tokenType":"Bearer","expiresAt":"2027-01-01T00:00:00Z","capabilities":[]}"#

    private var defaults: UserDefaults!
    private let suite = "AccountSwitchTests"

    override func setUp() {
        super.setUp()
        defaults = UserDefaults(suiteName: suite)
        defaults.removePersistentDomain(forName: suite)
    }

    override func tearDown() {
        defaults.removePersistentDomain(forName: suite)
        super.tearDown()
    }

    // MARK: - Per-user Keychain key

    func testKeyIsNamedPerClerkUser() {
        XCTAssertEqual(RuntimeCredentialKeys.key(forUserId: "user_A"), "allternit-ios-runtime-session.user_A")
        XCTAssertNotEqual(RuntimeCredentialKeys.key(forUserId: "user_A"), RuntimeCredentialKeys.key(forUserId: "user_B"))
        XCTAssertEqual(RuntimeCredentialKeys.legacy, "allternit-ios-runtime-session")
    }

    func testMigrationMovesTheUnkeyedItemAndKeepsTheCredential() {
        let store = MemoryStore()
        store.items[RuntimeCredentialKeys.legacy] = credential

        let loaded = RuntimeCredentialKeys.loadMigrating(userId: "user_A", store: store)

        XCTAssertEqual(loaded, credential)
        XCTAssertEqual(store.items[RuntimeCredentialKeys.key(forUserId: "user_A")], credential)
        XCTAssertNil(store.items[RuntimeCredentialKeys.legacy])
        // Second launch reads the keyed item.
        XCTAssertEqual(RuntimeCredentialKeys.loadMigrating(userId: "user_A", store: store), credential)
    }

    func testFailedCopyKeepsUsingTheUnkeyedItem() {
        let store = MemoryStore()
        store.items[RuntimeCredentialKeys.legacy] = credential
        store.failSaves = true

        XCTAssertEqual(RuntimeCredentialKeys.loadMigrating(userId: "user_A", store: store), credential)
        XCTAssertEqual(store.items[RuntimeCredentialKeys.legacy], credential, "a working pairing is never dropped")
    }

    func testAnotherPersonsUnkeyedItemIsNotTaken() {
        let store = MemoryStore()
        store.items[RuntimeCredentialKeys.legacy] = credential

        XCTAssertNil(RuntimeCredentialKeys.loadMigrating(userId: "user_B", store: store))
        XCTAssertEqual(store.items[RuntimeCredentialKeys.legacy], credential)
        XCTAssertNil(store.items[RuntimeCredentialKeys.key(forUserId: "user_B")])
    }

    func testEachPersonKeepsTheirOwnCredential() {
        let store = MemoryStore()
        store.items[RuntimeCredentialKeys.key(forUserId: "user_A")] = "A"
        store.items[RuntimeCredentialKeys.key(forUserId: "user_B")] = "B"

        XCTAssertEqual(RuntimeCredentialKeys.loadMigrating(userId: "user_A", store: store), "A")
        XCTAssertEqual(RuntimeCredentialKeys.loadMigrating(userId: "user_B", store: store), "B")
    }

    func testOnlyAPersonChangeClearsPersonData() {
        XCTAssertFalse(AccountSwitchReason.organizationChanged.clearsPersonData)
        XCTAssertTrue(AccountSwitchReason.personChanged.clearsPersonData)
    }

    // MARK: - Switch order

    @MainActor
    func testCleanupRunsBeforeSetActiveThenRebuild() async throws {
        var calls: [String] = []
        let steps = AccountSwitchSteps(
            cleanup: { reason in calls.append("cleanup:\(reason.rawValue)") },
            setActive: { session, org in calls.append("setActive:\(session):\(org ?? "personal")") },
            rebuild: { calls.append("rebuild") }
        )

        try await AccountSwitchOrder.run(sessionId: "sess_1", orgId: "org_2", reason: .organizationChanged, steps: steps)

        XCTAssertEqual(calls, ["cleanup:organizationChanged", "setActive:sess_1:org_2", "rebuild"])
    }

    @MainActor
    func testRefusedSwitchStillRebuildsAndPassesTheError() async {
        struct Refused: Error {}
        var calls: [String] = []
        let steps = AccountSwitchSteps(
            cleanup: { _ in calls.append("cleanup") },
            setActive: { _, _ in calls.append("setActive"); throw Refused() },
            rebuild: { calls.append("rebuild") }
        )

        do {
            try await AccountSwitchOrder.run(sessionId: "sess_1", orgId: nil, reason: .organizationChanged, steps: steps)
            XCTFail("the refusal must reach the caller")
        } catch {
            XCTAssertTrue(error is Refused)
        }
        XCTAssertEqual(calls, ["cleanup", "setActive", "rebuild"])
    }

    // MARK: - Cached plan labels (D14)

    func testPlanLabelRowKeyMatchesTheWeb() {
        XCTAssertEqual(AccountPlanLabels.rowKey(userId: "user_A", orgId: nil), "user_A:personal")
        XCTAssertEqual(AccountPlanLabels.rowKey(userId: "user_A", orgId: "org_1"), "user_A:org_1")
        XCTAssertEqual(AccountPlanLabels.defaultsKey, "allternit-account-plan-labels-v1")
    }

    func testInactiveRowReadsItsCachedPlan() {
        AccountPlanLabels.remember(userId: "user_A", orgId: "org_1", planName: "Team", defaults: defaults)
        AccountPlanLabels.remember(userId: "user_A", orgId: nil, planName: "Pro", defaults: defaults)

        let rows = accountRows(
            personName: "Ada",
            personFallbackDetail: "ada@example.com",
            organizations: [(id: "org_1", name: "Harbor"), (id: "org_2", name: "Pier")],
            activeOrgId: nil,
            livePlanName: "Max",
            cachedPlan: { AccountPlanLabels.read(userId: "user_A", orgId: $0, defaults: self.defaults) }
        )

        XCTAssertEqual(rows.map(\.name), ["Ada", "Harbor", "Pier"])
        XCTAssertEqual(rows[0].detail, "Personal · Max", "the active row shows the live plan")
        XCTAssertTrue(rows[0].isActive)
        XCTAssertEqual(rows[1].detail, "Organization · Team", "an inactive row shows its cached plan")
        XCTAssertFalse(rows[1].isActive)
        XCTAssertEqual(rows[2].detail, "Organization", "no cached plan, no fetch")
    }

    func testActiveRowFallsBackToCacheThenEmail() {
        AccountPlanLabels.remember(userId: "user_A", orgId: "org_1", planName: "Team", defaults: defaults)
        let read: (String?) -> String? = { AccountPlanLabels.read(userId: "user_A", orgId: $0, defaults: self.defaults) }

        let orgActive = accountRows(personName: "Ada", personFallbackDetail: "ada@example.com",
                                    organizations: [(id: "org_1", name: "Harbor")], activeOrgId: "org_1",
                                    livePlanName: nil, cachedPlan: read)
        XCTAssertEqual(orgActive[1].detail, "Organization · Team")
        XCTAssertEqual(orgActive[0].detail, "Personal · ada@example.com")
    }

    func testForgetDropsOnlyThatPersonsLabels() {
        AccountPlanLabels.remember(userId: "user_A", orgId: nil, planName: "Pro", defaults: defaults)
        AccountPlanLabels.remember(userId: "user_B", orgId: nil, planName: "Team", defaults: defaults)

        AccountPlanLabels.forget(userId: "user_A", defaults: defaults)

        XCTAssertNil(AccountPlanLabels.read(userId: "user_A", orgId: nil, defaults: defaults))
        XCTAssertEqual(AccountPlanLabels.read(userId: "user_B", orgId: nil, defaults: defaults), "Team")
    }

    func testRememberingNoPlanRemovesTheLabel() {
        AccountPlanLabels.remember(userId: "user_A", orgId: nil, planName: "Pro", defaults: defaults)
        AccountPlanLabels.remember(userId: "user_A", orgId: nil, planName: nil, defaults: defaults)
        XCTAssertNil(AccountPlanLabels.read(userId: "user_A", orgId: nil, defaults: defaults))
    }
}
