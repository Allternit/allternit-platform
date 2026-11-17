import XCTest
@testable import Allternit

/// "Switch account" without Clerk multi-session (phone calm I14b, option B;
/// web `src/lib/accounts/remembered-accounts.test.ts`): the device's list of
/// accounts, the person switch order, the list surviving the person cleanup,
/// and the new store resets.
final class RememberedAccountsTests: XCTestCase {
    private var defaults: UserDefaults!
    private let suite = "RememberedAccountsTests"

    override func setUp() {
        super.setUp()
        defaults = UserDefaults(suiteName: suite)
        defaults.removePersistentDomain(forName: suite)
    }

    override func tearDown() {
        defaults.removePersistentDomain(forName: suite)
        super.tearDown()
    }

    private func remember(_ n: Int, name: String? = nil, imageUrl: String? = nil, plan: String? = nil, at seconds: TimeInterval) {
        RememberedAccounts.remember(userId: "u\(n)", name: name ?? "Person \(n)", email: "p\(n)@example.com",
                                    imageUrl: imageUrl, planLabel: plan,
                                    defaults: defaults, now: Date(timeIntervalSince1970: seconds))
    }

    // MARK: - Remember, dedupe, cap, remove

    func testListsNewestFirstOneRowPerPersonRefreshedOnSignInAgain() {
        remember(1, at: 1000)
        remember(2, at: 2000)
        remember(1, name: "Person One", imageUrl: "https://img.example.com/1.png", at: 3000)

        let rows = RememberedAccounts.list(defaults: defaults)
        XCTAssertEqual(rows.map(\.userId), ["u1", "u2"])
        XCTAssertEqual(rows[0].name, "Person One")
        XCTAssertEqual(rows[0].imageUrl, "https://img.example.com/1.png")
        XCTAssertEqual(rows[0].lastUsedAt, Date(timeIntervalSince1970: 3000))
    }

    func testKeepsAtMostTheCapDroppingTheOldest() {
        for n in 1...(RememberedAccounts.maxCount + 2) { remember(n, at: TimeInterval(n * 1000)) }
        let ids = RememberedAccounts.list(defaults: defaults).map(\.userId)
        XCTAssertEqual(ids.count, RememberedAccounts.maxCount)
        XCTAssertFalse(ids.contains("u1"))
        XCTAssertFalse(ids.contains("u2"))
        XCTAssertEqual(ids.first, "u\(RememberedAccounts.maxCount + 2)")
    }

    func testRemoveFromThisDeviceKeepsTheOthers() {
        remember(1, at: 1000)
        remember(2, at: 2000)
        RememberedAccounts.forget(userId: "u1", defaults: defaults)
        XCTAssertEqual(RememberedAccounts.list(defaults: defaults).map(\.userId), ["u2"])
    }

    func testOthersLeaveOutTheSignedInPerson() {
        remember(1, at: 1000)
        remember(2, at: 2000)
        XCTAssertEqual(RememberedAccounts.others(excluding: "u2", defaults: defaults).map(\.userId), ["u1"])
    }

    func testKeepsTheSavedPlanWhenASignInHasNoneAndUpdatesIt() {
        remember(1, plan: "Pro", at: 1000)
        remember(1, plan: nil, at: 2000)
        XCTAssertEqual(RememberedAccounts.list(defaults: defaults).first?.planLabel, "Pro")
        RememberedAccounts.setPlan(userId: "u1", planLabel: "Max", defaults: defaults)
        XCTAssertEqual(RememberedAccounts.list(defaults: defaults).first?.detail, "p1@example.com · Max")
    }

    func testSkipsAnAccountWithNoEmailAndStoresNoSessionData() throws {
        remember(1, at: 1000)
        RememberedAccounts.remember(userId: "u9", name: "No email", email: "", imageUrl: nil, defaults: defaults)
        RememberedAccounts.remember(userId: "u8", name: "Bad", email: "not-an-email", imageUrl: nil, defaults: defaults)
        XCTAssertEqual(RememberedAccounts.list(defaults: defaults).map(\.userId), ["u1"])

        let data = try XCTUnwrap(defaults.data(forKey: RememberedAccounts.defaultsKey))
        let object = try XCTUnwrap(try JSONSerialization.jsonObject(with: data) as? [[String: Any]])
        let allowed: Set<String> = ["userId", "name", "email", "imageUrl", "planLabel", "lastUsedAt"]
        XCTAssertTrue(Set(object[0].keys).isSubset(of: allowed), "only what Clerk shows on screen, no token or session id")
    }

    func testNameFallsBackToTheEmailsLocalPart() {
        RememberedAccounts.remember(userId: "u1", name: "  ", email: "sam@example.com", imageUrl: nil, defaults: defaults)
        XCTAssertEqual(RememberedAccounts.list(defaults: defaults).first?.name, "sam")
        XCTAssertEqual(RememberedAccounts.list(defaults: defaults).first?.initial, "S")
    }

    // MARK: - Person switch order

    @MainActor
    private func steps(log: @escaping @MainActor (String) -> Void, refuse: Bool = false) -> PersonSwitchSteps {
        PersonSwitchSteps(
            cleanup: { log("cleanup") },
            signOut: {
                log("signOut")
                if refuse { throw URLError(.notConnectedToInternet) }
            },
            afterSignOut: { log("afterSignOut") },
            rebuild: { log("rebuild") },
            showSignIn: { email in log("signIn:\(email ?? "empty")") }
        )
    }

    @MainActor
    func testCleanupRunsBeforeSignOutThenSignInHasTheEmail() async throws {
        var calls: [String] = []
        try await PersonSwitchOrder.run(email: "p1@example.com", steps: steps(log: { calls.append($0) }))
        XCTAssertEqual(calls, ["cleanup", "signOut", "afterSignOut", "signIn:p1@example.com"])
    }

    @MainActor
    func testAddAnotherAccountOpensAnEmptySignIn() async throws {
        var calls: [String] = []
        try await PersonSwitchOrder.run(email: nil, steps: steps(log: { calls.append($0) }))
        XCTAssertEqual(calls.last, "signIn:empty")
        calls = []
        try await PersonSwitchOrder.run(email: "not-an-email", steps: steps(log: { calls.append($0) }))
        XCTAssertEqual(calls.last, "signIn:empty")
    }

    @MainActor
    func testRefusedSignOutKeepsThePersonAndShowsNoSignIn() async {
        var calls: [String] = []
        do {
            try await PersonSwitchOrder.run(email: "p1@example.com", steps: steps(log: { calls.append($0) }, refuse: true))
            XCTFail("a refused sign-out is passed on")
        } catch {}
        XCTAssertEqual(calls, ["cleanup", "signOut", "rebuild"])
    }

    // MARK: - Cleanup keeps the list, resets stores and selections

    /// Runs `body` with the app's standard defaults key for the list saved
    /// and restored, since the cleanup works on the shared stores.
    @MainActor
    private func withStandardList(_ body: () throws -> Void) rethrows {
        let standard = UserDefaults.standard
        let saved = standard.data(forKey: RememberedAccounts.defaultsKey)
        defer {
            if let saved { standard.set(saved, forKey: RememberedAccounts.defaultsKey) }
            else { standard.removeObject(forKey: RememberedAccounts.defaultsKey) }
        }
        try body()
    }

    @MainActor
    func testRememberedListSurvivesThePersonCleanup() {
        withStandardList {
            RememberedAccounts.remember(userId: "u1", name: "Person 1", email: "p1@example.com", imageUrl: nil)
            AccountSwitchCleanup.resetLocalState(reason: .personChanged)
            XCTAssertEqual(RememberedAccounts.list().map(\.userId).first, "u1")
        }
    }

    @MainActor
    func testOrgAndPersonSwitchClearSavedSelections() {
        for reason in [AccountSwitchReason.organizationChanged, .personChanged] {
            ProjectStore.shared.selectedProjectId = "proj_1"
            InstanceStore.shared.select("inst_1")
            RuntimeRelayStore.shared.select("rt_1")

            AccountSwitchCleanup.resetLocalState(reason: reason)

            XCTAssertNil(ProjectStore.shared.selectedProjectId, "\(reason)")
            XCTAssertNil(InstanceStore.shared.selectedInstanceID, "\(reason)")
            XCTAssertNil(RuntimeRelayStore.shared.selectedRuntimeId, "\(reason)")
        }
    }

    @MainActor
    func testOnlyAPersonChangeDropsThePairedRuntimeAndProjectFolders() {
        let env = EnvironmentStore.shared
        let previous = (env.environment, env.pairedRuntimeId)
        defer { if let id = previous.1 { env.pair(withRuntimeId: id) } else { env.unpair() }; env.environment = previous.0 }

        env.pair(withRuntimeId: "rt_person_a")
        ProjectFolderStore.shared.set(.init(path: "/Users/a/site", name: "site"), for: "proj_1")
        AccountSwitchCleanup.resetLocalState(reason: .organizationChanged)
        XCTAssertEqual(env.pairedRuntimeId, "rt_person_a")
        XCTAssertEqual(ProjectFolderStore.shared.folder(for: "proj_1")?.path, "/Users/a/site")

        AccountSwitchCleanup.resetLocalState(reason: .personChanged)
        XCTAssertNil(env.pairedRuntimeId)
        XCTAssertNil(ProjectFolderStore.shared.folder(for: "proj_1"))
    }

    @MainActor
    func testCodeCanvasResetDropsTheLeavingAccountsTiles() throws {
        let json = #"{"id":"ses_1","created_at":"2026-11-17T10:00:00Z","name":"Fix checkout"}"#
        let session = try JSONDecoder().decode(AgentSession.self, from: Data(json.utf8))
        CodeCanvasStore.shared.syncTiles(from: [session])
        XCTAssertEqual(CodeCanvasStore.shared.activeWorkspace?.tiles.count, 1)

        CodeCanvasStore.shared.reset()

        XCTAssertEqual(CodeCanvasStore.shared.activeWorkspace?.tiles.count, 0)
        XCTAssertEqual(CodeCanvasStore.shared.activeWorkspace?.sessionIds, [])
    }

    @MainActor
    func testBYOCResetDropsErrorsAndResults() {
        let store = BYOCStore()
        store.setCreateError("Couldn't add the credential.")
        store.reset()
        XCTAssertNil(store.createError)
        XCTAssertNil(store.profile)
        XCTAssertTrue(store.credentials.isEmpty)
    }

    func testVoiceKeysLeaveThePhoneWithNoOptOutBehind() {
        VoiceProviderKeys.store(.openai, apiKey: "sk-test-1234", label: "OpenAI")
        VoiceProviderKeys.disconnect(.elevenlabs)
        XCTAssertTrue(VoiceProviderKeys.isOptedOut(.elevenlabs))

        VoiceProviderKeys.forgetAll()

        XCTAssertNil(VoiceProviderKeys.value(.openai))
        XCTAssertNil(VoiceProviderKeys.label(.openai))
        XCTAssertFalse(VoiceProviderKeys.isOptedOut(.elevenlabs), "the next person starts clean")
    }
}
