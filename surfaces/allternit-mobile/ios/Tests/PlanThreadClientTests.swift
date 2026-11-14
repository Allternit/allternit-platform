import XCTest
@testable import Allternit

final class PlanThreadStubProtocol: URLProtocol {
    nonisolated(unsafe) static var handler: ((URLRequest) -> (Int, String))?
    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        let (status, body) = Self.handler?(request) ?? (500, "")
        let response = HTTPURLResponse(url: request.url!, statusCode: status, httpVersion: nil, headerFields: nil)!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: Data(body.utf8))
        client?.urlProtocolDidFinishLoading(self)
    }
    override func stopLoading() {}
}

final class PlanThreadClientTests: XCTestCase {
    private func client(token: String? = "tok") -> PlanThreadClient {
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [PlanThreadStubProtocol.self]
        return PlanThreadClient(baseURL: URL(string: "https://cloud.test")!, session: URLSession(configuration: config),
                                tokenProvider: { token })
    }

    func testCreateListAndGetDecode() async throws {
        let summary = #"{"id":"pth_1","title":"Hi","model":"plan/claude/sonnet","message_count":1,"created_at":"2026-11-14T10:00:00Z","updated_at":"2026-11-14T10:00:01Z"}"#
        PlanThreadStubProtocol.handler = { request in
            XCTAssertEqual(request.value(forHTTPHeaderField: "Authorization"), "Bearer tok")
            switch (request.httpMethod ?? "", request.url!.path) {
            case ("POST", "/api/v1/plan-threads"): return (201, summary)
            case ("GET", "/api/v1/plan-threads"): return (200, #"{"threads":[\#(summary)]}"#)
            default: return (200, String(summary.dropLast()) + #","messages":[{"role":"user","content":"Hi","at":"2026-11-14T10:00:00Z"}]}"#)
            }
        }
        let c = client()
        let created = try await c.create(model: "plan/claude/sonnet", title: "Hi")
        XCTAssertEqual(created.id, "pth_1")
        let listed = try await c.list()
        XCTAssertEqual(listed.first?.messageCount, 1)
        XCTAssertEqual(listed.first?.asSession?.name, "Hi")
        let detail = try await c.get(id: "pth_1")
        XCTAssertEqual(detail.messages.first?.content, "Hi")
    }

    func testStoreFailureThrowsSoCallersFallBackToDevice() async {
        PlanThreadStubProtocol.handler = { _ in (403, #"{"error":"forbidden"}"#) }
        do { _ = try await client().list(); XCTFail("expected throw") } catch {}
        do { _ = try await client(token: nil).create(model: "plan/claude/sonnet"); XCTFail("expected throw") } catch {}
    }

    func testDeleteSendsDeleteAndTreatsMissingAsGone() async throws {
        var seen: [(String, String)] = []
        PlanThreadStubProtocol.handler = { request in
            seen.append((request.httpMethod ?? "", request.url!.path))
            return request.url!.path.hasSuffix("pth_gone") ? (404, #"{"error":"nope"}"#) : (204, "")
        }
        try await client().delete(id: "pth_1")
        try await client().delete(id: "pth_gone")
        XCTAssertEqual(seen.first?.0, "DELETE")
        XCTAssertEqual(seen.first?.1, "/api/v1/plan-threads/pth_1")
        PlanThreadStubProtocol.handler = { _ in (500, "") }
        do { try await client().delete(id: "pth_1"); XCTFail("expected throw") } catch {}
    }

    func testChatGPTOnDeviceChatsAreNotStored() {
        XCTAssertTrue(PlanThreadClient.isStored(model: "plan/claude/sonnet"))
        XCTAssertTrue(PlanThreadClient.isStored(model: "copilot/gpt"))
        XCTAssertFalse(PlanThreadClient.isStored(model: "chatgpt/gpt-5"))
    }
}
