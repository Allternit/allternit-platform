import XCTest
@testable import Allternit

/// Universal Links and push `url` paths land on the right screen.
final class DeepLinkTests: XCTestCase {
    func testUniversalLinks() {
        XCTAssertEqual(DeepLink.parse(url: URL(string: "https://m.allternit.com/b/bot_42")!), .bot("bot_42"))
        XCTAssertEqual(DeepLink.parse(url: URL(string: "https://ai.allternit.com/b/bot_42/threads")!), .bot("bot_42"))
        XCTAssertEqual(DeepLink.parse(url: URL(string: "https://ai.allternit.com/settings/billing")!), .settings("billing"))
        XCTAssertEqual(DeepLink.parse(url: URL(string: "https://m.allternit.com/?session=s-9")!), .session("s-9"))
        XCTAssertEqual(
            DeepLink.parse(url: URL(string: "https://m.allternit.com/i/x7k2p/welcome?jt=tok")!),
            .invite(URL(string: "https://m.allternit.com/i/x7k2p/welcome?jt=tok")!)
        )
        XCTAssertNil(DeepLink.parse(url: URL(string: "https://evil.example.com/b/bot_42")!))
        XCTAssertNil(DeepLink.parse(url: URL(fileURLWithPath: "/tmp/report.pdf")))
    }

    func testPushPaths() {
        XCTAssertEqual(DeepLink.parse(path: "/?allternit_thread=t-1&bot=b-1"), .thread(botId: "b-1", threadId: "t-1"))
        XCTAssertEqual(DeepLink.parse(path: "/?allternit_call=room&from=u&thread=t-2&bot=b-2"), .call(room: "room", from: "u", threadId: "t-2", botId: "b-2"))
        XCTAssertEqual(DeepLink.parse(path: "/?allternit_channel=slack"), .channel("slack"))
        XCTAssertEqual(DeepLink.parse(path: "/settings/billing"), .settings("billing"))
        XCTAssertNil(DeepLink.parse(path: "/"))
        XCTAssertEqual(DeepLink.parse(path: "/computers"), .computers)
        XCTAssertEqual(DeepLink.parse(path: "/computers/inst_7"), .computerConsole("inst_7"))
        XCTAssertEqual(DeepLink.parse(path: "/computers/inst_7?run=run_9"), .computerRun(computerId: "inst_7", runId: "run_9"))
    }

    /// "Do with Allternit" hands over to the in-app bar with this link; the
    /// app id keeps its colon and every value arrives as typed.
    func testBarLinkFromDoIntent() {
        let app = BarAppInfo(kind: "connector", ref: "mcp_stripe", name: "Stripe", level: "app", toolCount: 3)
        let link = DoWording.barLink(app: app, tool: "refund_payment",
                                     values: ["order_id": .text("1042 & 1043"), "amount": .number(320)],
                                     botId: "refunds")
        XCTAssertEqual(DeepLink.parse(url: link), .bar(BarLinkRequest(
            app: "connector:mcp_stripe", tool: "refund_payment", bot: "refunds",
            values: ["order_id": "1042 & 1043", "amount": "320"])))
        XCTAssertEqual(DeepLink.parse(url: URL(string: "allternit://bar")!),
                       .bar(BarLinkRequest(app: nil, tool: nil, values: [:])))
    }
}
