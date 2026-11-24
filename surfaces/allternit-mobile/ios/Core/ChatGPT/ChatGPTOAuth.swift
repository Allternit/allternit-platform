import CryptoKit
import Foundation

// Sign in with ChatGPT on this iPhone (CONTRACT-connections-2026-11 §9.9),
// OpenAI's open-source token-sharing path:
// https://developers.openai.com/siwc/token-sharing-open-source/sign-in
//
// PKCE, no client secret. The first sign-in on a phone registers a client
// (`client_id=dynamic_agent_client`, `agent_name_hint=Allternit`); the
// callback returns the issued `oaiapp_…` id, which every later sign-in and
// every token call uses. The redirect is the loopback URL OpenAI expects; the
// in-app web view catches the navigation to it, so nothing listens on the
// phone. Tokens stay in this iPhone's Keychain and never go to cloud-api.
//
// This file holds only the pure parts (URLs, forms, parsing), so they can be
// tested without a network.
enum ChatGPTOAuth {
    static let authorizeEndpoint = URL(string: "https://auth.openai.com/api/accounts/authorize")!
    static let tokenEndpoint = URL(string: "https://auth.openai.com/api/accounts/oauth/token")!
    static let discoveryEndpoint = URL(string: "https://auth.openai.com/.well-known/openid-configuration")!
    static let issuer = "https://auth.openai.com"
    static let resource = "https://api.openai.com/v1"
    static let scope = "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct"
    /// The scope that lets the token spend the person's ChatGPT plan.
    static let directScope = "chatgpt.tokens.use.direct"
    static let dynamicClientId = "dynamic_agent_client"
    static let agentName = "Allternit"
    /// Loopback IP, never `localhost` (sign-in page, "Redirect URI rules").
    static let redirectURI = "http://127.0.0.1:1455/auth/callback"

    // MARK: - PKCE and random values

    struct PKCE: Equatable, Sendable {
        let verifier: String
        let challenge: String

        static func make() -> PKCE {
            let verifier = ChatGPTOAuth.randomToken()
            return PKCE(verifier: verifier, challenge: ChatGPTOAuth.challenge(for: verifier))
        }
    }

    /// S256: base64url(SHA-256(verifier)), no padding.
    static func challenge(for verifier: String) -> String {
        base64url(Data(SHA256.hash(data: Data(verifier.utf8))))
    }

    static func randomToken(bytes: Int = 32) -> String {
        var data = Data(count: bytes)
        let status = data.withUnsafeMutableBytes { SecRandomCopyBytes(kSecRandomDefault, bytes, $0.baseAddress!) }
        if status != errSecSuccess {
            data = Data((0..<bytes).map { _ in UInt8.random(in: 0...255) })
        }
        return base64url(data)
    }

    static func base64url(_ data: Data) -> String {
        data.base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }

    static func base64urlDecode(_ text: String) -> Data? {
        var s = text.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
        while s.count % 4 != 0 { s += "=" }
        return Data(base64Encoded: s)
    }

    // MARK: - Host id

    /// `ext_agent_host_id`: the RFC 9278 JWK-thumbprint URI of this phone's
    /// P-256 key (contract §9.9 step 2). `x963` is the uncompressed public
    /// key (0x04 ‖ X ‖ Y). The thumbprint is RFC 7638: SHA-256 over the
    /// required members in lexicographic order, no spaces.
    static func hostId(publicKeyX963 x963: Data) -> String? {
        guard x963.count == 65, x963.first == 0x04 else { return nil }
        let x = base64url(x963.subdata(in: 1..<33))
        let y = base64url(x963.subdata(in: 33..<65))
        let jwk = "{\"crv\":\"P-256\",\"kty\":\"EC\",\"x\":\"\(x)\",\"y\":\"\(y)\"}"
        let thumbprint = base64url(Data(SHA256.hash(data: Data(jwk.utf8))))
        return "urn:ietf:params:oauth:jwk-thumbprint:sha-256:\(thumbprint)"
    }

    // MARK: - Authorize

    /// The page the web view opens. `issuedClientId` nil = first sign-in on
    /// this phone (registration): `dynamic_agent_client` + the name hint.
    static func authorizeURL(issuedClientId: String?, hostId: String, state: String, nonce: String,
                             challenge: String, redirectURI: String = redirectURI) -> URL {
        var items: [URLQueryItem] = [
            URLQueryItem(name: "client_id", value: issuedClientId ?? dynamicClientId),
        ]
        if issuedClientId == nil { items.append(URLQueryItem(name: "agent_name_hint", value: agentName)) }
        items += [
            URLQueryItem(name: "ext_agent_host_id", value: hostId),
            URLQueryItem(name: "response_type", value: "code"),
            URLQueryItem(name: "redirect_uri", value: redirectURI),
            URLQueryItem(name: "scope", value: scope),
            URLQueryItem(name: "resource", value: resource),
            URLQueryItem(name: "state", value: state),
            URLQueryItem(name: "nonce", value: nonce),
            URLQueryItem(name: "code_challenge_method", value: "S256"),
            URLQueryItem(name: "code_challenge", value: challenge),
        ]
        var components = URLComponents(url: authorizeEndpoint, resolvingAgainstBaseURL: false)!
        components.queryItems = items
        // URLComponents leaves `+`, `:` and `/` alone in values; encode them so
        // the scope's spaces and the host id survive any server parser.
        components.percentEncodedQuery = components.percentEncodedQuery?
            .replacingOccurrences(of: "+", with: "%2B")
        return components.url!
    }

    // MARK: - Callback

    struct Callback: Equatable, Sendable {
        let code: String
        /// The issued client id to save and use from now on.
        let clientId: String
    }

    enum CallbackError: Error, Equatable, Sendable {
        case stateMismatch
        case denied
        case noCode
        /// First sign-in, but the callback carried no issued client id.
        case registrationIncomplete
        /// A later sign-in came back with a different client id.
        case clientMismatch
        case vendor(String)
    }

    /// True for the loopback redirect the web view must catch. Only the port
    /// may differ from `redirectURI`.
    static func isCallback(_ url: URL, redirectURI: String = redirectURI) -> Bool {
        guard let expected = URL(string: redirectURI) else { return false }
        return url.scheme == expected.scheme && url.host == expected.host && url.path == expected.path
    }

    /// Reads the redirect (sign-in page, "Callback"). `pendingClientId` is
    /// the issued id the authorize request used, nil when it registered.
    static func parseCallback(_ url: URL, expectedState: String, pendingClientId: String?) -> Result<Callback, CallbackError> {
        let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems ?? []
        func value(_ name: String) -> String? {
            items.first { $0.name == name }?.value.flatMap { $0.isEmpty ? nil : $0 }
        }
        guard value("state") == expectedState else { return .failure(.stateMismatch) }
        if let error = value("error") {
            return .failure(error == "access_denied" ? .denied : .vendor(value("error_description") ?? error))
        }
        guard let code = value("code") else { return .failure(.noCode) }
        let returned = value("client_id")
        if let pendingClientId {
            if let returned, returned != pendingClientId { return .failure(.clientMismatch) }
            return .success(Callback(code: code, clientId: pendingClientId))
        }
        guard let returned, returned != dynamicClientId else { return .failure(.registrationIncomplete) }
        return .success(Callback(code: code, clientId: returned))
    }

    // MARK: - Token calls

    static func codeExchangeForm(code: String, verifier: String, clientId: String, redirectURI: String = redirectURI) -> [(String, String)] {
        [("grant_type", "authorization_code"), ("client_id", clientId), ("code", code),
         ("code_verifier", verifier), ("redirect_uri", redirectURI), ("resource", resource)]
    }

    /// No `scope`: the existing grant is kept (Accounts and sessions, "Refreshing tokens").
    static func refreshForm(refreshToken: String, clientId: String) -> [(String, String)] {
        [("grant_type", "refresh_token"), ("client_id", clientId), ("refresh_token", refreshToken), ("resource", resource)]
    }

    static func revokeForm(refreshToken: String, clientId: String) -> [(String, String)] {
        [("token", refreshToken), ("token_type_hint", "refresh_token"), ("client_id", clientId)]
    }

    /// `application/x-www-form-urlencoded`, every value escaped.
    static func formBody(_ fields: [(String, String)]) -> Data {
        var allowed = CharacterSet.alphanumerics
        allowed.insert(charactersIn: "-._~")
        let text = fields.map { key, value in
            "\(key.addingPercentEncoding(withAllowedCharacters: allowed) ?? key)=\(value.addingPercentEncoding(withAllowedCharacters: allowed) ?? value)"
        }.joined(separator: "&")
        return Data(text.utf8)
    }

    struct TokenResponse: Decodable, Sendable {
        let accessToken: String
        let refreshToken: String?
        let idToken: String?
        let expiresIn: Double?
        let scope: String?

        private enum CodingKeys: String, CodingKey {
            case accessToken = "access_token", refreshToken = "refresh_token", idToken = "id_token"
            case expiresIn = "expires_in", scope
        }
    }

    /// The scopes the token response granted; falls back to the access
    /// token's `scope` claim when the response leaves the field out.
    static func grantedScopes(_ response: TokenResponse) -> Set<String> {
        let text = response.scope ?? (jwtClaims(response.accessToken)?["scope"] as? String) ?? ""
        return Set(text.split(separator: " ").map(String.init))
    }

    /// The JWT payload, unverified.
    static func jwtClaims(_ jwt: String) -> [String: Any]? {
        let parts = jwt.split(separator: ".")
        guard parts.count >= 2, let data = base64urlDecode(String(parts[1])) else { return nil }
        return (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
    }

    /// ID token claim checks: `iss`, `aud`, `exp`, `nonce`. The token comes
    /// straight from the token endpoint over TLS, so the signature check is
    /// left to TLS (OIDC Core §3.1.3.7, item 6).
    static func idTokenIsValid(_ idToken: String, clientId: String, nonce: String, now: Date = Date()) -> Bool {
        guard let claims = jwtClaims(idToken) else { return false }
        guard claims["iss"] as? String == issuer else { return false }
        let audience: [String]
        if let one = claims["aud"] as? String { audience = [one] } else { audience = claims["aud"] as? [String] ?? [] }
        guard audience.contains(clientId) else { return false }
        if let exp = claims["exp"] as? Double, Date(timeIntervalSince1970: exp) < now { return false }
        if let tokenNonce = claims["nonce"] as? String, tokenNonce != nonce { return false }
        return true
    }
}

/// One model the ChatGPT plan offers (`GET https://api.openai.com/v1/models`).
struct ChatGPTModel: Equatable, Sendable, Identifiable {
    /// `slug`, passed as `model` on `/v1/responses`.
    let slug: String
    let name: String
    /// The picker id: `chatgpt/<slug>` (contract §9.9).
    var id: String { "chatgpt/\(slug)" }

    /// `{models:[{slug, display_name, visibility}]}`, keeping only
    /// `visibility == "list"` in server order. An OpenAI-style `{data:[{id}]}`
    /// body is read too.
    static func parseList(_ data: Data) -> [ChatGPTModel] {
        guard let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else { return [] }
        if let models = object["models"] as? [[String: Any]] {
            return models.compactMap { row in
                guard let slug = row["slug"] as? String, !slug.isEmpty else { return nil }
                if let visibility = row["visibility"] as? String, visibility != "list" { return nil }
                return ChatGPTModel(slug: slug, name: (row["display_name"] as? String) ?? slug)
            }
        }
        if let rows = object["data"] as? [[String: Any]] {
            return rows.compactMap { row in
                (row["id"] as? String).map { ChatGPTModel(slug: $0, name: $0) }
            }
        }
        return []
    }
}

/// One frame of the Responses stream (`POST /v1/responses`, `stream:true`).
enum ResponsesStreamEvent: Equatable, Sendable {
    case delta(String)
    case completed
    /// `response.failed`: `response.error.code` and message.
    case failed(code: String?, message: String?)
    /// `response.incomplete`: `response.incomplete_details.reason`.
    case incomplete(reason: String?)
    /// A top-level `error` frame.
    case error(code: String?, message: String?)
    case other

    /// Reads one `data:` payload. Frames the chat doesn't use are `.other`.
    static func parse(_ payload: String) -> ResponsesStreamEvent? {
        guard let data = payload.data(using: .utf8),
              let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else { return nil }
        let type = object["type"] as? String ?? ""
        let response = object["response"] as? [String: Any]
        switch type {
        case "response.output_text.delta":
            return (object["delta"] as? String).map { .delta($0) } ?? .other
        case "response.completed":
            return .completed
        case "response.failed":
            let error = response?["error"] as? [String: Any]
            return .failed(code: error?["code"] as? String, message: error?["message"] as? String)
        case "response.incomplete":
            let details = response?["incomplete_details"] as? [String: Any]
            return .incomplete(reason: details?["reason"] as? String)
        case "error":
            let error = object["error"] as? [String: Any] ?? object
            return .error(code: error["code"] as? String, message: error["message"] as? String)
        default:
            return .other
        }
    }

    /// The two codes that mean the plan's usage ran out (Models and inference).
    static let usageLimitCodes: Set<String> = [
        "subscription_sharing_usage_limit_exceeded",
        "subscription_sharing_usage_unavailable",
    ]
}
