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
        XCTAssertEqual(DeepLink.parse(path: "/?allternit_call=room&from=u&thread=t-2&bot=b-2"), .thread(botId: "b-2", threadId: "t-2"))
        XCTAssertEqual(DeepLink.parse(path: "/?allternit_channel=slack"), .channel("slack"))
        XCTAssertEqual(DeepLink.parse(path: "/settings/billing"), .settings("billing"))
        XCTAssertNil(DeepLink.parse(path: "/"))
    }
}
