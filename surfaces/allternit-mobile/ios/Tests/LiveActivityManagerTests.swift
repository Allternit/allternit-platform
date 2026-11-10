import XCTest
@testable import Allternit

@MainActor
final class LiveActivityManagerTests: XCTestCase {

    // MARK: - Loop Live Activity projection

    func testLoopContentState_countsIterations() {
        let loop = Loop(
            id: "loop-1",
            agentId: nil,
            command: "echo hello",
            exitCondition: nil,
            maxIterations: 5,
            iterationLog: [
                LoopIteration(iteration: 1, output: "hello", exitCode: 0, timestamp: "2026-08-19T12:00:00Z"),
                LoopIteration(iteration: 2, output: "hello", exitCode: 0, timestamp: "2026-08-19T12:00:01Z"),
            ],
            state: "running",
            timeCreated: 0,
            timeUpdated: 0,
            metric: nil,
            ledger: nil
        )

        let state = LoopLiveActivityManager.contentState(for: loop)

        XCTAssertEqual(state.iterationsCompleted, 2)
        XCTAssertEqual(state.maxIterations, 5)
        XCTAssertEqual(state.state, "running")
    }

    func testLoopContentState_zeroMaxIterations() {
        let loop = Loop(
            id: "loop-2",
            agentId: nil,
            command: "ping",
            exitCondition: nil,
            maxIterations: 0,
            iterationLog: [],
            state: "running",
            timeCreated: 0,
            timeUpdated: 0,
            metric: nil,
            ledger: nil
        )

        let state = LoopLiveActivityManager.contentState(for: loop)

        XCTAssertEqual(state.iterationsCompleted, 0)
        XCTAssertEqual(state.maxIterations, 0)
    }

    // MARK: - Bot Live Activity projection

    func testBotProjection_emptyEntries_returnsNil() {
        let state = BotLiveActivityManager.projectedContentState(from: [:])
        XCTAssertNil(state)
    }

    func testBotProjection_onlyIdleBots_returnsNil() {
        let entries: [String: BotStatusStore.Entry] = [
            "bot-1": entry(status: .idle)
        ]
        XCTAssertNil(BotLiveActivityManager.projectedContentState(from: entries))
    }

    func testBotProjection_workingBot_showsWorking() {
        let entries: [String: BotStatusStore.Entry] = [
            "bot-1": entry(status: .working, activityLabel: "Running")
        ]

        guard let state = BotLiveActivityManager.projectedContentState(from: entries) else {
            return XCTFail("Expected a content state")
        }

        XCTAssertEqual(state.status, "working")
        XCTAssertEqual(state.activityLabel, "Running")
        XCTAssertEqual(state.activeBotsCount, 1)
        XCTAssertEqual(state.attentionBotsCount, 0)
        XCTAssertNil(state.displayName)
    }

    func testBotProjection_waitingApprovalDominatesWorking() {
        let entries: [String: BotStatusStore.Entry] = [
            "bot-1": entry(status: .working),
            "bot-2": entry(status: .waitingApproval, pendingApprovalsCount: 3)
        ]

        guard let state = BotLiveActivityManager.projectedContentState(from: entries) else {
            return XCTFail("Expected a content state")
        }

        XCTAssertEqual(state.status, "waiting_approval")
        XCTAssertEqual(state.pendingApprovalsCount, 3)
        XCTAssertEqual(state.activeBotsCount, 2)
        XCTAssertEqual(state.attentionBotsCount, 1)
    }

    func testBotProjection_failedAndBlocked_attentionCountsBoth() {
        let entries: [String: BotStatusStore.Entry] = [
            "bot-1": entry(status: .failed),
            "bot-2": entry(status: .blocked),
            "bot-3": entry(status: .completed)
        ]

        guard let state = BotLiveActivityManager.projectedContentState(from: entries) else {
            return XCTFail("Expected a content state")
        }

        XCTAssertEqual(state.status, "blocked")
        XCTAssertEqual(state.activeBotsCount, 3)
        XCTAssertEqual(state.attentionBotsCount, 2)
    }

    func testBotProjection_offlineBotIgnored() {
        let entries: [String: BotStatusStore.Entry] = [
            "bot-1": entry(status: .offline),
            "bot-2": entry(status: .completed)
        ]

        guard let state = BotLiveActivityManager.projectedContentState(from: entries) else {
            return XCTFail("Expected a content state")
        }

        XCTAssertEqual(state.status, "completed")
        XCTAssertEqual(state.activeBotsCount, 1)
    }

    func testBotProjection_pinnedBotTakesPrecedence() {
        let entries: [String: BotStatusStore.Entry] = [
            "bot-1": entry(status: .working, activityLabel: "Indexing"),
            "bot-2": entry(status: .waitingApproval, pendingApprovalsCount: 1)
        ]
        let pinned = BotStatusStore.PinnedBot(botId: "bot-1", displayName: "Indexer")

        guard let state = BotLiveActivityManager.projectedContentState(from: entries, pinnedBot: pinned) else {
            return XCTFail("Expected a content state")
        }

        XCTAssertEqual(state.status, "working")
        XCTAssertEqual(state.activityLabel, "Indexing")
        XCTAssertEqual(state.displayName, "Indexer")
        XCTAssertEqual(state.activeBotsCount, 1)
        XCTAssertEqual(state.pendingApprovalsCount, 0)
    }

    func testBotProjection_pinnedBotIdleFallsBackToSummary() {
        let entries: [String: BotStatusStore.Entry] = [
            "bot-1": entry(status: .idle),
            "bot-2": entry(status: .working)
        ]
        let pinned = BotStatusStore.PinnedBot(botId: "bot-1", displayName: "IdleBot")

        guard let state = BotLiveActivityManager.projectedContentState(from: entries, pinnedBot: pinned) else {
            return XCTFail("Expected a content state")
        }

        XCTAssertEqual(state.status, "working")
        XCTAssertNil(state.displayName)
    }

    // MARK: - Server content state (cloud-api routes/live_activity.rs bot_content_state)

    private func decode(_ json: String) throws -> BotActivityAttributes.ContentState {
        try JSONDecoder().decode(BotActivityAttributes.ContentState.self, from: Data(json.utf8))
    }

    func testServerContentState_working() throws {
        let state = try decode("""
        {"status":"working","activityLabel":"Working","pendingApprovalsCount":0,"activeBotsCount":2,
         "attentionBotsCount":0,"displayName":"Raven","botId":"b1","sessionId":"s1","runtimeId":"rt_1",
         "step":"Reading mail","progress":0.4,"othersCount":1}
        """)
        XCTAssertEqual(state.status, "working")
        XCTAssertEqual(state.displayName, "Raven")
        XCTAssertEqual(state.step, "Reading mail")
        XCTAssertEqual(state.progress, 0.4)
        XCTAssertEqual(state.othersCount, 1)
        XCTAssertNil(state.question)
        XCTAssertFalse(state.showsInlineDecision)
    }

    func testServerContentState_needsApprovalInline() throws {
        let state = try decode("""
        {"status":"waiting_approval","activityLabel":"Waiting for approval","pendingApprovalsCount":1,
         "activeBotsCount":1,"attentionBotsCount":1,"displayName":"Raven","botId":"b1","inboxId":"approval:ap1",
         "sessionId":"s1","runtimeId":"rt_1","step":"Drafting the reply","othersCount":0,
         "question":"Send the invoice to Acme?","contentHash":"ab12","approveInline":true}
        """)
        XCTAssertTrue(state.showsInlineDecision)
        XCTAssertEqual(state.question, "Send the invoice to Acme?")
        XCTAssertEqual(state.inboxId, "approval:ap1")
        XCTAssertEqual(state.runtimeId, "rt_1")
        XCTAssertEqual(state.contentHash, "ab12")
    }

    func testServerContentState_needsApprovalPrivate_showsOpenOnly() throws {
        // Private, long or hashless questions arrive as the generic line with
        // no contentHash / approveInline; the inbox id still opens the item.
        let state = try decode("""
        {"status":"waiting_approval","activityLabel":"Waiting for approval","pendingApprovalsCount":1,
         "activeBotsCount":1,"attentionBotsCount":1,"displayName":"Raven","botId":"b1","inboxId":"approval:ap1",
         "sessionId":"s1","runtimeId":"rt_1","othersCount":0,"question":"Needs your approval"}
        """)
        XCTAssertFalse(state.showsInlineDecision)
        XCTAssertEqual(state.question, "Needs your approval")
        // Assert where Open lands, not one spelling: ":" may be percent-encoded.
        XCTAssertEqual(state.inboxURL.flatMap { DeepLink.parse(url: $0) }, .inboxItem("approval:ap1"))
    }

    func testMerge_dropsQuestionWhenLocalNoLongerNeedsOwner() throws {
        let server = try decode("""
        {"status":"waiting_approval","pendingApprovalsCount":1,"activeBotsCount":1,"attentionBotsCount":1,
         "botId":"b1","inboxId":"approval:ap1","question":"Send it?","contentHash":"ab12","approveInline":true,
         "step":"Drafting the reply"}
        """)
        var local = BotActivityAttributes.ContentState(
            status: "working", activityLabel: "Running", pendingApprovalsCount: 0,
            activeBotsCount: 1, attentionBotsCount: 0, displayName: "Raven"
        )
        local.botId = "b1"
        let merged = BotLiveActivityManager.merged(local: local, current: server)
        XCTAssertEqual(merged.status, "working")
        XCTAssertEqual(merged.step, "Drafting the reply")
        XCTAssertNil(merged.question)
        XCTAssertFalse(merged.showsInlineDecision)
    }

    // MARK: - Activity token upload

    func testTokenUploadBody_matchesCloudRoute() throws {
        let upload = ActivityTokenUpload(
            deviceId: "dev-1", kind: "la_update", attributesType: "LoopActivityAttributes",
            activityKey: "loop-1", token: "ab01", environment: "sandbox"
        )
        let body = String(decoding: try ActivityTokenClient.body(upload), as: UTF8.self)
        XCTAssertEqual(body, #"{"activityKey":"loop-1","attributesType":"LoopActivityAttributes","deviceId":"dev-1","environment":"sandbox","kind":"la_update","token":"ab01"}"#)

        let start = ActivityTokenUpload(
            deviceId: "dev-1", kind: "la_start", attributesType: "BotActivityAttributes",
            activityKey: nil, token: "ab01", environment: "production"
        )
        let startBody = String(decoding: try ActivityTokenClient.body(start), as: UTF8.self)
        XCTAssertEqual(startBody, #"{"attributesType":"BotActivityAttributes","deviceId":"dev-1","environment":"production","kind":"la_start","token":"ab01"}"#)
    }

    func testRegistry_uploadsHexTokenWhenSwitchOn() async {
        let (registry, transport, _) = makeRegistry(switchOn: nil)
        await registry.update(kind: .laUpdate, attributesType: BotLiveActivityManager.attributesType,
                              activityKey: BotLiveActivityManager.activityKey, token: Data([0xAB, 0x01, 0xFF]))
        let puts = await transport.puts
        XCTAssertEqual(puts, [ActivityTokenUpload(
            deviceId: "dev-1", kind: "la_update", attributesType: "BotActivityAttributes",
            activityKey: "summary", token: "ab01ff", environment: "sandbox"
        )])
    }

    func testRegistry_noUploadWhenSwitchOff_thenResyncWhenOn() async {
        let (registry, transport, defaults) = makeRegistry(switchOn: false)
        await registry.update(kind: .laStart, attributesType: "BotActivityAttributes", activityKey: nil, token: Data([1, 2]))
        let none = await transport.puts
        XCTAssertTrue(none.isEmpty)

        defaults.set(true, forKey: LiveActivityPushSetting.key)
        await registry.resync()
        let puts = await transport.puts
        XCTAssertEqual(puts.map(\.kind), ["la_start"])
    }

    func testRegistry_deleteAllRemovesUploadedTokens() async {
        let (registry, transport, _) = makeRegistry(switchOn: true)
        await registry.update(kind: .laUpdate, attributesType: "LoopActivityAttributes", activityKey: "loop-1", token: Data([0x0A]))
        await registry.deleteAll()
        let deletes = await transport.deletes
        XCTAssertEqual(deletes, ["0a"])
        XCTAssertTrue(registry.uploaded.isEmpty)
    }

    private func makeRegistry(switchOn: Bool?) -> (ActivityTokenRegistry, FakeActivityTokenTransport, UserDefaults) {
        let suite = "LiveActivityManagerTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defaults.removePersistentDomain(forName: suite)
        if let switchOn { defaults.set(switchOn, forKey: LiveActivityPushSetting.key) }
        let transport = FakeActivityTokenTransport()
        let registry = ActivityTokenRegistry(
            transport: transport, defaults: defaults, environment: "sandbox",
            deviceId: { "dev-1" }, isSignedIn: { true }
        )
        return (registry, transport, defaults)
    }

    // MARK: - Helpers

    private func entry(
        status: BotOperationalStatus,
        activityLabel: String? = nil,
        pendingApprovalsCount: Int = 0
    ) -> BotStatusStore.Entry {
        var state = BotOperationalState()
        state.status = status
        state.activityLabel = activityLabel
        state.pendingApprovalsCount = pendingApprovalsCount
        return BotStatusStore.Entry(
            state: state,
            subscriptionState: .connected,
            recentEvents: [],
            lastFetchedAt: Date(),
            loadError: nil
        )
    }
}

/// The network edge: records what would go to cloud-api.
private actor FakeActivityTokenTransport: ActivityTokenTransport {
    private(set) var puts: [ActivityTokenUpload] = []
    private(set) var deletes: [String] = []

    func put(_ upload: ActivityTokenUpload) async throws { puts.append(upload) }
    func delete(token: String) async throws { deletes.append(token) }
}
