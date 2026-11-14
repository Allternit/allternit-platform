import CryptoKit
import Foundation

/// The saved ChatGPT sign-in on this iPhone (Keychain only).
struct ChatGPTSession: Codable, Equatable, Sendable {
    var clientId: String
    var accessToken: String
    var refreshToken: String?
    var idToken: String?
    var expiresAt: Date
    var email: String?
    var plan: String?
}

/// One sign-in attempt in flight: what the callback is checked against.
struct ChatGPTPendingSignIn: Sendable, Equatable {
    let url: URL
    let state: String
    let nonce: String
    let verifier: String
    /// The issued client id the authorize request used; nil = registering.
    let clientId: String?
}

enum ChatGPTAuthError: Error, Equatable, Sendable {
    case callback(ChatGPTOAuth.CallbackError)
    /// The token endpoint said no (`invalid_grant` and friends).
    case rejected(String)
    /// The grant lacks `chatgpt.tokens.use.direct`.
    case noPlanScope
    case badIdToken
    case signedOut
    case network

    /// Plain words for the sheet.
    var plainText: String {
        switch self {
        case .callback(.denied): return "You cancelled the ChatGPT sign-in."
        case .callback(.stateMismatch): return "The sign-in came back for a different attempt. Try again."
        case .callback(.registrationIncomplete): return "ChatGPT didn't finish setting up Allternit on this iPhone. Try again."
        case .callback(.clientMismatch): return "ChatGPT answered for a different app. Try again."
        case .callback(.noCode): return "ChatGPT didn't send a sign-in code. Try again."
        case .callback(.vendor(let text)): return "ChatGPT said: \(text)"
        case .rejected: return "ChatGPT didn't accept the sign-in. Try again."
        case .noPlanScope: return "This ChatGPT account didn't allow plan use. Sign in with a Plus or Pro account."
        case .badIdToken: return "The sign-in couldn't be checked. Try again."
        case .signedOut: return "Sign in to ChatGPT first."
        case .network: return "No connection. Try again."
        }
    }
}

/// Sign in with ChatGPT on this iPhone (contract §9.9): the host key, the
/// issued client id, the tokens, refresh and sign-out. Refreshes run one at a
/// time because OpenAI rotates the refresh token on every use.
actor ChatGPTAuthService {
    static let shared = ChatGPTAuthService()

    private enum Account {
        static let hostKey = "chatgpt.host-key.p256"
        static let clientId = "chatgpt.issued-client-id"
        static let session = "chatgpt.session"
    }

    private var refreshTask: Task<ChatGPTSession, Error>?
    private let urlSession: URLSession = .shared

    // MARK: - Host id

    /// This phone's `ext_agent_host_id`, from a P-256 key made once and kept
    /// in the Keychain.
    func hostId() -> String {
        let key: P256.Signing.PrivateKey
        if let saved = KeychainHelper.load(Account.hostKey).flatMap({ Data(base64Encoded: $0) }),
           let restored = try? P256.Signing.PrivateKey(rawRepresentation: saved) {
            key = restored
        } else {
            key = P256.Signing.PrivateKey()
            KeychainHelper.save(key.rawRepresentation.base64EncodedString(), for: Account.hostKey)
        }
        return ChatGPTOAuth.hostId(publicKeyX963: key.publicKey.x963Representation) ?? "urn:uuid:\(UUID().uuidString.lowercased())"
    }

    // MARK: - Session

    func session() -> ChatGPTSession? {
        guard let text = KeychainHelper.load(Account.session) else { return nil }
        return try? JSONDecoder().decode(ChatGPTSession.self, from: Data(text.utf8))
    }

    private func save(_ session: ChatGPTSession) {
        if let data = try? JSONEncoder().encode(session) {
            KeychainHelper.save(String(decoding: data, as: UTF8.self), for: Account.session)
        }
        KeychainHelper.save(session.clientId, for: Account.clientId)
    }

    // MARK: - Sign in

    func beginSignIn() -> ChatGPTPendingSignIn {
        let pkce = ChatGPTOAuth.PKCE.make()
        let state = ChatGPTOAuth.randomToken()
        let nonce = ChatGPTOAuth.randomToken()
        let clientId = KeychainHelper.load(Account.clientId)
        let url = ChatGPTOAuth.authorizeURL(issuedClientId: clientId, hostId: hostId(), state: state, nonce: nonce, challenge: pkce.challenge)
        return ChatGPTPendingSignIn(url: url, state: state, nonce: nonce, verifier: pkce.verifier, clientId: clientId)
    }

    /// The web view caught the loopback redirect: check it, exchange the code
    /// on the phone, keep the tokens in the Keychain.
    func complete(callback url: URL, pending: ChatGPTPendingSignIn) async throws -> ChatGPTSession {
        let callback: ChatGPTOAuth.Callback
        switch ChatGPTOAuth.parseCallback(url, expectedState: pending.state, pendingClientId: pending.clientId) {
        case .success(let value): callback = value
        case .failure(let error): throw ChatGPTAuthError.callback(error)
        }
        // A registered client is kept even if the exchange fails.
        if pending.clientId == nil { KeychainHelper.save(callback.clientId, for: Account.clientId) }
        let response = try await tokenCall(ChatGPTOAuth.codeExchangeForm(code: callback.code, verifier: pending.verifier, clientId: callback.clientId))
        guard ChatGPTOAuth.grantedScopes(response).contains(ChatGPTOAuth.directScope) else { throw ChatGPTAuthError.noPlanScope }
        if let idToken = response.idToken,
           !ChatGPTOAuth.idTokenIsValid(idToken, clientId: callback.clientId, nonce: pending.nonce) {
            throw ChatGPTAuthError.badIdToken
        }
        let session = Self.session(from: response, clientId: callback.clientId, previous: nil)
        save(session)
        return session
    }

    private static func session(from response: ChatGPTOAuth.TokenResponse, clientId: String, previous: ChatGPTSession?) -> ChatGPTSession {
        let idToken = response.idToken ?? previous?.idToken
        let claims = idToken.flatMap(ChatGPTOAuth.jwtClaims)
        let auth = claims?["https://api.openai.com/auth"] as? [String: Any]
        return ChatGPTSession(
            clientId: clientId,
            accessToken: response.accessToken,
            refreshToken: response.refreshToken ?? previous?.refreshToken,
            idToken: idToken,
            expiresAt: Date().addingTimeInterval(response.expiresIn ?? 3600),
            email: (claims?["email"] as? String) ?? previous?.email,
            plan: (auth?["chatgpt_plan_type"] as? String) ?? previous?.plan
        )
    }

    private func tokenCall(_ form: [(String, String)]) async throws -> ChatGPTOAuth.TokenResponse {
        var request = URLRequest(url: ChatGPTOAuth.tokenEndpoint)
        request.httpMethod = "POST"
        request.setValue("application/x-www-form-urlencoded", forHTTPHeaderField: "Content-Type")
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.httpBody = ChatGPTOAuth.formBody(form)
        let data: Data
        let response: URLResponse
        do { (data, response) = try await urlSession.data(for: request) } catch { throw ChatGPTAuthError.network }
        guard let http = response as? HTTPURLResponse else { throw ChatGPTAuthError.network }
        guard (200...299).contains(http.statusCode) else {
            let body = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
            throw ChatGPTAuthError.rejected((body?["error"] as? String) ?? "status \(http.statusCode)")
        }
        do { return try JSONDecoder().decode(ChatGPTOAuth.TokenResponse.self, from: data) } catch { throw ChatGPTAuthError.rejected("unreadable") }
    }

    // MARK: - Tokens for calls

    /// A token good for at least two more minutes, refreshing when needed.
    func validAccessToken() async throws -> String {
        guard let current = session() else { throw ChatGPTAuthError.signedOut }
        if current.expiresAt.timeIntervalSinceNow > 120 { return current.accessToken }
        return try await refresh().accessToken
    }

    /// Refresh now (after a 401). Concurrent callers share one refresh.
    @discardableResult
    func refresh() async throws -> ChatGPTSession {
        if let refreshTask { return try await refreshTask.value }
        guard let current = session() else { throw ChatGPTAuthError.signedOut }
        guard let refreshToken = current.refreshToken else { throw ChatGPTAuthError.signedOut }
        let task = Task { () throws -> ChatGPTSession in
            let response = try await self.tokenCall(ChatGPTOAuth.refreshForm(refreshToken: refreshToken, clientId: current.clientId))
            let next = Self.session(from: response, clientId: current.clientId, previous: current)
            self.save(next)
            return next
        }
        refreshTask = task
        defer { refreshTask = nil }
        do {
            return try await task.value
        } catch ChatGPTAuthError.rejected(let code) where code == "invalid_grant" {
            // The refresh token is dead: sign out locally.
            KeychainHelper.delete(Account.session)
            throw ChatGPTAuthError.signedOut
        }
    }

    // MARK: - Sign out

    /// Revokes the refresh token at OpenAI, then deletes the local tokens
    /// either way. Returns false when OpenAI didn't confirm the revoke. The
    /// issued client id and the host key stay (revoking doesn't delete the
    /// registered client).
    func signOut() async -> Bool {
        defer { KeychainHelper.delete(Account.session) }
        guard let current = session(), let refreshToken = current.refreshToken else { return true }
        guard let endpoint = await revocationEndpoint() else { return false }
        var request = URLRequest(url: endpoint)
        request.httpMethod = "POST"
        request.setValue("application/x-www-form-urlencoded", forHTTPHeaderField: "Content-Type")
        request.httpBody = ChatGPTOAuth.formBody(ChatGPTOAuth.revokeForm(refreshToken: refreshToken, clientId: current.clientId))
        for attempt in 0..<3 {
            if attempt > 0 { try? await Task.sleep(for: .seconds(Double(attempt))) }
            guard let (_, response) = try? await urlSession.data(for: request),
                  let http = response as? HTTPURLResponse else { continue }
            if (200...299).contains(http.statusCode) { return true }
            if http.statusCode < 500 { return false }
        }
        return false
    }

    private func revocationEndpoint() async -> URL? {
        guard let (data, _) = try? await urlSession.data(from: ChatGPTOAuth.discoveryEndpoint),
              let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let text = object["revocation_endpoint"] as? String else { return nil }
        return URL(string: text)
    }

    // MARK: - Calls to api.openai.com

    /// Sends `build(token)`; on a 401 refreshes once and sends again.
    func authorizedBytes(_ build: @Sendable (String) -> URLRequest) async throws -> (URLSession.AsyncBytes, HTTPURLResponse) {
        var token = try await validAccessToken()
        for attempt in 0..<2 {
            let (bytes, response) = try await urlSession.bytes(for: build(token))
            guard let http = response as? HTTPURLResponse else { throw ChatGPTAuthError.network }
            if http.statusCode == 401, attempt == 0 {
                token = try await refresh().accessToken
                continue
            }
            return (bytes, http)
        }
        throw ChatGPTAuthError.signedOut
    }

    /// The plan's models (`GET /v1/models`).
    func listModels() async throws -> [ChatGPTModel] {
        let (bytes, http) = try await authorizedBytes { token in
            var request = URLRequest(url: URL(string: "https://api.openai.com/v1/models")!)
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
            request.timeoutInterval = 20
            return request
        }
        var data = Data()
        for try await byte in bytes { data.append(byte) }
        guard (200...299).contains(http.statusCode) else { throw APIError.httpError(statusCode: http.statusCode, message: nil) }
        return ChatGPTModel.parseList(data)
    }
}
