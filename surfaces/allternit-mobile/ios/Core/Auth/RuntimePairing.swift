import Foundation
import CryptoKit
import Combine
import UIKit

/// iOS runtime pairing with the Allternit Cloud API.
///
/// After the user signs in with Clerk, the mobile app pairs itself as an
/// `ios` runtime: it creates a pairing, approves it with the Clerk session
/// token, then exchanges the pairing for a long-lived cloud-issued device
/// credential (`allternit_runtime_…`). The device token is persisted in the
/// Keychain and used by `APIClient` for gateway calls, matching how desktop
/// and VPS runtimes authenticate.
///
/// The credential belongs to one person (plan contract C7): it is stored
/// under a Keychain key per Clerk user id (`RuntimeCredentialKeys`).
/// `useAccount(userId:)` loads the signed-in person's item; the old unkeyed
/// item is moved to that key once, so a working pairing is never dropped.
@MainActor
final class RuntimePairing: ObservableObject {
    static let shared = RuntimePairing()

    #if DEBUG
    private func debugLog(_ message: String) {
        NSLog("%@", "[RuntimePairing] " + message)
    }
    #endif

    /// Live pairing state surfaced by `LoginGateView` / the account row.
    @Published private(set) var state: PairingState = .idle

    /// The current cloud-issued device session, loaded from Keychain at init.
    @Published private(set) var session: RuntimeDeviceSession?

    private let cloudBaseURL: URL
    private let jsonEncoder = JSONEncoder()
    private let jsonDecoder = JSONDecoder()
    private var pairingTask: Task<RuntimeDeviceSession, Error>?
    /// Clerk user id whose credential is loaded; nil before the first
    /// signed-in launch with per-user keys (the unkeyed item is used then).
    private(set) var activeUserId: String?
    private let store: SecretStore = KeychainSecretStore()

    enum PairingState: Equatable {
        case idle
        case creating
        case approving
        case exchanging
        case success
        case failed(String)
    }

    enum PairingError: LocalizedError {
        case notSignedIn
        case network(Error)
        case server(Int, String?)
        case invalidResponse
        case crypto(Error)
        case expired
        case denied

        var errorDescription: String? {
            switch self {
            case .notSignedIn:
                return "Sign in with your Allternit account first."
            case .network(let error):
                return "Network error: \(error.localizedDescription)"
            case .server(let code, let message):
                return message ?? "Server error \(code)"
            case .invalidResponse:
                return "The pairing response was invalid."
            case .crypto(let error):
                return "Key error: \(error.localizedDescription)"
            case .expired:
                return "The pairing request expired. Please try again."
            case .denied:
                return "The pairing request was denied."
            }
        }
    }

    private init(cloudBaseURL: URL = AppConfig.cloudAPIBaseURL) {
        self.cloudBaseURL = cloudBaseURL
        self.jsonDecoder.dateDecodingStrategy = .iso8601
        self.jsonEncoder.dateEncodingStrategy = .iso8601
        // The person signed in last time, so API calls made before Clerk
        // loads its session still find their credential.
        self.activeUserId = UserDefaults.standard.string(forKey: RuntimeCredentialKeys.activeUserDefaultsKey)
        self.session = loadSessionFromKeychain()
    }

    // MARK: - Per-person credential

    /// Loads the credential of the signed-in Clerk user. Call whenever the
    /// session is read (AuthManager.refreshAuthState). Returns true when the
    /// person changed from an earlier signed-in person: the caller then runs
    /// the per-person cleanup. Switching between one person's organizations
    /// never calls this with a new id, so the credential stays.
    @discardableResult
    func useAccount(userId: String?) -> Bool {
        guard let userId, !userId.isEmpty else { return false }
        let previous = activeUserId
        // Same person: their item was loaded (and migrated) already.
        if previous == userId { return false }
        let personChanged = previous != nil && previous != userId
        if personChanged {
            // The earlier person's pairing must not finish into this person's key.
            pairingTask?.cancel()
            pairingTask = nil
            state = .idle
        }
        activeUserId = userId
        UserDefaults.standard.set(userId, forKey: RuntimeCredentialKeys.activeUserDefaultsKey)
        session = loadSessionFromKeychain()
        return personChanged
    }

    // MARK: - Public API

    /// Returns the stored device token when present and not expired.
    func deviceToken() -> String? {
        guard let session, session.expiresAt > Date() else { return nil }
        return session.deviceToken
    }

    /// Returns true when a non-expired device session exists.
    var isPaired: Bool {
        deviceToken() != nil
    }

    /// Creates, approves, and exchanges a pairing in one shot. Call after
    /// Clerk sign-in succeeds. Idempotent: if already paired, returns
    /// immediately. Concurrent callers share the same in-flight pairing task.
    @discardableResult
    func ensurePaired() async throws -> RuntimeDeviceSession {
        if let session = self.session, session.expiresAt > Date() {
            return session
        }

        if let existing = pairingTask, !existing.isCancelled {
            return try await existing.value
        }

        let task = Task { @MainActor in try await performPairing() }
        pairingTask = task
        do {
            let session = try await task.value
            pairingTask = nil
            return session
        } catch {
            pairingTask = nil
            throw error
        }
    }

    /// Revokes the paired runtime device and clears local storage.
    func revoke() async {
        guard let session else {
            forgetActiveUser()
            return
        }
        let runtimeId = session.runtimeId
        let token = session.deviceToken
        do {
            var request = URLRequest(url: cloudBaseURL.appendingPathComponent("api/v1/runtime-devices/\(runtimeId)/revoke-self"))
            request.httpMethod = "POST"
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
            let (_, response) = try await URLSession.shared.data(for: request)
            if let http = response as? HTTPURLResponse, http.statusCode >= 400, http.statusCode != 404 {
                #if DEBUG
                debugLog("revoke warning: \(http.statusCode)")
                #endif
            }
        } catch {
            #if DEBUG
            debugLog("revoke error: \(error.localizedDescription)")
            #endif
        }
        clearSession()
    }

    /// Rotates the device credential if it is within the refresh window.
    func rotateIfNeeded() async {
        guard let session else { return }
        let refreshWindow: TimeInterval = 7 * 24 * 60 * 60 // 7 days before expiry
        guard session.expiresAt.timeIntervalSinceNow < refreshWindow else { return }

        do {
            let rotated = try await rotateCredential(runtimeId: session.runtimeId, token: session.deviceToken)
            self.session = rotated
            saveSessionToKeychain(rotated)
        } catch {
            #if DEBUG
            debugLog("rotation deferred: \(error.localizedDescription)")
            #endif
        }
    }

    // MARK: - Pairing flow

    private func performPairing() async throws -> RuntimeDeviceSession {
        state = .creating
        do {
            guard let clerkToken = try await AuthManager.shared.getToken(), !clerkToken.isEmpty else {
                throw PairingError.notSignedIn
            }
            // The credential is saved under this person's key.
            if activeUserId == nil { useAccount(userId: AuthManager.shared.currentUserId) }
            let pairedFor = activeUserId
            #if DEBUG
            debugLog("performPairing starting with active Clerk token")
            #endif

            // 1. Generate an Ed25519 keypair for this runtime.
            let privateKey = try Curve25519.Signing.PrivateKey()
            let publicKeyRaw = privateKey.publicKey.rawRepresentation
            let publicKeyB64 = publicKeyRaw.base64URLEncodedString()

            // 2. Create the pairing.
            #if DEBUG
            debugLog("creating runtime pairing on cloud API")
            #endif
            let pairing = try await createPairing(publicKey: publicKeyB64)
            #if DEBUG
            debugLog("pairing created: \(pairing.pairingId), userCode: \(pairing.userCode)")
            #endif

            // 3. Approve it with the Clerk-authenticated user.
            state = .approving
            #if DEBUG
            debugLog("approving pairing with Clerk token")
            #endif
            try await approvePairing(code: pairing.userCode, clerkToken: clerkToken)
            #if DEBUG
            debugLog("pairing approved")
            #endif

            // 4. Exchange for the device credential.
            state = .exchanging
            let message = "allternit-runtime-pairing:\(pairing.pairingId):\(pairing.challenge)"
            let signature = try privateKey.signature(for: Data(message.utf8))
            let signatureB64 = signature.base64URLEncodedString()
            #if DEBUG
            debugLog("exchanging pairing for device credential")
            #endif
            let session = try await exchangePairing(
                pairingId: pairing.pairingId,
                deviceCode: pairing.deviceCode,
                signature: signatureB64
            )
            #if DEBUG
            debugLog("device credential received; runtimeId: \(session.runtimeId)")
            #endif

            // The person changed while this pairing ran: their credential is
            // not this person's. Drop it (the server row expires unused).
            if let pairedFor, pairedFor != activeUserId {
                throw CancellationError()
            }
            self.session = session
            saveSessionToKeychain(session)
            state = .success
            return session
        } catch {
            #if DEBUG
            debugLog("performPairing error: \(error.localizedDescription)")
            #endif
            state = .failed(error.localizedDescription)
            throw error
        }
    }

    private func createPairing(publicKey: String) async throws -> PairingStartResponse {
        let device = UIDevice.current
        let name = "\(device.name) iOS"
        let platform = "ios-\(device.systemVersion)"
        let body = CreatePairingRequest(
            name: name,
            runtimeType: "ios",
            hostname: device.name,
            platform: platform,
            version: Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String,
            publicKey: publicKey,
            capabilities: ["runtime:connect", "runtime:execute", "runtime:files", "providers:connect", "providers:use"]
        )

        var request = URLRequest(url: cloudBaseURL.appendingPathComponent("api/v1/runtime-pairings"))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try jsonEncoder.encode(body)

        let (data, response) = try await URLSession.shared.data(for: request)
        try validate(response, data: data)
        return try jsonDecoder.decode(PairingStartResponse.self, from: data)
    }

    private func approvePairing(code: String, clerkToken: String) async throws {
        let normalized = Self.normalizeUserCode(code)
        let url = cloudBaseURL.appendingPathComponent("api/v1/runtime-pairings/code/\(normalized)/approve")
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue("Bearer \(clerkToken)", forHTTPHeaderField: "Authorization")
        request.httpBody = try jsonEncoder.encode([String: String]())

        let (data, response) = try await URLSession.shared.data(for: request)
        try validate(response, data: data)
    }

    private func exchangePairing(pairingId: String, deviceCode: String, signature: String) async throws -> RuntimeDeviceSession {
        var request = URLRequest(url: cloudBaseURL.appendingPathComponent("api/v1/runtime-pairings/exchange"))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try jsonEncoder.encode(ExchangePairingRequest(
            pairingId: pairingId,
            deviceCode: deviceCode,
            signature: signature
        ))

        let (data, response) = try await URLSession.shared.data(for: request)
        try validate(response, data: data)
        let exchange = try jsonDecoder.decode(ExchangePairingResponse.self, from: data)
        return RuntimeDeviceSession(
            runtimeId: exchange.runtimeId,
            userId: exchange.userId,
            userEmail: exchange.userEmail,
            organizationId: exchange.organizationId,
            deviceToken: exchange.deviceToken,
            tokenType: exchange.tokenType,
            expiresAt: exchange.expiresAt,
            capabilities: exchange.capabilities
        )
    }

    private func rotateCredential(runtimeId: String, token: String) async throws -> RuntimeDeviceSession {
        let url = cloudBaseURL.appendingPathComponent("api/v1/runtime-devices/\(runtimeId)/rotate")
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")

        let (data, response) = try await URLSession.shared.data(for: request)
        try validate(response, data: data)
        let rotated = try jsonDecoder.decode(RotateResponse.self, from: data)
        guard let session = self.session else {
            throw PairingError.invalidResponse
        }
        return RuntimeDeviceSession(
            runtimeId: session.runtimeId,
            userId: session.userId,
            userEmail: session.userEmail,
            organizationId: session.organizationId,
            deviceToken: rotated.deviceToken,
            tokenType: rotated.tokenType,
            expiresAt: rotated.expiresAt,
            capabilities: session.capabilities
        )
    }

    // MARK: - Helpers

    private func validate(_ response: URLResponse, data: Data) throws {
        guard let http = response as? HTTPURLResponse else {
            throw PairingError.invalidResponse
        }
        guard (200...299).contains(http.statusCode) else {
            var message: String?
            if let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] {
                message = (object["message"] as? String) ?? (object["error"] as? String)
            }
            let bodyPreview = String(data: data, encoding: .utf8) ?? "<non-utf8>"
            #if DEBUG
            debugLog("server error \(http.statusCode): \(message ?? bodyPreview)")
            #endif
            if http.statusCode == 410 {
                throw PairingError.expired
            }
            if http.statusCode == 403 {
                throw PairingError.denied
            }
            throw PairingError.server(http.statusCode, message)
        }
    }

    static func normalizeUserCode(_ value: String) -> String {
        let compact = value
            .filter { $0.isASCII && ($0.isLetter || $0.isNumber) }
            .uppercased()
            .prefix(8)
        if compact.count == 8 {
            let start = compact.startIndex
            let mid = compact.index(start, offsetBy: 4)
            return "\(compact[start..<mid])-\(compact[mid...])"
        }
        return String(compact)
    }

    // MARK: - Persistence

    /// The Keychain key for the loaded person; the old unkeyed key until a
    /// signed-in person is known.
    private var sessionKey: String {
        activeUserId.map(RuntimeCredentialKeys.key(forUserId:)) ?? RuntimeCredentialKeys.legacy
    }

    private func loadSessionFromKeychain() -> RuntimeDeviceSession? {
        let stored: String?
        if let activeUserId {
            stored = RuntimeCredentialKeys.loadMigrating(userId: activeUserId, store: store)
        } else {
            stored = store.load(RuntimeCredentialKeys.legacy)
        }
        guard let json = stored,
              let data = json.data(using: .utf8),
              let session = try? jsonDecoder.decode(RuntimeDeviceSession.self, from: data) else {
            return nil
        }
        return session
    }

    private func saveSessionToKeychain(_ session: RuntimeDeviceSession) {
        guard let data = try? jsonEncoder.encode(session),
              let json = String(data: data, encoding: .utf8) else {
            return
        }
        store.save(json, for: sessionKey)
    }

    /// Revoke and sign-out: this person's item and any unkeyed copy go.
    private func clearSession() {
        session = nil
        store.delete(sessionKey)
        store.delete(RuntimeCredentialKeys.legacy)
        state = .idle
        forgetActiveUser()
    }

    private func forgetActiveUser() {
        activeUserId = nil
        UserDefaults.standard.removeObject(forKey: RuntimeCredentialKeys.activeUserDefaultsKey)
    }
}

// MARK: - Data models

struct RuntimeDeviceSession: Codable {
    let runtimeId: String
    let userId: String
    let userEmail: String
    let organizationId: String?
    let deviceToken: String
    let tokenType: String
    let expiresAt: Date
    let capabilities: [String]
}

private struct CreatePairingRequest: Encodable {
    let name: String
    let runtimeType: String
    let hostname: String?
    let platform: String?
    let version: String?
    let publicKey: String
    let capabilities: [String]
}

private struct PairingStartResponse: Decodable {
    let pairingId: String
    let deviceCode: String
    let userCode: String
    let challenge: String
    let verificationUrl: String
    let expiresAt: Date
    let pollIntervalSeconds: UInt64
}

private struct ExchangePairingRequest: Encodable {
    let pairingId: String
    let deviceCode: String
    let signature: String
}

private struct ExchangePairingResponse: Decodable {
    let runtimeId: String
    let userId: String
    let userEmail: String
    let organizationId: String?
    let deviceToken: String
    let tokenType: String
    let expiresAt: Date
    let capabilities: [String]
}

private struct RotateResponse: Decodable {
    let runtimeId: String
    let deviceToken: String
    let tokenType: String
    let expiresAt: Date
}

// MARK: - Base64URL encoding

extension Data {
    /// base64url encoding without padding, matching the cloud API's expectation.
    func base64URLEncodedString() -> String {
        let base64 = self.base64EncodedString()
        return base64
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }
}
