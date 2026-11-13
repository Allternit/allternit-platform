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
}
