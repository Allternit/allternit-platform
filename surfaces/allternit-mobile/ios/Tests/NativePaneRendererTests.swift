import XCTest
@testable import Allternit

/// The phone's answers to pane_render requests, run for real in the host app
/// (off-screen WKWebView, AVAssetWriter). Guards that iOS renders the same
/// requests web does (artifact_check, media_generate images and videos).
@MainActor
final class NativePaneRendererTests: XCTestCase {
    private func request(_ fields: [String: Any]) -> PaneRenderRequest {
        var payload: [String: Any] = ["request_id": "r-\(UUID().uuidString)", "session_id": "ses_1", "time": Date().timeIntervalSince1970 * 1000]
        payload.merge(fields) { $1 }
        let json = String(data: try! JSONSerialization.data(withJSONObject: payload), encoding: .utf8)!
        return PaneRenderRequest(payload: json)!
    }

    func testCheckFindsSidewaysScrollErrorsAndTakesPictures() async {
        let page = "<!doctype html><html><body style=\"margin:0;background:#0b1020\"><h1 style=\"color:#fff\">Hi</h1><div style=\"width:1200px;height:40px;background:red\"></div><script>undefinedFn()</script></body></html>"
        let r = request(["kind": "check", "format": "html", "code": page, "width": 1440, "height": 900,
                         "artifactKind": "page", "bodyFormat": "text/html", "widths": [390, 1440]])
        let result = await NativePaneRenderer.shared.run(r, client: AgentChatClient())
        XCTAssertTrue(result.ok, result.error ?? "")
        let checks = result.checks ?? []
        XCTAssertEqual(checks.map(\.width), [390, 1440])
        XCTAssertEqual(checks.first?.overflowX, 810)
        XCTAssertEqual(checks.last?.overflowX, 0)
        XCTAssertTrue(checks.first?.errors.contains { $0.contains("undefinedFn") } ?? false, "\(checks.first?.errors ?? [])")
        XCTAssertNotNil(checks.first?.image)
        XCTAssertEqual(checks.first?.blank, false)
    }

    func testCheckComparesWithReference() async {
        let page = "<!doctype html><html><body style=\"margin:0;background:#fff\"></body></html>"
        let ref = NativePaneRenderer.resized(UIGraphicsImageRenderer(size: CGSize(width: 39, height: 84)).image { ctx in
            UIColor.black.setFill(); ctx.fill(CGRect(x: 0, y: 0, width: 39, height: 28))
            UIColor.white.setFill(); ctx.fill(CGRect(x: 0, y: 28, width: 39, height: 56))
        }, width: 390, height: 844)
        let r = request(["kind": "check", "format": "html", "code": page, "width": 390, "height": 844,
                         "artifactKind": "page", "widths": [390], "reference": NativePaneRenderer.dataURL(png: ref)!])
        let result = await NativePaneRenderer.shared.run(r, client: AgentChatClient())
        let c = result.checks?.first
        XCTAssertEqual(c?.blank, true)
        XCTAssertLessThan(c?.similarity ?? 1, 0.8)
        XCTAssertEqual(c?.mismatch?.first?.hasPrefix("top"), true, "\(c?.mismatch ?? [])")
    }

    func testRefusesCanvasDesignsWithAReason() async {
        let r = request(["kind": "check", "format": "html", "code": "{\"artboards\":[]}", "width": 1440, "height": 900, "artifactKind": "design", "bodyFormat": "application/json"])
        let result = await NativePaneRenderer.shared.run(r, client: AgentChatClient())
        XCTAssertFalse(result.ok)
        XCTAssertTrue(result.error?.contains("canvas") ?? false)
    }

    func testRendersCanvasAndSvgImages() async throws {
        let canvas = await NativePaneRenderer.shared.run(request(["kind": "image", "format": "canvas", "width": 64, "height": 32,
            "code": "ctx.fillStyle = '#f00'; ctx.fillRect(0, 0, width, height);"]), client: AgentChatClient())
        XCTAssertTrue(canvas.ok, canvas.error ?? "")
        let img = try XCTUnwrap(canvas.image.flatMap(NativePaneRenderer.image(fromDataURL:))?.cgImage)
        XCTAssertEqual(img.width, 64)
        let svg = await NativePaneRenderer.shared.run(request(["kind": "image", "format": "svg", "width": 100, "height": 50,
            "code": "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 100 50\"><rect width=\"100\" height=\"50\" fill=\"#00f\"/></svg>"]), client: AgentChatClient())
        XCTAssertTrue(svg.ok, svg.error ?? "")
        let px = try XCTUnwrap(NativePaneRenderer.pixels(XCTUnwrap(svg.image.flatMap(NativePaneRenderer.image(fromDataURL:))?.cgImage), cols: 4, rows: 2))
        XCTAssertGreaterThan(Int(px[2]), 200) // blue
        XCTAssertLessThan(Int(px[0]), 40)
    }

    func testEncodesAVideoWithSound() async {
        // No computer to store it in the test, so the render gets as far as the
        // upload: that message only comes after a finished MP4.
        let r = request(["kind": "video", "format": "canvas", "width": 320, "height": 180, "duration": 1, "fps": 24, "motionBlur": 2,
                         "code": "ctx.fillStyle = 'hsl(' + (t * 360) + ',80%,50%)'; ctx.fillRect(0, 0, width, height);",
                         "audio": "var o = ctx.createOscillator(); var g = ctx.createGain(); g.gain.value = 0.2; o.connect(g).connect(ctx.destination); o.start(0); o.stop(duration);",
                         "timeoutMs": 120000])
        let result = await NativePaneRenderer.shared.run(r, client: AgentChatClient())
        XCTAssertFalse(result.ok)
        XCTAssertTrue(result.error?.hasPrefix("The video rendered but couldn't be saved") ?? false, result.error ?? "")
    }

    func testScenesThatThrowSayWhere() async {
        let r = request(["kind": "video", "format": "canvas", "width": 64, "height": 64, "duration": 1, "fps": 24,
                         "code": "if (t > 0.5) throw new Error('boom');", "timeoutMs": 60000])
        let result = await NativePaneRenderer.shared.run(r, client: AgentChatClient())
        XCTAssertTrue(result.error?.contains("boom") ?? false, result.error ?? "")
    }
}
