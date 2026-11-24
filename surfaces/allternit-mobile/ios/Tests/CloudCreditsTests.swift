import XCTest
@testable import Allternit

/// CONTRACT-cloud-credits-2026-11-15 on the phone: refusal decoding, the
/// entitlement and its per-row lock, and the no-computer Cloud path.
@MainActor
final class CloudCreditsTests: XCTestCase {
    // MARK: - 402 refusals (D3)

    func testCreditsRefusalsDecodeFromTheNestedBody() {
        let required = PersonChatClient.refusal(
            status: 402,
            body: Data(#"{"error":{"type":"billing_error","code":"credits_required","message":"Allternit Cloud models need credits. Buy credits or choose a plan.","balance_usd":0,"packs":[]}}"#.utf8),
            vendor: "Allternit Cloud")
        XCTAssertEqual(required.code, "credits_required")
        XCTAssertTrue(required.isCreditsRefusal)
        XCTAssertEqual(required.plainText, "Allternit Cloud models need credits.")

        let exhausted = PersonChatClient.refusal(
            status: 402,
            body: Data(#"{"error":{"type":"billing_error","code":"credits_exhausted","message":"Your Allternit Cloud credits are used up. Add credits to keep going."}}"#.utf8),
            vendor: "Allternit Cloud")
        XCTAssertEqual(exhausted.code, "credits_exhausted")
        XCTAssertEqual(exhausted.plainText, "Your Allternit Cloud credits are used up.")
    }

    func testAny402IsACreditsRefusalAndA403StaysPersonOnly() {
        // A flat ApiError body or none at all still opens the credits sheet.
        XCTAssertEqual(PersonChatClient.refusal(status: 402, body: Data(#"{"error":"Payment required"}"#.utf8), vendor: "Allternit Cloud").code, "credits_required")
        XCTAssertEqual(PersonChatClient.refusal(status: 402, body: Data(), vendor: "Allternit Cloud").code, "credits_required")
        let forbidden = PersonChatClient.refusal(status: 403, body: Data(), vendor: "Claude")
        XCTAssertEqual(forbidden.code, "plan_person_only")
        XCTAssertFalse(forbidden.isCreditsRefusal)
    }

    func testRelayedCreditsErrorsAreRecognizedFromProse() {
        XCTAssertEqual(CloudCreditsGate.reason(inRelayedError: "Upstream returned 402 Payment Required"), "credits_required")
        XCTAssertEqual(CloudCreditsGate.reason(inRelayedError: "Your Allternit Cloud credits are used up. Add credits to keep going."), "credits_exhausted")
        XCTAssertNil(CloudCreditsGate.reason(inRelayedError: "request 4021 timed out"))
        XCTAssertNil(CloudCreditsGate.reason(inRelayedError: "Your Claude plan only answers messages you send yourself in chat."))
    }

    // MARK: - Entitlement (D4, D5, D10)

    func testMissingCloudModelsMeansNotLocked() throws {
        let old = try JSONDecoder().decode(BillingCredits.self, from: Data(#"{"balance_usd":0,"month_to_date_usage_usd":0,"free_inference":{"monthly_allowance_usd":2,"used_usd":0,"remaining_usd":2}}"#.utf8))
        XCTAssertNil(old.cloudModels)
        XCTAssertFalse(CloudModelsEntitlement.isLocked(old.cloudModels, provider: "deepinfra"))
    }

    func testLockedRowsFollowAllowedAndOwnKeys() throws {
        let credits = try JSONDecoder().decode(BillingCredits.self, from: Data(#"{"balance_usd":0,"month_to_date_usage_usd":0,"cloud_models":{"allowed":false,"reason":"credits_required","balance_usd":0,"byok_providers":["openrouter"]}}"#.utf8))
        let cloud = try XCTUnwrap(credits.cloudModels)
        XCTAssertEqual(cloud.reason, "credits_required")
        XCTAssertTrue(CloudModelsEntitlement.isLocked(cloud, provider: "deepinfra"))
        XCTAssertFalse(CloudModelsEntitlement.isLocked(cloud, provider: "openrouter"), "a row served by the person's own key is never locked")
        XCTAssertTrue(CloudModelsEntitlement.isLocked(cloud, provider: nil))

        let allowed = try JSONDecoder().decode(CloudModelsEntitlement.self, from: Data(#"{"allowed":true,"reason":null}"#.utf8))
        XCTAssertEqual(allowed.byokProviders, [])
        XCTAssertFalse(CloudModelsEntitlement.isLocked(allowed, provider: "deepinfra"))
    }

    // MARK: - No-computer Cloud chat (D7, D8, D14)

    func testCatalogRowsCarryProviderPricingAndSkipHiddenAndOpenLane() throws {
        let body = #"{"object":"list","data":[{"id":"glm-5.3","name":"GLM 5.3","provider":"deepinfra","default":true,"lane":"standard","pricing":{"input_per_1m_usd":0.9,"output_per_1m_usd":3.3}},{"id":"old-model","name":"Old","provider":"deepinfra","hidden":true,"lane":"standard"},{"id":"open-x","name":"Open X","provider":"open","lane":"open"},{"id":"bare","owned_by":"openrouter"}]}"#
        let rows = SetupAPI.cloudModelRows(try JSONDecoder().decode(JSONValue.self, from: Data(body.utf8)))
        XCTAssertEqual(rows.count, 4)
        XCTAssertEqual(rows[0].provider, "deepinfra")
        XCTAssertEqual(rows[0].inputPer1mUsd, 0.9)
        XCTAssertEqual(rows[0].outputPer1mUsd, 3.3)
        XCTAssertEqual(rows[3].provider, "openrouter")
        XCTAssertNil(rows[3].inputPer1mUsd)

        let offered = ModelStore.cloudChatModels(rows)
        XCTAssertEqual(offered.map(\.id), ["allternit/glm-5.3", "allternit/bare"])
        XCTAssertTrue(offered[0].isDefault)
        XCTAssertEqual(offered[0].provider, "allternit")
    }

    func testCloudTurnsAreStoredAndSentBare() throws {
        XCTAssertTrue(PlanThreadClient.isStored(model: "allternit/glm-5.3"))
        XCTAssertTrue(PlanThreadClient.isStored(model: "plan/claude/sonnet"))
        XCTAssertFalse(PlanThreadClient.isStored(model: "chatgpt/gpt-5"))
        XCTAssertFalse(PlanThreadClient.isStored(model: "allternit/"))

        let body = try PersonChatClient.cloudBody(model: "allternit/glm-5.3", messages: [.init(role: "user", content: "hi")])
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: body) as? [String: Any])
        XCTAssertEqual(object["model"] as? String, "glm-5.3")
        let plan = try PersonChatClient.cloudBody(model: "plan/claude/sonnet", messages: [])
        XCTAssertEqual((try JSONSerialization.jsonObject(with: plan) as? [String: Any])?["model"] as? String, "plan/claude/sonnet")
        XCTAssertEqual(PersonModels.vendorName(for: "allternit/glm-5.3"), "Allternit Cloud")
        XCTAssertFalse(PersonModels.isPersonOnly("allternit/glm-5.3"), "bots and routines still get Cloud models from the computer")
    }

    func testWithNoComputerCloudRowsAreOfferedAndRouteDirect() {
        let defaults = UserDefaults(suiteName: "CloudCreditsTests-\(UUID().uuidString)")!
        let store = ModelStore(defaults: defaults)
        let catalog = [SetupModelRow(id: "glm-5.3", name: "GLM 5.3", provider: "deepinfra", lane: "standard")]

        store.setFixtures(models: [], cloudCatalog: catalog)
        XCTAssertEqual(store.chatModels.map(\.id), ["allternit/glm-5.3"])
        XCTAssertTrue(store.routesDirect("allternit/glm-5.3"))
        XCTAssertFalse(store.routesDirect("plan/claude/sonnet"))
        XCTAssertEqual(store.cloudRow(for: "allternit/glm-5.3")?.provider, "deepinfra")

        // With a computer, its catalog wins and Cloud models keep the computer path.
        store.setFixtures(models: [RuntimeModel(id: "allternit/glm-5.3", name: "GLM 5.3", provider: "allternit")], cloudCatalog: catalog)
        XCTAssertEqual(store.chatModels.map(\.id), ["allternit/glm-5.3"])
        XCTAssertFalse(store.routesDirect("allternit/glm-5.3"))
    }

    // MARK: - Sheet copy (D11, D12)

    func testCopyVariantsAndPriceLine() {
        XCTAssertEqual(CloudCreditsCopy.title("credits_required"), "Allternit Cloud models need credits")
        XCTAssertEqual(CloudCreditsCopy.title("credits_exhausted"), "Your Cloud credits are used up")
        XCTAssertEqual(CloudCreditsCopy.title(nil), "Allternit Cloud models need credits")
        XCTAssertEqual(CloudCreditsCopy.priceLine(model: "GLM 5.3", input: 0.9, output: 3.3), "GLM 5.3: $0.90 per 1M input, $3.30 per 1M output")
        XCTAssertNil(CloudCreditsCopy.priceLine(model: "GLM 5.3", input: nil, output: 3.3))
        // The no-link line carries no price.
        XCTAssertFalse(CloudCreditsCopy.line("credits_required").contains("$"))
        XCTAssertFalse(CloudCreditsCopy.line("credits_exhausted").contains("$"))
    }

    // MARK: - Claude organization refusal: "Use Allternit credits"

    func test_credits_fallback_picks_last_cloud_model() {
        let listed = ["allternit/glm-5.3", "allternit/kimi-k3"]
        XCTAssertEqual(CreditsFallback.modelId(recent: "allternit/kimi-k3", listed: listed), "allternit/kimi-k3")
        // A recent model no longer listed falls back to the first listed one.
        XCTAssertEqual(CreditsFallback.modelId(recent: "allternit/gone", listed: listed), "allternit/glm-5.3")
        XCTAssertEqual(CreditsFallback.modelId(recent: nil, listed: listed), "allternit/glm-5.3")
    }

    func test_no_cloud_model_hides_button() {
        XCTAssertNil(CreditsFallback.modelId(recent: "allternit/glm-5.3", listed: []))
        XCTAssertNil(CreditsFallback.modelId(recent: nil, listed: []))
        // Only claude_link_* codes get actions; Link again only when disconnected.
        XCTAssertTrue(ChatErrorAction.forClaudeLink(code: "login_expired").isEmpty)
        XCTAssertEqual(ChatErrorAction.forClaudeLink(code: "claude_link_spend_limit").map(\.kind), [.useCredits, .manageClaudeOrg])
        XCTAssertEqual(ChatErrorAction.forClaudeLink(code: "claude_link_disconnected").map(\.kind), [.useCredits, .linkAgain, .manageClaudeOrg])
    }
}
