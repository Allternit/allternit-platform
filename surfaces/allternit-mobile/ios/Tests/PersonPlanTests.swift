import CryptoKit
import XCTest
@testable import Allternit

/// Plans with no computer (CONTRACT-connections-2026-11 §9): the 409
/// disclosure body, Sign in with ChatGPT's PKCE and registration rules, the
/// Responses stream, and keeping person-only models out of bot pickers.
final class PersonPlanTests: XCTestCase {

    // MARK: - §9.2 409 disclosure_required

    func testDisclosureRequiredKeepsServerTextAsSent() throws {
        let body = """
        {"error":{"code":"disclosure_required","disclosure":{"version":"2026-11-14.1",
          "title":"Before you connect your Claude plan",
          "body":["Your plan runs on an Allternit server, in a private space only your account uses.","Second."],
          "accept_label":"I understand, connect","decline_label":"Use a key instead"}}}
        """
        guard case .disclosureRequired(let d)? = ConnectionsError.parse(status: 409, data: Data(body.utf8)) else {
            return XCTFail("expected disclosure_required")
        }
        XCTAssertEqual(d.version, "2026-11-14.1")
        XCTAssertEqual(d.title, "Before you connect your Claude plan")
        XCTAssertEqual(d.body, ["Your plan runs on an Allternit server, in a private space only your account uses.", "Second."])
        XCTAssertEqual(d.acceptLabel, "I understand, connect")
        XCTAssertEqual(d.declineLabel, "Use a key instead")
    }

    func testOtherCodedRefusalsKeepTheirCodeAndPlainWords() {
        let busy = ConnectionsError.parse(status: 503, data: Data(#"{"error":{"code":"runner_busy","message":"Busy. Try again"}}"#.utf8))
        XCTAssertEqual(busy, .refused(status: 503, code: "runner_busy", message: "Busy. Try again"))
        XCTAssertEqual(ConnectionsClient.describe(busy!), "Busy. Try again")
        // A body with no code is left to the plain HTTP status.
        XCTAssertNil(ConnectionsError.parse(status: 500, data: Data(#"{"message":"boom"}"#.utf8)))
        // disclosure_required without the text can't be shown: treat as a refusal.
        XCTAssertEqual(ConnectionsError.parse(status: 409, data: Data(#"{"error":{"code":"disclosure_required"}}"#.utf8)),
                       .refused(status: 409, code: "disclosure_required", message: nil))
    }

    func testRunnerRowDecodesDisclosureAndPersonOnly() throws {
        let json = """
        {"version":1,"connections":[{"id":"grok","state":"on","runs_on":"runner","person_only":true,
          "disclosure":{"version":"2026-11-14.1","accepted_at":"2026-11-14T10:00:00Z"},
          "models":["plan/grok/grok-4"],"source":"cloud"}]}
        """
        let row = try JSONDecoder().decode(ConnectionsEnvelope.self, from: Data(json.utf8)).connections[0]
        XCTAssertTrue(row.personOnly)
        XCTAssertEqual(row.disclosure?.acceptedAt, "2026-11-14T10:00:00Z")
        XCTAssertEqual(row.runsOn, "runner")
    }

    // MARK: - §9.9 Sign in with ChatGPT

    func testPKCEChallengeIsS256OfVerifier() {
        // RFC 7636 appendix B.
        XCTAssertEqual(ChatGPTOAuth.challenge(for: "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
                       "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM")
        let pkce = ChatGPTOAuth.PKCE.make()
        XCTAssertFalse(pkce.verifier.contains("="))
        XCTAssertEqual(pkce.challenge, ChatGPTOAuth.challenge(for: pkce.verifier))
    }

    func testHostIdIsRFC7638ThumbprintURI() {
        // RFC 7638 §3.1 shape for EC keys: {"crv","kty","x","y"} in that order.
        let key = P256.Signing.PrivateKey()
        let x963 = key.publicKey.x963Representation
        let x = ChatGPTOAuth.base64url(x963.subdata(in: 1..<33))
        let y = ChatGPTOAuth.base64url(x963.subdata(in: 33..<65))
        let expected = ChatGPTOAuth.base64url(Data(SHA256.hash(data: Data("{\"crv\":\"P-256\",\"kty\":\"EC\",\"x\":\"\(x)\",\"y\":\"\(y)\"}".utf8))))
        XCTAssertEqual(ChatGPTOAuth.hostId(publicKeyX963: x963), "urn:ietf:params:oauth:jwk-thumbprint:sha-256:\(expected)")
        XCTAssertNil(ChatGPTOAuth.hostId(publicKeyX963: Data([1, 2, 3])))
    }

    private func query(_ url: URL) -> [String: String] {
        Dictionary(uniqueKeysWithValues: (URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems ?? []).map { ($0.name, $0.value ?? "") })
    }

    func testFirstSignInRegistersWithDynamicClientAndNameHint() {
        let url = ChatGPTOAuth.authorizeURL(issuedClientId: nil, hostId: "urn:ietf:params:oauth:jwk-thumbprint:sha-256:abc",
                                            state: "s1", nonce: "n1", challenge: "c1")
        XCTAssertEqual(url.host, "auth.openai.com")
        XCTAssertEqual(url.path, "/api/accounts/authorize")
        let q = query(url)
        XCTAssertEqual(q["client_id"], "dynamic_agent_client")
        XCTAssertEqual(q["agent_name_hint"], "Allternit")
        XCTAssertEqual(q["ext_agent_host_id"], "urn:ietf:params:oauth:jwk-thumbprint:sha-256:abc")
        XCTAssertEqual(q["response_type"], "code")
        XCTAssertEqual(q["redirect_uri"], "http://127.0.0.1:1455/auth/callback")
        XCTAssertEqual(q["scope"], "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct")
        XCTAssertEqual(q["resource"], "https://api.openai.com/v1")
        XCTAssertEqual(q["code_challenge_method"], "S256")
        XCTAssertEqual(q["code_challenge"], "c1")
        XCTAssertEqual(q["state"], "s1")
        XCTAssertEqual(q["nonce"], "n1")
    }

    func testLaterSignInUsesIssuedClientWithoutNameHint() {
        let q = query(ChatGPTOAuth.authorizeURL(issuedClientId: "oaiapp_123", hostId: "h", state: "s", nonce: "n", challenge: "c"))
        XCTAssertEqual(q["client_id"], "oaiapp_123")
        XCTAssertNil(q["agent_name_hint"])
    }

    func testCallbackRules() {
        let cb = URL(string: "http://127.0.0.1:1455/auth/callback?code=abc&scope=openid&state=s1&client_id=oaiapp_9")!
        XCTAssertTrue(ChatGPTOAuth.isCallback(cb))
        XCTAssertFalse(ChatGPTOAuth.isCallback(URL(string: "http://localhost:1455/auth/callback?code=x")!))
        XCTAssertFalse(ChatGPTOAuth.isCallback(URL(string: "http://127.0.0.1:1455/callback?code=x")!))
        XCTAssertTrue(ChatGPTOAuth.isCallback(URL(string: "http://127.0.0.1:54321/auth/callback?code=x")!))

        // Registration: the issued id comes back and is used from now on.
        XCTAssertEqual(ChatGPTOAuth.parseCallback(cb, expectedState: "s1", pendingClientId: nil),
                       .success(.init(code: "abc", clientId: "oaiapp_9")))
        // Registration without an issued id is incomplete; dynamic_agent_client is never kept.
        XCTAssertEqual(ChatGPTOAuth.parseCallback(URL(string: "http://127.0.0.1:1455/auth/callback?code=abc&state=s1")!, expectedState: "s1", pendingClientId: nil),
                       .failure(.registrationIncomplete))
        XCTAssertEqual(ChatGPTOAuth.parseCallback(URL(string: "http://127.0.0.1:1455/auth/callback?code=abc&state=s1&client_id=dynamic_agent_client")!, expectedState: "s1", pendingClientId: nil),
                       .failure(.registrationIncomplete))
        // Re-auth may omit client_id; a different one is rejected.
        XCTAssertEqual(ChatGPTOAuth.parseCallback(URL(string: "http://127.0.0.1:1455/auth/callback?code=abc&state=s1")!, expectedState: "s1", pendingClientId: "oaiapp_9"),
                       .success(.init(code: "abc", clientId: "oaiapp_9")))
        XCTAssertEqual(ChatGPTOAuth.parseCallback(cb, expectedState: "s1", pendingClientId: "oaiapp_other"), .failure(.clientMismatch))
        // State is checked first, also on a denial.
        XCTAssertEqual(ChatGPTOAuth.parseCallback(cb, expectedState: "other", pendingClientId: nil), .failure(.stateMismatch))
        XCTAssertEqual(ChatGPTOAuth.parseCallback(URL(string: "http://127.0.0.1:1455/auth/callback?error=access_denied&state=s1")!, expectedState: "s1", pendingClientId: nil),
                       .failure(.denied))
    }

    func testTokenFormsUseIssuedClientAndResource() {
        let exchange = Dictionary(uniqueKeysWithValues: ChatGPTOAuth.codeExchangeForm(code: "c", verifier: "v", clientId: "oaiapp_9"))
        XCTAssertEqual(exchange, ["grant_type": "authorization_code", "client_id": "oaiapp_9", "code": "c", "code_verifier": "v",
                                  "redirect_uri": "http://127.0.0.1:1455/auth/callback", "resource": "https://api.openai.com/v1"])
        let refresh = Dictionary(uniqueKeysWithValues: ChatGPTOAuth.refreshForm(refreshToken: "r", clientId: "oaiapp_9"))
        XCTAssertEqual(refresh, ["grant_type": "refresh_token", "client_id": "oaiapp_9", "refresh_token": "r", "resource": "https://api.openai.com/v1"])
        XCTAssertNil(refresh["scope"])
        let body = String(decoding: ChatGPTOAuth.formBody([("redirect_uri", "http://127.0.0.1:1455/auth/callback"), ("a", "b c+d")]), as: UTF8.self)
        XCTAssertEqual(body, "redirect_uri=http%3A%2F%2F127.0.0.1%3A1455%2Fauth%2Fcallback&a=b%20c%2Bd")
    }

    func testPlanScopeIsRequired() throws {
        let granted = try JSONDecoder().decode(ChatGPTOAuth.TokenResponse.self, from: Data(#"{"access_token":"a","scope":"openid offline_access chatgpt.tokens.use.direct"}"#.utf8))
        XCTAssertTrue(ChatGPTOAuth.grantedScopes(granted).contains(ChatGPTOAuth.directScope))
        let identityOnly = try JSONDecoder().decode(ChatGPTOAuth.TokenResponse.self, from: Data(#"{"access_token":"a","scope":"openid profile email"}"#.utf8))
        XCTAssertFalse(ChatGPTOAuth.grantedScopes(identityOnly).contains(ChatGPTOAuth.directScope))
    }

    func testIdTokenClaimChecks() {
        func jwt(_ claims: [String: Any]) -> String {
            let payload = ChatGPTOAuth.base64url(try! JSONSerialization.data(withJSONObject: claims))
            return "eyJhbGciOiJSUzI1NiJ9.\(payload).sig"
        }
        let future = Date().timeIntervalSince1970 + 600
        XCTAssertTrue(ChatGPTOAuth.idTokenIsValid(jwt(["iss": "https://auth.openai.com", "aud": "oaiapp_9", "exp": future, "nonce": "n"]), clientId: "oaiapp_9", nonce: "n"))
        XCTAssertFalse(ChatGPTOAuth.idTokenIsValid(jwt(["iss": "https://evil.example", "aud": "oaiapp_9", "exp": future, "nonce": "n"]), clientId: "oaiapp_9", nonce: "n"))
        XCTAssertFalse(ChatGPTOAuth.idTokenIsValid(jwt(["iss": "https://auth.openai.com", "aud": ["other"], "exp": future, "nonce": "n"]), clientId: "oaiapp_9", nonce: "n"))
        XCTAssertFalse(ChatGPTOAuth.idTokenIsValid(jwt(["iss": "https://auth.openai.com", "aud": "oaiapp_9", "exp": future, "nonce": "x"]), clientId: "oaiapp_9", nonce: "n"))
    }

    // MARK: - Responses stream and model list

    func testResponsesStreamFrames() {
        XCTAssertEqual(ResponsesStreamEvent.parse(#"{"type":"response.output_text.delta","delta":"Hi"}"#), .delta("Hi"))
        XCTAssertEqual(ResponsesStreamEvent.parse(#"{"type":"response.completed","response":{"id":"r"}}"#), .completed)
        XCTAssertEqual(ResponsesStreamEvent.parse(#"{"type":"response.failed","response":{"error":{"code":"subscription_sharing_usage_limit_exceeded","message":"limit"}}}"#),
                       .failed(code: "subscription_sharing_usage_limit_exceeded", message: "limit"))
        XCTAssertEqual(ResponsesStreamEvent.parse(#"{"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"}}}"#),
                       .incomplete(reason: "max_output_tokens"))
        XCTAssertEqual(ResponsesStreamEvent.parse(#"{"type":"response.created"}"#), .other)
        XCTAssertNil(ResponsesStreamEvent.parse("not json"))
        XCTAssertTrue(ResponsesStreamEvent.usageLimitCodes.contains("subscription_sharing_usage_unavailable"))
    }

    func testResponsesBodyIsStatelessStreamedAndToolFree() throws {
        let data = try PersonChatClient.responsesBody(model: "chatgpt/gpt-6.1-sol", messages: [PersonChatMessage(role: "user", content: "Hi")])
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(object["model"] as? String, "gpt-6.1-sol")
        XCTAssertEqual(object["store"] as? Bool, false)
        XCTAssertEqual(object["stream"] as? Bool, true)
        XCTAssertNil(object["tools"])
        XCTAssertEqual((object["input"] as? [[String: String]])?.first, ["role": "user", "content": "Hi"])
    }

    func testChatGPTModelListKeepsListedOnesInOrder() {
        let data = Data(#"{"models":[{"slug":"gpt-6.1-sol","display_name":"GPT-6.1 Sol","visibility":"list"},{"slug":"internal","display_name":"x","visibility":"hide"},{"slug":"gpt-6-mini","display_name":"GPT-6 mini","visibility":"list"}]}"#.utf8)
        XCTAssertEqual(ChatGPTModel.parseList(data).map(\.id), ["chatgpt/gpt-6.1-sol", "chatgpt/gpt-6-mini"])
    }

    // MARK: - Cloud plan stream

    func testCloudChunksAndRefusals() {
        XCTAssertEqual(PersonChatClient.parseCloudChunk(#"{"choices":[{"delta":{"content":"Hel"}}]}"#), .delta("Hel"))
        XCTAssertEqual(PersonChatClient.parseCloudChunk(#"{"choices":[{"delta":{"role":"assistant"}}]}"#), .none)
        XCTAssertEqual(PersonChatClient.parseCloudChunk(#"{"error":{"code":"login_expired","message":"Sign-in expired"}}"#),
                       .error(code: "login_expired", message: "Sign-in expired"))
        let refusal = PersonChatClient.refusal(status: 403, body: Data(#"{"error":{"code":"plan_person_only","message":"x"}}"#.utf8), vendor: "Claude")
        XCTAssertEqual(refusal.code, "plan_person_only")
        XCTAssertEqual(refusal.plainText, "Your Claude plan only answers messages you send yourself in chat.")
        XCTAssertEqual(PersonChatClient.refusal(status: 429, body: Data(), vendor: "Grok").code, "plan_rate")
    }

    // MARK: - Pickers

    func testPersonModelsComeFromConnectedRowsOnly() {
        var claude = ConnectionWire(id: "claude", state: .on, source: "cloud")
        claude.models = ["plan/claude/sonnet", "plan/claude/opus"]
        var grok = ConnectionWire(id: "grok", state: .off, source: "cloud")
        grok.models = ["plan/grok/grok-4"]
        var codexOnMac = ConnectionWire(id: "chatgpt", state: .on, source: "runtime:mac-1")
        codexOnMac.models = ["codex/gpt-5"]
        let built = PersonModels.build(rows: [grok, codexOnMac, claude], chatgpt: [ChatGPTModel(slug: "gpt-6-mini", name: "GPT-6 mini")])
        XCTAssertEqual(built.map(\.id), ["plan/claude/sonnet", "plan/claude/opus", "chatgpt/gpt-6-mini"])
        XCTAssertEqual(built.first?.name, "Sonnet")
    }

    func testBotAndRoutinePickersNeverOfferPersonModels() {
        let catalog = [
            RuntimeModel(id: "allternit/kimi-k3", name: "Kimi K3", provider: "allternit"),
            RuntimeModel(id: "plan/claude/sonnet", name: "Sonnet", provider: "plan-claude"),
            RuntimeModel(id: "copilot/gpt-5", name: "GPT-5", provider: "copilot"),
            RuntimeModel(id: "chatgpt/gpt-6-mini", name: "GPT-6 mini", provider: "chatgpt-phone"),
            RuntimeModel(id: "anthropic/claude-sonnet", name: "Claude Sonnet", provider: "anthropic"),
        ]
        XCTAssertEqual(PersonModels.forAutomation(catalog).map(\.id), ["allternit/kimi-k3", "anthropic/claude-sonnet"])
        XCTAssertTrue(PersonModels.isPersonOnly("plan/kimi/k2"))
        XCTAssertFalse(PersonModels.isPersonOnly("openai/gpt-5"))
    }

    func testPersonHistoryReplaysFinishedTurnsWithTheSentText() {
        var failed = MessageRecord(id: "a2", role: "assistant", content: "partial")
        failed.error = ChatError(name: nil, message: "x", raw: "x")
        let feed = [
            MessageRecord(id: "u1", role: "user", content: "Hi"),
            MessageRecord(id: "a1", role: "assistant", content: "Hello"),
            MessageRecord(id: "u2", role: "user", content: "Draft"),
            failed,
            MessageRecord(id: "u3", role: "user", content: "Search: weather"),
        ]
        XCTAssertEqual(ChatViewModel.personHistory(feed, text: "[web] Search: weather"),
                       [PersonChatMessage(role: "user", content: "Hi"), PersonChatMessage(role: "assistant", content: "Hello"),
                        PersonChatMessage(role: "user", content: "Draft"), PersonChatMessage(role: "user", content: "[web] Search: weather")])
    }
}
