import XCTest
import UIKit
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
        let picture = ChatErrorInfo.make(message: "Run gizzi connect chatgpt", code: "signed_out", lane: .picture)
        XCTAssertNil(picture.actionLabel)
        XCTAssertTrue(picture.isChatGPT)
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

/// Regression coverage for browser requests (empty code must not become executable HTML),
/// crash/replay submission fencing, foreground origin markers, and image byte validation.
final class ACIBrowserImageContractTests: XCTestCase {
    private func payload(_ changes: [String: Any] = [:]) throws -> String {
        var object: [String: Any] = ["id": "job-1", "sessionID": "chat-1", "toolCall": ["messageID": "message-1", "callID": "tool-1"],
            "kind": "browser-image", "engine": "chatgpt-aci", "prompt": "Draw a blue bird",
            "format": "html", "code": "", "width": 1024, "height": 1024, "timeoutMs": 600000, "time": Date().timeIntervalSince1970 * 1000]
        object.merge(changes) { _, new in new }
        return String(data: try JSONSerialization.data(withJSONObject: object), encoding: .utf8)!
    }

    func testBrowserRequestKeepsIdentityAndNeverFallsThroughToCode() throws {
        let request = try XCTUnwrap(PaneRenderRequest(payload: payload()))
        XCTAssertEqual(request.kind, .browserImage)
        XCTAssertEqual(request.engine, "chatgpt-aci")
        XCTAssertEqual(request.prompt, "Draw a blue bird")
        XCTAssertEqual(request.toolCall, "tool-1")
        XCTAssertEqual(request.toolMessageId, "message-1")
        XCTAssertEqual(request.sessionId, "chat-1")
        XCTAssertEqual(request.code, "")
        for changes: [String: Any] in [["kind": "unknown"], ["engine": "api-key"], ["prompt": " "],
            ["prompt": String(repeating: "a", count: 16001)], ["code": "alert(1)"],
            ["format": "canvas"], ["timeoutMs": 600001], ["width": 1e100]] {
            XCTAssertNil(PaneRenderRequest(payload: try payload(changes)))
        }
    }

    func testBrowserRequestRequiresNumericFreshTimeAndBoundedTimeout() throws {
        let now = Date().timeIntervalSince1970 * 1000
        for changes: [String: Any] in [
            ["time": NSNull()], ["time": "now"], ["time": true],
            ["time": now - 600_001], ["time": now + 31_000],
            ["timeoutMs": NSNull()], ["timeoutMs": false], ["timeoutMs": 0],
            ["timeoutMs": -1], ["timeoutMs": 600_001], ["timeoutMs": "600000"]
        ] {
            XCTAssertNil(PaneRenderRequest(payload: try payload(changes)))
        }
        XCTAssertNotNil(PaneRenderRequest(payload: try payload(["time": now - 1000])))
        XCTAssertNotNil(PaneRenderRequest(payload: try payload(["time": now + 29_000])))
    }

    func testDurableFenceRejectsReplayAndIdentityMutation() throws {
        let suite = "aci-image-test-" + UUID().uuidString
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let request = try XCTUnwrap(PaneRenderRequest(payload: payload()))
        XCTAssertTrue(ACIBrowserImageLedger(defaults: defaults).reserve(request))
        // A newly created ledger represents a process restart, not just an in-memory Set.
        XCTAssertFalse(ACIBrowserImageLedger(defaults: defaults).reserve(request))
        let changed = try XCTUnwrap(PaneRenderRequest(payload: payload(["sessionID": "another-chat", "prompt": "Changed"])))
        XCTAssertFalse(ACIBrowserImageLedger(defaults: defaults).reserve(changed))
    }

    func testOnlyExplicitForegroundSendGetsPersonMarker() {
        XCTAssertEqual(AgentChatClient.personTurn(personInitiated: true, isForeground: true), "person")
        XCTAssertNil(AgentChatClient.personTurn(personInitiated: false, isForeground: true))
        XCTAssertNil(AgentChatClient.personTurn(personInitiated: true, isForeground: false))
        XCTAssertNil(AgentChatClient.personTurn(personInitiated: false, isForeground: false))
    }

    @MainActor func testDownloadedImageReturnsActualDimensionsAndPNG() throws {
        let format = UIGraphicsImageRendererFormat(); format.scale = 1
        let png = try XCTUnwrap(UIGraphicsImageRenderer(size: CGSize(width: 23, height: 17), format: format)
            .image { ctx in UIColor.blue.setFill(); ctx.fill(CGRect(x: 0, y: 0, width: 23, height: 17)) }.pngData())
        let result = try ACIBrowserImageJob.normalize(png, mime: "image/png")
        XCTAssertTrue(result.ok)
        XCTAssertEqual(result.width, 23)
        XCTAssertEqual(result.height, 17)
        XCTAssertTrue(try XCTUnwrap(result.image).hasPrefix("data:image/png;base64,"))
        XCTAssertThrowsError(try ACIBrowserImageJob.normalize(png, mime: "text/html"))
        XCTAssertThrowsError(try ACIBrowserImageJob.normalize(Data("broken".utf8), mime: "image/png"))
        XCTAssertThrowsError(try ACIBrowserImageJob.normalize(Data(repeating: 0, count: 20 * 1024 * 1024 + 1), mime: "image/png"))
        let tooWide = try XCTUnwrap(UIGraphicsImageRenderer(size: CGSize(width: 4097, height: 1), format: format).image { _ in }.pngData())
        XCTAssertThrowsError(try ACIBrowserImageJob.normalize(tooWide, mime: "image/png"))
    }
}

final class ACILocalImageHistoryTests: XCTestCase {
    @MainActor func testOfflineResultSurvivesRestartInItsOriginalChat() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = ACILocalImageStore(directory: directory, owner: "person-a")
        let original = try store.createChat(history: [MessageRecord(id: "prior", role: "user", content: "Earlier chat")])
        let other = try store.createChat(history: [])
        try store.begin(sessionId: original, id: "local-request", messageId: "assistant-image", prompt: "A blue bird")
        let format = UIGraphicsImageRendererFormat(); format.scale = 1
        let png = try XCTUnwrap(UIGraphicsImageRenderer(size: CGSize(width: 23, height: 17), format: format).image { _ in }.pngData())
        let result = try ACIBrowserImageJob.normalize(png, mime: "image/png")
        try store.finish(sessionId: original, id: "local-request", owner: "person-a", result: result)
        let reopened = ACILocalImageStore(directory: directory, owner: "person-a")
        XCTAssertEqual(reopened.messages(sessionId: original).map(\.id), ["prior", "local-request-user", "assistant-image"])
        XCTAssertTrue(reopened.messages(sessionId: other).isEmpty)
        let entry = try XCTUnwrap(reopened.entry(messageId: "assistant-image"))
        XCTAssertEqual(entry.prompt, "A blue bird")
        let tool = try XCTUnwrap(reopened.messages(sessionId: original).last?.toolCalls.first)
        XCTAssertEqual(tool.input["kind"], "image")
        XCTAssertEqual(tool.input["engine"], "chatgpt-aci")
        let file = try XCTUnwrap(reopened.file(entry))
        XCTAssertEqual(file.pathExtension, "png")
        XCTAssertEqual(UIImage(contentsOfFile: file.path)?.size, CGSize(width: 23, height: 17))
        XCTAssertEqual(Set(reopened.sessions().map(\.id)), Set([original, other]))
        let otherAccount = ACILocalImageStore(directory: directory, owner: "person-b")
        XCTAssertTrue(otherAccount.messages(sessionId: original).isEmpty)
        XCTAssertThrowsError(try otherAccount.finish(sessionId: original, id: "local-request", owner: "person-a", result: result))
    }

    @MainActor func testRestartDoesNotResumeOrResendPendingTurn() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = ACILocalImageStore(directory: directory, owner: "a")
        let id = try store.createChat(history: [])
        try store.begin(sessionId: id, id: "pending", messageId: "assistant", prompt: "Draw a bird")
        let reopened = ACILocalImageStore(directory: directory, owner: "a")
        let messages = reopened.messages(sessionId: id)
        XCTAssertFalse(try XCTUnwrap(messages.last).isStreaming)
        XCTAssertEqual(messages.last?.toolCalls.first?.state, .failed)
        XCTAssertTrue(messages.last?.toolCalls.first?.error?.contains("will not be sent again") == true)
    }

    @MainActor func testFinalPngLimitIsIndependentOfSourceLimit() throws {
        let side = 1700
        var bytes = Data(count: side * side * 4)
        var seed: UInt32 = 12345
        bytes.withUnsafeMutableBytes { buffer in
            let values = buffer.bindMemory(to: UInt8.self)
            for index in values.indices { seed = seed &* 1664525 &+ 1013904223; values[index] = UInt8(truncatingIfNeeded: seed >> 24) }
        }
        let provider = try XCTUnwrap(CGDataProvider(data: bytes as CFData))
        let cg = try XCTUnwrap(CGImage(width: side, height: side, bitsPerComponent: 8, bitsPerPixel: 32,
            bytesPerRow: side * 4, space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.last.rawValue),
            provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent))
        let png = try XCTUnwrap(UIImage(cgImage: cg).pngData())
        XCTAssertGreaterThan(png.count, ACIBrowserImageJob.pngLimit)
        XCTAssertLessThan(png.count, ACIBrowserImageJob.sourceLimit)
        XCTAssertThrowsError(try ACIBrowserImageJob.normalize(png, mime: "image/png"))
    }
}
