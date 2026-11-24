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

/// A waiting key-share request from another device, with the code computed
/// here from its public key (never the code the request carries: a swapped
/// key would show a different code than the new device does).
public struct FilesKeyApproval: Sendable, Hashable, Identifiable {
    public var request: PendingKeyRequest
    /// Computed from `request.devicePub` on this device.
    public var code: String
    /// The code the request carries matches `code`. A mismatch is never approvable.
    public var codeMatches: Bool
    public var id: String { request.deviceId }

    public init(request: PendingKeyRequest) {
        self.request = request
        if let pub = FilesKeyCrypto.fromBase64url(request.devicePub), pub.count == 32 {
            code = FilesKeyCrypto.confirmCode(devicePub: pub)
            codeMatches = code == request.confirmCode
        } else {
            code = request.confirmCode
            codeMatches = false
        }
    }
}

/// cloud-api's Files key routes (SPEC section 2, "cloud-api"; section 13)
/// and this device's key ring:
///
/// - `GET    /api/v1/files-keys`: `FilesKeyInfo` (no `key_id` while the account has none)
/// - `POST   /api/v1/files-keys/requests`: `KeyShareRequest`, 202 `PendingKeyRequest`
/// - `DELETE /api/v1/files-keys/requests/:device_id`: decline a waiting request
/// - `POST   /api/v1/files-keys/grants`: `KeyShareGrant` (with the request's code), 204
/// - `POST   /api/v1/files-keys/rotations`: `KeyRotation`, 204
/// - `GET    /api/v1/files-keys/envelopes/:device_id`: `KeyEnvelope` (404 until approved)
/// - `DELETE /api/v1/files-keys/devices/:device_id`: remove a device, 204
///
/// The app (Settings > Files > Devices) uses all of it; the File Provider
/// extension only calls `rootKey(store:keyId:)`, which reads the shared
/// keychain and never touches the network.
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
    public func state(_ known: FilesKeyInfo? = nil) async throws -> FilesKeyState {
        let fetched: FilesKeyInfo?
        if let known { fetched = known } else { fetched = try await keyInfo() }
        guard let info = fetched, let keyId = info.keyId else {
            store.pendingKeyRequest = nil
            return .noKey
        }
        if store.filesKeyEnvelopes().contains(where: { $0.keyId == keyId }),
           (try? Self.rootKey(store: store, keyId: keyId)) != nil {
            store.pendingKeyRequest = nil
            return .ready(keyId: keyId)
        }
        if let envelope = try await envelope(), envelope.keyId == keyId,
           (try? FilesKeyCrypto.unwrap(envelope, deviceSecret: try deviceKey())) != nil {
            keep(envelope)
            store.pendingKeyRequest = nil
            return .ready(keyId: keyId)
        }
        // Still waiting only while cloud-api lists this device's request with
        // this device's key (it lapses after 10 minutes).
        let me = try deviceId
        let pub = FilesKeyCrypto.base64url(try deviceKey().publicKey.rawRepresentation)
        if let mine = info.pendingRequests.first(where: { $0.deviceId == me && $0.devicePub == pub }) {
            return .waiting(code: mine.confirmCode)
        }
        store.pendingKeyRequest = nil
        return .needsRequest
    }

    /// `GET /api/v1/files-keys`; nil only when the route is missing (404).
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

    /// This device's own request (its public key and the derived code).
    func ownRequest(deviceName: String) throws -> KeyShareRequest {
        let pub = try deviceKey().publicKey.rawRepresentation
        return KeyShareRequest(deviceId: try deviceId, devicePub: FilesKeyCrypto.base64url(pub),
                               deviceName: deviceName, confirmCode: FilesKeyCrypto.confirmCode(devicePub: pub))
    }

    private func post(_ request: KeyShareRequest) async throws -> PendingKeyRequest {
        let (data, status) = try await call("POST", "api/v1/files-keys/requests", body: try JSONEncoder().encode(request))
        return try decode(data, status)
    }

    // MARK: First device

    /// Makes the account's Files key on this device (only while the account
    /// has none): posts this device's own request and grants it to itself.
    /// When another device made the key first, cloud-api answers
    /// `already_exists`: ask that device instead.
    @discardableResult
    public func createFirstKey(deviceName: String) async throws -> String {
        if let keyId = try await keyInfo()?.keyId {
            throw FilesError.keyUnavailable("Your account already has a Files key (\(keyId)). Approve this device from another one.")
        }
        let me = try deviceId
        let pending = try await post(try ownRequest(deviceName: deviceName))
        let frk = FilesRootKey.generate()
        let envelope = try FilesKeyCrypto.wrap(frk, toDevicePub: try deviceKey().publicKey.rawRepresentation,
                                               deviceId: me, createdAt: Self.now())
        try await grant(KeyShareGrant(envelope: envelope, approvedByDeviceId: me, confirmCode: pending.confirmCode))
        keep(envelope)
        store.pendingKeyRequest = nil
        return frk.keyId
    }

    // MARK: New device

    /// Sends this device's `KeyShareRequest` and returns the 6-digit code to
    /// show until another device approves. Sending again is safe (it
    /// replaces the earlier request and restarts the 10 minutes).
    @discardableResult
    public func requestAccess(deviceName: String) async throws -> String {
        let request = try ownRequest(deviceName: deviceName)
        let pending = try await post(request)
        store.pendingKeyRequest = request
        return pending.confirmCode
    }

    // MARK: Approving device

    /// Waiting requests from the user's other devices, each with the code
    /// recomputed here. This device's own request is left out.
    public func approvals(_ info: FilesKeyInfo) -> [FilesKeyApproval] {
        let me = store.deviceId
        return info.pendingRequests.filter { $0.deviceId != me }.map(FilesKeyApproval.init(request:))
    }

    /// Wraps the current key to the requesting device and posts the grant.
    /// Call only after the user saw the same code on both devices.
    public func approve(_ approval: FilesKeyApproval) async throws {
        guard approval.codeMatches, let pub = FilesKeyCrypto.fromBase64url(approval.request.devicePub), pub.count == 32 else {
            throw FilesError.keyUnavailable("The code from \(approval.request.deviceName) doesn't match its key. Don't approve it; ask it to request again.")
        }
        guard let current = try await keyInfo()?.keyId else {
            throw FilesError.keyUnavailable("There is no Files key yet.")
        }
        let frk = try Self.rootKey(store: store, keyId: current)
        let envelope = try FilesKeyCrypto.wrap(frk, toDevicePub: pub, deviceId: approval.request.deviceId, createdAt: Self.now())
        try await grant(KeyShareGrant(envelope: envelope, approvedByDeviceId: try deviceId,
                                      confirmCode: approval.request.confirmCode))
    }

    /// Declines a waiting request (`DELETE /files-keys/requests/:device_id`).
    /// Only a device that holds the key may; cloud-api checks it.
    public func decline(_ approval: FilesKeyApproval) async throws {
        let me = try deviceId
        let path = "api/v1/files-keys/requests/\(FilesClient.escape(approval.request.deviceId))"
            + "?declined_by_device_id=\(FilesClient.escape(me))"
        let (data, status) = try await call("DELETE", path)
        guard (200..<300).contains(status) else { throw ErrorDecoding.error(status: status, data: data) }
    }

    /// Removes a device from the Files key; the key then needs rotating.
    public func removeDevice(_ deviceId: DeviceId) async throws {
        let (data, status) = try await call("DELETE", "api/v1/files-keys/devices/\(FilesClient.escape(deviceId))")
        guard (200..<300).contains(status) else { throw ErrorDecoding.error(status: status, data: data) }
        if deviceId == store.deviceId { store.setFilesKeyEnvelopes([]) }
    }

    /// Makes a new Files key and seals it to every device still listed (this
    /// one included), after a device was removed (`rotation_needed`). The
    /// removed device is no longer listed, so it never gets the new key.
    /// cloud-api answers `rev_mismatch` when the list changed: reload, retry.
    @discardableResult
    public func rotate(_ info: FilesKeyInfo) async throws -> String {
        guard let current = info.keyId else { throw FilesError.keyUnavailable("There is no Files key yet.") }
        _ = try Self.rootKey(store: store, keyId: current) // this device must hold it
        let me = try deviceId
        let frk = FilesRootKey.generate()
        var envelopes: [KeyEnvelope] = []
        for device in info.devices {
            guard let pub = FilesKeyCrypto.fromBase64url(device.devicePub), pub.count == 32 else {
                throw FilesError.invalidResponse("\(device.deviceName) has a bad public key")
            }
            envelopes.append(try FilesKeyCrypto.wrap(frk, toDevicePub: pub, deviceId: device.deviceId, createdAt: Self.now()))
        }
        let rotation = KeyRotation(keyId: frk.keyId, rotatedFrom: current, rotatedByDeviceId: me, envelopes: envelopes)
        let (data, status) = try await call("POST", "api/v1/files-keys/rotations", body: try JSONEncoder().encode(rotation))
        guard (200..<300).contains(status) else { throw ErrorDecoding.error(status: status, data: data) }
        if let mine = envelopes.first(where: { $0.deviceId == me }) { keep(mine) }
        return frk.keyId
    }

    // MARK: Computers

    /// Gives a computer the Files key: reads its request from its runtime
    /// (`GET /replicas/key` over the owner's relay), posts it, approves it
    /// (the code is checked here: the request came over the owner's own
    /// relay, so there is no second screen), then delivers the envelope.
    public func giveComputer(_ client: FilesClient) async throws {
        let (data, response) = try await client.send(FilesRequest(path: "/replicas/key"))
        guard response.statusCode == 200 else { throw ErrorDecoding.error(status: response.statusCode, data: data) }
        let request: KeyShareRequest
        do { request = try JSONDecoder().decode(KeyShareRequest.self, from: data) } catch {
            throw FilesError.invalidResponse("KeyShareRequest: \(error)")
        }
        let pending = try await post(request)
        try await approve(FilesKeyApproval(request: pending))
        try await deliver(to: client, deviceId: request.deviceId)
    }

    /// `PUT /replicas/key` with the computer's current envelope. Replacing a
    /// different key on the computer needs the signed unlock token; the
    /// client sends it when one is held (`FilesClient.unlock(reason:)`).
    public func deliver(to client: FilesClient, deviceId: DeviceId) async throws {
        let (envData, status) = try await call("GET", "api/v1/files-keys/envelopes/\(FilesClient.escape(deviceId))")
        let envelope: KeyEnvelope = try decode(envData, status)
        let body = try JSONEncoder().encode(envelope)
        let (data, response) = try await client.send(FilesRequest(method: "PUT", path: "/replicas/key",
                                                                  headers: ["Content-Type": "application/json"],
                                                                  body: .json(body)))
        guard (200..<300).contains(response.statusCode) else { throw ErrorDecoding.error(status: response.statusCode, data: data) }
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
