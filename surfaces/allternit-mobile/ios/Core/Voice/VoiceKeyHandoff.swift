import CryptoKit
import Foundation

// The iPhone side of the voice key QR handoff (plan snoopy-napping-rabbit,
// decision 1). A computer that has the key (web, Desktop or PWA) shows QR 1;
// this phone scans it, makes a one-time P-256 key, posts only its public key
// to the cloud-api relay, and shows a 6-digit code. Once the user confirms
// the codes match on the computer, it shows QR 2: the key sealed with
// AES-256-GCM under HKDF of the ECDH shared secret. The scanner, still open,
// reads it and opens it with the private key that never leaves this phone's
// memory. The API key travels only from screen to camera.
//
// Wire format: docs/voice-handoff-vectors.json (generator:
// docs/voice-handoff-vectors.gen.mjs). Tests check this file against it.
//   QR1  = allternit-voice:1:<sid>:<pubA>
//   QR2  = allternit-voice:2:<sid>:<nonce>:<ciphertext||tag>
//   code = SHA-256(utf8(sid) || pubA_raw || pubB_raw) → first 4 bytes, big-endian
//          uint32, mod 1_000_000, zero-padded to 6 digits
//   key  = HKDF-SHA256(ikm = ECDH x-coordinate, salt = utf8(sid),
//                      info = "allternit-voice-key-handoff-v1", 32 bytes)
// Public keys are uncompressed SEC1 points (65 bytes); base64url is unpadded.

enum VoiceHandoffCrypto {
    static let info = "allternit-voice-key-handoff-v1"
    static let prefix = "allternit-voice"

    /// A scanned handoff QR code.
    enum QR: Equatable {
        /// QR 1: the session id and the computer's one-time public key.
        case offer(sid: String, senderKey: Data)
        /// QR 2: the sealed key for that session.
        case sealed(sid: String, nonce: Data, ciphertext: Data)

        var sid: String {
            switch self {
            case .offer(let sid, _), .sealed(let sid, _, _): return sid
            }
        }

        init?(_ text: String) {
            let parts = text.trimmingCharacters(in: .whitespacesAndNewlines).split(separator: ":", omittingEmptySubsequences: false).map(String.init)
            guard parts.count >= 4, parts[0] == VoiceHandoffCrypto.prefix, !parts[2].isEmpty else { return nil }
            switch (parts[1], parts.count) {
            case ("1", 4):
                guard let key = VoiceHandoffCrypto.base64URLDecode(parts[3]), key.count == 65, key.first == 0x04 else { return nil }
                self = .offer(sid: parts[2], senderKey: key)
            case ("2", 5):
                guard let nonce = VoiceHandoffCrypto.base64URLDecode(parts[3]), nonce.count == 12,
                      let sealed = VoiceHandoffCrypto.base64URLDecode(parts[4]), sealed.count > 16 else { return nil }
                self = .sealed(sid: parts[2], nonce: nonce, ciphertext: sealed)
            default:
                return nil
            }
        }
    }

    /// What QR 2 carries.
    struct Payload: Decodable, Equatable, Sendable {
        let provider: String
        let apiKey: String
        let voiceId: String?

        private enum CodingKeys: String, CodingKey {
            case provider, apiKey = "api_key", voiceId = "voice_id"
        }
    }

    enum Failure: Error, Equatable {
        case badKey, cannotOpen, badPayload
    }

    static func confirmCode(sid: String, senderKey: Data, receiverKey: Data) -> String {
        var hash = SHA256()
        hash.update(data: Data(sid.utf8))
        hash.update(data: senderKey)
        hash.update(data: receiverKey)
        let digest = Array(hash.finalize())
        let value = UInt32(digest[0]) << 24 | UInt32(digest[1]) << 16 | UInt32(digest[2]) << 8 | UInt32(digest[3])
        return String(format: "%06u", value % 1_000_000)
    }

    static func sessionKey(privateKey: P256.KeyAgreement.PrivateKey, senderKey: Data, sid: String) throws -> SymmetricKey {
        guard let peer = try? P256.KeyAgreement.PublicKey(x963Representation: senderKey) else { throw Failure.badKey }
        let shared = try privateKey.sharedSecretFromKeyAgreement(with: peer)
        return shared.hkdfDerivedSymmetricKey(using: SHA256.self, salt: Data(sid.utf8),
                                              sharedInfo: Data(info.utf8), outputByteCount: 32)
    }

    /// Opens QR 2's sealed box (AES-256-GCM, no AAD, 16-byte tag appended).
    static func open(nonce: Data, ciphertext: Data, key: SymmetricKey) throws -> Payload {
        let plain: Data
        do {
            let box = try AES.GCM.SealedBox(nonce: AES.GCM.Nonce(data: nonce),
                                            ciphertext: ciphertext.dropLast(16),
                                            tag: ciphertext.suffix(16))
            plain = try AES.GCM.open(box, using: key)
        } catch {
            throw Failure.cannotOpen
        }
        guard let payload = try? JSONDecoder().decode(Payload.self, from: plain),
              VoiceVendor(rawValue: payload.provider) != nil,
              !payload.apiKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { throw Failure.badPayload }
        return payload
    }

    static func base64URLEncode(_ data: Data) -> String {
        data.base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }

    static func base64URLDecode(_ text: String) -> Data? {
        var s = text.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
        while s.count % 4 != 0 { s += "=" }
        return Data(base64Encoded: s)
    }
}

/// One handoff, from scanning QR 1 to a key saved on this phone.
@MainActor
final class VoiceKeyHandoffSession: ObservableObject {
    enum Phase: Equatable {
        /// Point the camera at QR 1 on the computer.
        case scanning
        /// Telling the relay this phone's public key.
        case registering
        /// Show the code; keep scanning for QR 2.
        case confirm(code: String)
        /// QR 2 read; checking the key with the vendor.
        case checking(VoiceVendor)
        /// Saved on this phone.
        case done(VoiceVendor, label: String)
        case failed(String)
    }

    /// The relay's lifetime for one handoff.
    static let timeout: TimeInterval = 120

    @Published private(set) var phase: Phase = .scanning

    private var sid: String?
    private var senderKey: Data?
    private var privateKey: P256.KeyAgreement.PrivateKey?
    private var timeoutTask: Task<Void, Never>?
    private let vendorClient: VoiceVendorClient

    init(vendorClient: VoiceVendorClient = VoiceVendorClient()) {
        self.vendorClient = vendorClient
    }

    /// Start over (Try again).
    func restart() {
        timeoutTask?.cancel()
        sid = nil
        senderKey = nil
        privateKey = nil
        phase = .scanning
    }

    /// Ends the handoff and forgets the one-time key.
    func cancel() {
        timeoutTask?.cancel()
        privateKey = nil
    }

    /// Every QR string the scanner reads lands here; anything else is ignored.
    func scanned(_ text: String) {
        guard let qr = VoiceHandoffCrypto.QR(text) else { return }
        switch (phase, qr) {
        case (.scanning, .offer(let sid, let senderKey)):
            Task { await self.register(sid: sid, senderKey: senderKey) }
        case (.confirm, .sealed(let sid, let nonce, let ciphertext)) where sid == self.sid:
            Task { await self.receive(nonce: nonce, ciphertext: ciphertext) }
        default:
            break
        }
    }

    private func register(sid: String, senderKey: Data) async {
        phase = .registering
        let key = P256.KeyAgreement.PrivateKey()
        let pub = key.publicKey.x963Representation
        do {
            let client = APIClient.shared
            let url = AppConfig.cloudAPIBaseURL.appendingPathComponent("api/v1/voice/handoff/\(sid)/receiver")
            var request = try await client.authorizedRequest(url: url, method: "PUT")
            request.httpBody = try JSONSerialization.data(withJSONObject: ["pubkey": VoiceHandoffCrypto.base64URLEncode(pub)])
            let (data, response) = try await client.session.data(for: request)
            try client.validate(response, data: data)
        } catch {
            phase = .failed(Self.relayMessage(error))
            return
        }
        self.sid = sid
        self.senderKey = senderKey
        self.privateKey = key
        phase = .confirm(code: VoiceHandoffCrypto.confirmCode(sid: sid, senderKey: senderKey, receiverKey: pub))
        timeoutTask?.cancel()
        timeoutTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: UInt64(Self.timeout * 1_000_000_000))
            guard !Task.isCancelled, let self else { return }
            if case .confirm = self.phase {
                self.privateKey = nil
                self.phase = .failed("The code expired. Start again on your computer.")
            }
        }
    }

    private func receive(nonce: Data, ciphertext: Data) async {
        guard let privateKey, let senderKey, let sid else { return }
        let payload: VoiceHandoffCrypto.Payload
        do {
            let key = try VoiceHandoffCrypto.sessionKey(privateKey: privateKey, senderKey: senderKey, sid: sid)
            payload = try VoiceHandoffCrypto.open(nonce: nonce, ciphertext: ciphertext, key: key)
        } catch {
            phase = .failed("This phone couldn't open the key. Start again on your computer.")
            return
        }
        timeoutTask?.cancel()
        self.privateKey = nil
        guard let vendor = VoiceVendor(rawValue: payload.provider) else { return }
        phase = .checking(vendor)
        let check = await vendorClient.checkKey(vendor, key: payload.apiKey)
        if case .rejected(let message) = check {
            phase = .failed(message)
            return
        }
        let label = check.savedLabel ?? "Saved, not checked"
        VoiceProviderKeys.store(vendor, apiKey: payload.apiKey, voiceId: payload.voiceId, label: label)
        SettingsStore.shared.voiceRoute = vendor.route
        VoiceKeySync.shared.localChanged(vendor)
        phase = .done(vendor, label: label)
    }

    static func relayMessage(_ error: Error) -> String {
        if case APIError.httpError(let status, _) = error {
            switch status {
            case 401, 403: return "Sign in to Allternit on this phone, with the same account as your computer, then try again."
            case 404: return "This code has expired or belongs to another Allternit account. Sign in to the same account on both devices and start again on your computer."
            case 409: return "This code was already used. Start again on your computer."
            default: return "Allternit couldn't start the handoff (error \(status)). Try again."
            }
        }
        if error is URLError { return "No connection. Check your internet and try again." }
        return "Allternit couldn't start the handoff. Try again."
    }
}
