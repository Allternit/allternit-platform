import CryptoKit
import XCTest
@testable import Allternit

/// The iPhone receiver against the shared vector the web/Desktop sender is
/// tested with (docs/voice-handoff-vectors.json, bundled as a test resource).
final class VoiceKeyHandoffTests: XCTestCase {
    private struct Vector: Decodable {
        struct Party: Decodable { let private_scalar_hex: String; let public_uncompressed_b64url: String }
        struct Confirm: Decodable { let code: String }
        struct HKDF: Decodable { let okm_hex: String }
        struct GCM: Decodable { let plaintext_utf8: String }
        let sender: Party
        let receiver: Party
        let sid: String
        let confirm_code: Confirm
        let hkdf: HKDF
        let aes_gcm: GCM
        let qr1: String
        let qr2: String
    }

    private func vector() throws -> Vector {
        let url = try XCTUnwrap(Bundle(for: Self.self).url(forResource: "voice-handoff-vectors", withExtension: "json"))
        return try JSONDecoder().decode(Vector.self, from: Data(contentsOf: url))
    }

    private func hex(_ text: String) -> Data {
        var data = Data()
        var index = text.startIndex
        while index < text.endIndex {
            let next = text.index(index, offsetBy: 2)
            data.append(UInt8(text[index..<next], radix: 16)!)
            index = next
        }
        return data
    }

    func testReceiverOpensTheSharedVector() throws {
        let v = try vector()
        let receiver = try P256.KeyAgreement.PrivateKey(rawRepresentation: hex(v.receiver.private_scalar_hex))
        XCTAssertEqual(VoiceHandoffCrypto.base64URLEncode(receiver.publicKey.x963Representation), v.receiver.public_uncompressed_b64url)

        guard case .offer(let sid, let senderKey)? = VoiceHandoffCrypto.QR(v.qr1) else { return XCTFail("QR 1 did not parse") }
        XCTAssertEqual(sid, v.sid)
        XCTAssertEqual(VoiceHandoffCrypto.base64URLEncode(senderKey), v.sender.public_uncompressed_b64url)

        let code = VoiceHandoffCrypto.confirmCode(sid: sid, senderKey: senderKey, receiverKey: receiver.publicKey.x963Representation)
        XCTAssertEqual(code, v.confirm_code.code)
        XCTAssertEqual(code, "847880")

        let key = try VoiceHandoffCrypto.sessionKey(privateKey: receiver, senderKey: senderKey, sid: sid)
        XCTAssertEqual(key.withUnsafeBytes { Data($0) }, hex(v.hkdf.okm_hex))

        guard case .sealed(let sid2, let nonce, let ciphertext)? = VoiceHandoffCrypto.QR(v.qr2) else { return XCTFail("QR 2 did not parse") }
        XCTAssertEqual(sid2, sid)
        let payload = try VoiceHandoffCrypto.open(nonce: nonce, ciphertext: ciphertext, key: key)
        let expected = try JSONDecoder().decode(VoiceHandoffCrypto.Payload.self, from: Data(v.aes_gcm.plaintext_utf8.utf8))
        XCTAssertEqual(payload, expected)
        XCTAssertEqual(payload.provider, "elevenlabs")
        XCTAssertEqual(payload.apiKey, "sk_test_123")
        XCTAssertEqual(payload.voiceId, "21m00Tcm4TlvDq8ikWAM")
    }

    /// A photo of both QR codes is useless without the phone's private key.
    func testAnotherPrivateKeyCannotOpenQR2() throws {
        let v = try vector()
        guard case .offer(let sid, let senderKey)? = VoiceHandoffCrypto.QR(v.qr1),
              case .sealed(_, let nonce, let ciphertext)? = VoiceHandoffCrypto.QR(v.qr2) else { return XCTFail("QRs did not parse") }
        let attacker = P256.KeyAgreement.PrivateKey()
        let key = try VoiceHandoffCrypto.sessionKey(privateKey: attacker, senderKey: senderKey, sid: sid)
        XCTAssertThrowsError(try VoiceHandoffCrypto.open(nonce: nonce, ciphertext: ciphertext, key: key)) { error in
            XCTAssertEqual(error as? VoiceHandoffCrypto.Failure, .cannotOpen)
        }
    }

    func testOtherQRCodesAreIgnored() {
        XCTAssertNil(VoiceHandoffCrypto.QR("https://ai.allternit.com"))
        XCTAssertNil(VoiceHandoffCrypto.QR("allternit-voice:1:sid:not-a-key"))
        XCTAssertNil(VoiceHandoffCrypto.QR("allternit-voice:3:sid:a:b"))
        XCTAssertNil(VoiceHandoffCrypto.QR("allternit-voice:2:sid:oaKjpKWmp6ipqqus"))
    }

    // MARK: Relay polling (one scan)

    private actor FakeRelay {
        var pubB: Data?
        var fetches = 0
        var answers: [VoiceHandoffRelay.Sealed]
        init(_ answers: [VoiceHandoffRelay.Sealed]) { self.answers = answers }
        func register(_ pub: Data) { pubB = pub }
        func next() -> VoiceHandoffRelay.Sealed {
            fetches += 1
            return answers.count > 1 ? answers.removeFirst() : answers[0]
        }
        func setAnswers(_ a: [VoiceHandoffRelay.Sealed]) { answers = a }
    }

    private func relay(_ fake: FakeRelay) -> VoiceHandoffRelay {
        VoiceHandoffRelay(register: { _, pub in await fake.register(pub) }, fetchSealed: { _ in await fake.next() })
    }

    private func vendorClient401() -> VoiceVendorClient {
        VoiceVendorStub.routes = ["/v1/models": (401, "{}"), "/v1/user": (401, "{}"), "/v1/voices": (401, "{}")]
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [VoiceVendorStub.self]
        let base = URL(string: "https://fake-vendor.test")!
        return VoiceVendorClient(session: URLSession(configuration: config), openAIBase: base, elevenLabsBase: base)
    }

    @MainActor
    private func waitFor(_ session: VoiceKeyHandoffSession, timeout: TimeInterval = 5, _ done: (VoiceKeyHandoffSession.Phase) -> Bool) async {
        let end = Date().addingTimeInterval(timeout)
        while Date() < end, !done(session.phase) { try? await Task.sleep(nanoseconds: 20_000_000) }
    }

    /// Seals the vector's payload for the phone's key, the way the computer does.
    private func seal(_ v: Vector, receiverKey: Data) throws -> (Data, Data) {
        let sender = try P256.KeyAgreement.PrivateKey(rawRepresentation: hex(v.sender.private_scalar_hex))
        let key = try VoiceHandoffCrypto.sessionKey(privateKey: sender, senderKey: receiverKey, sid: v.sid)
        let nonce = AES.GCM.Nonce()
        let box = try AES.GCM.seal(Data(v.aes_gcm.plaintext_utf8.utf8), using: key, nonce: nonce)
        return (Data(nonce), box.ciphertext + box.tag)
    }

    @MainActor
    func testPolledSealedKeyIsOpenedAndChecked() async throws {
        let v = try vector()
        let fake = FakeRelay([.waiting])
        let session = VoiceKeyHandoffSession(vendorClient: vendorClient401(), relay: relay(fake), pollInterval: 0.05)
        session.scanned(v.qr1)
        await waitFor(session) { if case .confirm = $0 { return true }; return false }
        let capturedPub = await fake.pubB
        let pubB = try XCTUnwrap(capturedPub)
        let (nonce, ct) = try seal(v, receiverKey: pubB)
        try? await Task.sleep(nanoseconds: 150_000_000)
        let waited = await fake.fetches
        XCTAssertGreaterThan(waited, 1)
        await fake.setAnswers([.sealed(nonce: nonce, ciphertext: ct)])
        await waitFor(session) { if case .failed = $0 { return true }; return false }
        // The vendor stub answers 401: the vendor rejection proves the polled
        // bytes were decrypted (and nothing was saved to the Keychain).
        guard case .failed(let message) = session.phase else { return XCTFail("expected failed, got \(session.phase)") }
        XCTAssertEqual(message, "ElevenLabs didn't accept this key. Copy it again from your ElevenLabs account.")
        XCTAssertFalse(message.contains("couldn't open"))
    }

    @MainActor
    func testRelayNotFoundShowsExpired() async throws {
        let v = try vector()
        let session = VoiceKeyHandoffSession(vendorClient: vendorClient401(), relay: relay(FakeRelay([.notFound])), pollInterval: 0.05)
        session.scanned(v.qr1)
        await waitFor(session) { if case .failed = $0 { return true }; return false }
        XCTAssertEqual(session.phase, .failed(VoiceKeyHandoffSession.expiredMessage))
    }

    @MainActor
    func testRelayDeliveredStopsPollingAndStaysInConfirm() async throws {
        let v = try vector()
        let fake = FakeRelay([.delivered])
        let session = VoiceKeyHandoffSession(vendorClient: vendorClient401(), relay: relay(fake), pollInterval: 0.03)
        session.scanned(v.qr1)
        try? await Task.sleep(nanoseconds: 400_000_000)
        let fetches = await fake.fetches
        XCTAssertEqual(fetches, 1)
        guard case .confirm = session.phase else { return XCTFail("expected confirm, got \(session.phase)") }
        session.cancel()
    }
}
