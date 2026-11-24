import XCTest
@testable import Allternit

/// Answers vendor requests in tests; no real keys, no network.
final class VoiceVendorStub: URLProtocol {
    nonisolated(unsafe) static var routes: [String: (Int, String)] = [:]

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        let path = request.url?.path ?? ""
        let (status, body) = Self.routes[path] ?? (404, "{}")
        let response = HTTPURLResponse(url: request.url!, statusCode: status, httpVersion: nil, headerFields: ["Content-Type": "application/json"])!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: Data(body.utf8))
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}
}

final class VoiceVendorClientTests: XCTestCase {
    private var client: VoiceVendorClient!

    override func setUp() {
        super.setUp()
        VoiceVendorStub.routes = [:]
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [VoiceVendorStub.self]
        let base = URL(string: "https://fake-vendor.test")!
        client = VoiceVendorClient(session: URLSession(configuration: config), openAIBase: base, elevenLabsBase: base)
    }

    func testOpenAIKeyThatWorks() async {
        VoiceVendorStub.routes["/v1/models"] = (200, #"{"data":[]}"#)
        let result = await client.checkKey(.openai, key: "sk-test-abcd")
        XCTAssertEqual(result, .ok(label: "sk-…abcd"))
    }

    func testOpenAIRejectsKey() async {
        VoiceVendorStub.routes["/v1/models"] = (401, #"{"error":{"code":"invalid_api_key"}}"#)
        guard case .rejected = await client.checkKey(.openai, key: "sk-bad") else { return XCTFail("expected rejected") }
    }

    func testElevenLabsLabelShowsPlanAndCharactersLeft() async {
        VoiceVendorStub.routes["/v1/user"] = (200, #"{"subscription":{"tier":"creator","character_count":500,"character_limit":10000}}"#)
        let result = await client.checkKey(.elevenlabs, key: "el-key")
        XCTAssertEqual(result, .ok(label: "Creator plan · 9,500 characters left"))
    }

    func testElevenLabsKeyWithoutUserReadIsRestricted() async {
        VoiceVendorStub.routes["/v1/user"] = (401, #"{"detail":{"status":"missing_permissions","message":"missing user_read"}}"#)
        VoiceVendorStub.routes["/v1/voices"] = (200, #"{"voices":[]}"#)
        let result = await client.checkKey(.elevenlabs, key: "el-restricted")
        XCTAssertEqual(result, .restricted(label: "restricted key"))
    }

    func testElevenLabsRejectsKey() async {
        VoiceVendorStub.routes["/v1/user"] = (401, #"{"detail":{"status":"invalid_api_key"}}"#)
        guard case .rejected = await client.checkKey(.elevenlabs, key: "el-bad") else { return XCTFail("expected rejected") }
    }

    func testListVoicesReadsPreviewURLs() async throws {
        VoiceVendorStub.routes["/v1/voices"] = (200, #"{"voices":[{"voice_id":"21m00Tcm4TlvDq8ikWAM","name":"Rachel","category":"premade","preview_url":"https://storage.googleapis.com/x/rachel.mp3"},{"voice_id":"abc","name":"Mine","preview_url":""}]}"#)
        let voices = try await client.listVoices(key: "el-key")
        XCTAssertEqual(voices.map(\.name), ["Rachel", "Mine"])
        XCTAssertEqual(voices[0].previewURL?.host, "storage.googleapis.com")
        XCTAssertNil(voices[1].previewURL)
    }
}
