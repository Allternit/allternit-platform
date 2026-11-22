import XCTest
@testable import Allternit

/// The tricky logic behind the generation experience: which typed error a
/// failure becomes (and what it says), when progress is sent, and how a
/// running call's metadata reads back. Mirrors the web tests of ai #1224 / #1229.
final class GenerationExperienceTests: XCTestCase {
    // MARK: errors

    func testKnownCodesBecomeTypedErrors() {
        let limit = ChatErrorInfo.make(message: "ChatGPT will not take more right now.", code: "subscription_sharing_usage_limit_exceeded")
        XCTAssertEqual(limit.kind, .usageLimit)
        XCTAssertEqual(limit.action, .pickModel)
        XCTAssertTrue(limit.isChatGPT)

        XCTAssertEqual(ChatErrorInfo.make(message: nil, code: "subscription_sharing_usage_unavailable").kind, .usageLimit)
        XCTAssertEqual(ChatErrorInfo.make(message: "x", code: "signed_out").kind, .signedOut)
        XCTAssertEqual(ChatErrorInfo.make(message: "x", code: "login_expired").action, .connect)
        XCTAssertEqual(ChatErrorInfo.make(message: "Failed to fetch", code: "network").kind, .network)
        XCTAssertEqual(ChatErrorInfo.make(message: "Boom", code: nil, status: 500).kind, .server)
        // A person-only plan answer is not a sign-in problem.
        XCTAssertEqual(ChatErrorInfo.make(message: "Your ChatGPT plan only answers messages you send yourself.", code: "plan_person_only").kind, .server)
    }

    func testSignedOutReasonNeverShowsTheCLICommand() {
        let info = ChatErrorInfo.make(message: "Run gizzi connect chatgpt", code: "signed_out")
        XCTAssertEqual(info.kind, .signedOut)
        XCTAssertFalse(info.reason.localizedCaseInsensitiveContains("gizzi"))
        XCTAssertEqual(info.actionLabel, "Connect ChatGPT")
    }

    func testUsageLimitTitleCarriesTheResetTimeOnlyWhenItIsInTheFuture() {
        let now = Date(timeIntervalSince1970: 1_800_000_000)
        var info = ChatErrorInfo.make(message: "limit", code: "usage_limit_reached")
        XCTAssertEqual(info.title(lane: .chat, now: now), "Usage limit reached")
        info.resetAt = now.addingTimeInterval(3600)
        XCTAssertTrue(info.title(lane: .chat, now: now).contains("resets at"))
        info.resetAt = now.addingTimeInterval(-60)
        XCTAssertEqual(info.title(lane: .chat, now: now), "Usage limit reached")
    }

    func testResetEpochAcceptsSecondsAndMilliseconds() {
        XCTAssertEqual(ChatErrorInfo.resetDate(epoch: 1_800_000_000), Date(timeIntervalSince1970: 1_800_000_000))
        XCTAssertEqual(ChatErrorInfo.resetDate(epoch: 1_800_000_000_000), Date(timeIntervalSince1970: 1_800_000_000))
        XCTAssertNil(ChatErrorInfo.resetDate(epoch: nil))
    }

    func testErrorFrameWithACodeDecodesTypedAndWithoutStaysPlain() throws {
        let typed = #"{"type":"error","error":"Limit","code":"subscription_sharing_usage_limit_exceeded","reset_at":1800000000}"#
        guard case .streamErrorFrame(let frame) = try JSONDecoder().decode(AgentChatEvent.self, from: Data(typed.utf8)) else {
            return XCTFail("a coded error frame is typed")
        }
        XCTAssertEqual(frame.info.kind, .usageLimit)
        XCTAssertEqual(frame.resetAt, Date(timeIntervalSince1970: 1_800_000_000))

        let plain = #"{"type":"error","error":"Stream broke"}"#
        guard case .streamError(let text) = try JSONDecoder().decode(AgentChatEvent.self, from: Data(plain.utf8)) else {
            return XCTFail("an error frame with no code stays plain")
        }
        XCTAssertEqual(text, "Stream broke")
    }

    // MARK: progress

    func testProgressIsSentOnceASecondButANewStageGoesAtOnce() {
        var throttle = GenerationProgressThrottle()
        XCTAssertTrue(throttle.shouldSend(stage: .rendering, now: 0))
        XCTAssertFalse(throttle.shouldSend(stage: .rendering, now: 0.4))
        XCTAssertFalse(throttle.shouldSend(stage: .rendering, now: 0.9))
        XCTAssertTrue(throttle.shouldSend(stage: .encoding, now: 0.95))
        XCTAssertFalse(throttle.shouldSend(stage: .encoding, now: 1.5))
        XCTAssertTrue(throttle.shouldSend(stage: .encoding, now: 2.0))
    }

    func testRunningCallMetadataReadsBackAndBadOnesAreDropped() {
        let json = #"{"progress":"Frame 3 of 90","progress_detail":{"lane":"motion","stage":"rendering","label":"Frame 3 of 90","frame":3,"frames":90,"preview":"data:image/jpeg;base64,AA=="}}"#
        let detail = GenerationProgressDetail.read(metadata: json)
        XCTAssertEqual(detail?.lane, .motion)
        XCTAssertEqual(GenerationProgressDetail.mainLine(detail, lane: .motion), "Frame 3 of 90")
        XCTAssertEqual(GenerationProgressDetail.barFraction(detail) ?? 0, 3.0 / 90.0, accuracy: 0.0001)
        XCTAssertNil(GenerationProgressDetail.read(metadata: #"{"progress_detail":{"lane":"nope","stage":"rendering"}}"#))
        XCTAssertNil(GenerationProgressDetail.read(metadata: #"{"progress_detail":{"lane":"picture","stage":"nope"}}"#))
        XCTAssertNil(GenerationProgressDetail.read(metadata: nil))
    }

    func testProgressBodyUsesTheWireNames() throws {
        let detail = GenerationProgressDetail(lane: .motion, stage: .downloading, label: "x", totalBytes: 10, etaSeconds: 2)
        let object = try JSONSerialization.jsonObject(with: JSONEncoder().encode(detail)) as? [String: Any]
        XCTAssertEqual(object?["total_bytes"] as? Int, 10)
        XCTAssertEqual(object?["eta_s"] as? Double, 2)
        XCTAssertNil(object?["preview"])
    }

    @MainActor
    func testStoreKeepsTheLastFourMotionThumbnailsAndClearsWhenTheCallEnds() {
        let store = GenerationProgressStore()
        for n in 1...6 {
            store.set("call", detail: GenerationProgressDetail(lane: .motion, stage: .rendering, label: "", frame: n, frames: 6, preview: "data:image/jpeg;base64,\(n)"))
        }
        XCTAssertEqual(store.entry("call")?.thumbs.count, GenerationProgressStore.maxThumbs)
        XCTAssertEqual(store.entry("call")?.thumbs.last, "data:image/jpeg;base64,6")
        var call = ToolCallRecord(id: "call", toolName: "media_generate", state: .done)
        store.sync(calls: [call], replyIsLive: true)
        XCTAssertNil(store.entry("call"))
        call.state = .running
        call.metadata = #"{"progress_detail":{"lane":"picture","stage":"drawing","label":"Drawing"}}"#
        store.sync(calls: [call], replyIsLive: false)
        XCTAssertNil(store.entry("call"), "a stopped reply leaves no spinner")
        store.sync(calls: [call], replyIsLive: true)
        XCTAssertNotNil(store.entry("call"))
    }

    func testBlockPulseGeometryMatchesTheWebNumbers() {
        XCTAssertEqual(BlockPulseMath.cell(width: 390, height: 219), 22)
        XCTAssertEqual(BlockPulseMath.cell(width: 80, height: 80), 12)
        XCTAssertEqual(BlockPulseMath.cell(width: 2000, height: 2000), 30)
        XCTAssertNil(BlockPulseMath.tile(since: 0.1, distance: 500, spread: 0.001))
        let peak = BlockPulseMath.tile(since: BlockPulseMath.rise * BlockPulseMath.tileSeconds, distance: 0, spread: 0.001)
        XCTAssertEqual(peak?.alpha ?? 0, 1, accuracy: 0.0001)
        XCTAssertEqual(peak?.scale ?? 0, 1, accuracy: 0.0001)
    }
}
