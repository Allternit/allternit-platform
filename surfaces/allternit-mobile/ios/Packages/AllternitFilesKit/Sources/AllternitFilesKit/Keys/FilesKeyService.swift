import CryptoKit
import Foundation

/// Where this device stands with the user's Files key.
public enum FilesKeyState: Sendable, Equatable {
    /// This device holds an envelope for the account's current key.
    case ready(keyId: String)
    /// This device asked; another device must approve. Show `code`.
    case waiting(code: String)
    /// The account has a Files key and this device has not asked yet.
    case needsRequest
    /// The account has no Files key yet: this device may make it.
    case noKey
}

/// A key-share request from another device, with the code computed here
/// from its public key (never the code the request carries: a swapped key
/// would show a different code than the new device does).
public struct FilesKeyApproval: Sendable, Hashable, Identifiable {
    public var request: KeyShareRequest
    /// Computed from `request.devicePub` on this device.
    public var code: String
    /// The code the request carries matches `code`. A mismatch is refused.
    public var codeMatches: Bool
    public var id: String { request.deviceId }
}

/// cloud-api's Files key routes (SPEC section 2, "cloud-api"; section 13)
/// and this device's key ring:
///
/// - `GET  /api/v1/files-keys`: `FilesKeyInfo` (404 while the account has none)
/// - `POST /api/v1/files-keys/requests`: `KeyShareRequest`, 202
/// - `POST /api/v1/files-keys/grants`: `KeyShareGrant`, 204
/// - `GET  /api/v1/files-keys/envelopes/:device_id`: `KeyEnvelope` (404 until approved)
/// - `GET  /api/v1/files-keys/requests`: `{requests: [KeyShareRequest]}`, the
///   open requests an approving device lists. **Not in SPEC section 2 yet**
///   (the four routes above are); the approve screen needs it, so the
///   cloud-api route and the SPEC row must land with it.
///
/// The app (sign-in, Settings > Files > Devices) uses all of it; the File
/// Provider extension only calls `rootKey(store:keyId:)`, which reads the
/// shared keychain and never touches the network.
public final class FilesKeyService: @unchecked Sendable {
    public let store: FilesSharedStore
    private let http: FilesHTTP

    public init(store: FilesSharedStore, http: FilesHTTP = URLSessionFilesHTTP()) {
        self.store = store
        self.http = http
    }

    // MARK: Key ring (no network)

    /// The FRK for `keyId` (nil: the newest this device holds), opened in
    /// memory from its envelope. Throws `keyUnavailable` when this device has
    /// no envelope for it.
    public static func rootKey(store: FilesSharedStore, keyId: String? = nil) throws -> FilesRootKey {
        guard let secret = store.filesDeviceSecret(),
              let deviceKey = try? Curve25519.KeyAgreement.PrivateKey(rawRepresentation: secret) else {
            throw FilesError.keyUnavailable("This device has no Files key yet. Approve it from another of your devices in Settings > Files > Devices.")
        }
        let envelopes = store.filesKeyEnvelopes()
        guard let envelope = keyId.map({ id in envelopes.first { $0.keyId == id } }) ?? envelopes.first else {
            throw FilesError.keyUnavailable(keyId.map { "This device doesn't have Files key \($0) yet." }
                                                ?? "This device has no Files key yet.")
        }
        return try FilesKeyCrypto.unwrap(envelope, deviceSecret: deviceKey)
    }

    /// This device's X25519 key, made once.
    func deviceKey() throws -> Curve25519.KeyAgreement.PrivateKey {
        if let secret = store.filesDeviceSecret(), let key = try? Curve25519.KeyAgreement.PrivateKey(rawRepresentation: secret) {
            return key
        }
        let key = Curve25519.KeyAgreement.PrivateKey()
        guard store.setFilesDeviceSecret(key.rawRepresentation) else {
            throw FilesError.keyUnavailable("This device's secure storage isn't available. Unlock the phone and try again.")
        }
        return key
    }

    private var deviceId: String {
        get throws {
            guard let id = store.deviceId, !id.isEmpty else { throw FilesError.signedOut }
            return id
        }
    }

    /// Keeps `envelope` (newest first, one per key id).
    private func keep(_ envelope: KeyEnvelope) {
        var all = store.filesKeyEnvelopes().filter { $0.keyId != envelope.keyId }
        all.insert(envelope, at: 0)
        store.setFilesKeyEnvelopes(all)
    }

    // MARK: State

    /// Asks cloud-api and settles this device's state: picks up an envelope
    /// another device granted (also after a key rotation) and clears the
    /// pending request once it opens.
    public func state() async throws -> FilesKeyState {
        guard let info = try await keyInfo() else { return .noKey }
        if store.filesKeyEnvelopes().contains(where: { $0.keyId == info.keyId }),
           (try? Self.rootKey(store: store, keyId: info.keyId)) != nil {
            store.pendingKeyRequest = nil
            return .ready(keyId: info.keyId)
        }
        if let envelope = try await envelope(), envelope.keyId == info.keyId,
           (try? FilesKeyCrypto.unwrap(envelope, deviceSecret: try deviceKey())) != nil {
            keep(envelope)
            store.pendingKeyRequest = nil
            return .ready(keyId: info.keyId)
        }
        if let pending = store.pendingKeyRequest { return .waiting(code: pending.confirmCode) }
        return .needsRequest
    }

    /// `GET /api/v1/files-keys`; nil while the account has no key.
    public func keyInfo() async throws -> FilesKeyInfo? {
        let (data, status) = try await call("GET", "api/v1/files-keys")
        if status == 404 { return nil }
        return try decode(data, status)
    }

    /// `GET /api/v1/files-keys/envelopes/:device_id` for this device; nil until approved.
    func envelope() async throws -> KeyEnvelope? {
        let (data, status) = try await call("GET", "api/v1/files-keys/envelopes/\(FilesClient.escape(try deviceId))")
        if status == 404 { return nil }
        return try decode(data, status)
    }

    // MARK: First device

    /// Makes the account's Files key on this device (only while the account
    /// has none) and grants it to this device itself.
    @discardableResult
    public func createFirstKey() async throws -> String {
        if let info = try await keyInfo() {
            throw FilesError.keyUnavailable("Your account already has a Files key (\(info.keyId)). Approve this device from another one.")
        }
        let me = try deviceId
        let frk = FilesRootKey.generate()
        let envelope = try FilesKeyCrypto.wrap(frk, toDevicePub: try deviceKey().publicKey.rawRepresentation,
                                               deviceId: me, createdAt: Self.now())
        try await grant(KeyShareGrant(envelope: envelope, approvedByDeviceId: me))
        keep(envelope)
        store.pendingKeyRequest = nil
        return frk.keyId
    }

    // MARK: New device

    /// Sends this device's `KeyShareRequest` and returns the 6-digit code to
    /// show until another device approves. Sending again is safe.
    @discardableResult
    public func requestAccess(deviceName: String) async throws -> String {
        let pub = try deviceKey().publicKey.rawRepresentation
        let request = KeyShareRequest(deviceId: try deviceId, devicePub: FilesKeyCrypto.base64url(pub),
                                      deviceName: deviceName, confirmCode: FilesKeyCrypto.confirmCode(devicePub: pub))
        let (data, status) = try await call("POST", "api/v1/files-keys/requests", body: try JSONEncoder().encode(request))
        guard (200..<300).contains(status) else { throw ErrorDecoding.error(status: status, data: data) }
        store.pendingKeyRequest = request
        return request.confirmCode
    }

    // MARK: Approving device

    /// Open requests from the user's other devices (route not in SPEC yet,
    /// see the type comment). An empty list when the route is missing (404).
    public func pendingRequests() async throws -> [FilesKeyApproval] {
        let (data, status) = try await call("GET", "api/v1/files-keys/requests")
        if status == 404 { return [] }
        guard (200..<300).contains(status) else { throw ErrorDecoding.error(status: status, data: data) }
        struct Listing: Decodable { var requests: [KeyShareRequest] }
        let requests = (try? JSONDecoder().decode(Listing.self, from: data).requests)
            ?? (try? JSONDecoder().decode([KeyShareRequest].self, from: data)) ?? []
        let me = store.deviceId
        return requests.filter { $0.deviceId != me }.compactMap { request in
            guard let pub = FilesKeyCrypto.fromBase64url(request.devicePub), pub.count == 32 else { return nil }
            let code = FilesKeyCrypto.confirmCode(devicePub: pub)
            return FilesKeyApproval(request: request, code: code, codeMatches: code == request.confirmCode)
        }
    }

    /// Wraps the current key to the requesting device and posts the grant.
    /// Call only after the user saw the same code on both devices.
    public func approve(_ approval: FilesKeyApproval) async throws {
        guard approval.codeMatches, let pub = FilesKeyCrypto.fromBase64url(approval.request.devicePub), pub.count == 32 else {
            throw FilesError.keyUnavailable("The code from \(approval.request.deviceName) doesn't match its key. Ask it to request again.")
        }
        let current = try await keyInfo()?.keyId
        let frk = try Self.rootKey(store: store, keyId: current)
        let envelope = try FilesKeyCrypto.wrap(frk, toDevicePub: pub, deviceId: approval.request.deviceId, createdAt: Self.now())
        try await grant(KeyShareGrant(envelope: envelope, approvedByDeviceId: try deviceId))
    }

    private func grant(_ grant: KeyShareGrant) async throws {
        let (data, status) = try await call("POST", "api/v1/files-keys/grants", body: try JSONEncoder().encode(grant))
        guard (200..<300).contains(status) else { throw ErrorDecoding.error(status: status, data: data) }
    }

    // MARK: HTTP

    private func call(_ method: String, _ path: String, body: Data? = nil) async throws -> (Data, Int) {
        guard let token = store.token(), !store.tokenRejected else { throw FilesError.signedOut }
        var base = store.cloudBaseURL.absoluteString
        while base.hasSuffix("/") { base.removeLast() }
        guard let url = URL(string: base + "/" + path) else { throw FilesError.invalidResponse("bad URL \(path)") }
        var request = URLRequest(url: url)
        request.httpMethod = method
        request.timeoutInterval = 30
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        if let id = store.deviceId { request.setValue(id, forHTTPHeaderField: "X-Allternit-Device-Id") }
        if let body {
            request.httpBody = body
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        let (data, response) = try await http.data(for: request)
        if response.statusCode == 401 { store.tokenRejected = true }
        return (data, response.statusCode)
    }

    private func decode<T: Decodable>(_ data: Data, _ status: Int) throws -> T {
        guard (200..<300).contains(status) else { throw ErrorDecoding.error(status: status, data: data) }
        do { return try JSONDecoder().decode(T.self, from: data) } catch { throw FilesError.invalidResponse("\(T.self): \(error)") }
    }

    static func now() -> String {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime]
        return f.string(from: Date())
    }
}
