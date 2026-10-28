// Ported from iCTRL (https://github.com/Allternit/allternit-ictrl,
// iCTRLKit/SecureEnvelope.swift and the X25519 pairing in AuthManager.swift).
// Copyright iCTRL authors. Licensed under the Apache License, Version 2.0;
// see http://www.apache.org/licenses/LICENSE-2.0. Modified for Allternit:
// both ends of an app phone socket seal with the request construction, and
// the pairing key comes from X25519 + HKDF instead of a minted random key.

import CryptoKit
import Foundation

/// Protocol v1 envelope, wire-identical to iCTRL's (the Rust node opens it
/// with the same construction):
///
///     K          = 32B session key (from pairing, see `sessionKey`)
///     tok        = first 4 bytes of SHA256(K), 8 hex chars (lookup id only)
///     nonce      = 16 random bytes per message
///     ts         = unix seconds (receiver accepts |ts - now| <= 120)
///
///     K_enc_req  = HMAC(K, "ictrl-v1|enc-req" || nonce)
///     K_mac      = HMAC(K, "ictrl-v1|mac"     || nonce)
///
///     keystream(Kx) = HMAC(Kx, nonce || be32(0)) || HMAC(Kx, nonce || be32(1)) || ...
///     ct  = plaintext XOR keystream(K_enc_req)
///     tag = HMAC(K_mac, "v1" || tok_4B || be64(ts) || nonce || ct)
///
///     {"v":1,"tok":"<hex>","ts":<int>,"nonce":"<hex>","ct":"<base64>","tag":"<hex>"}
///
/// Every app phone message, in either direction, is one such envelope: the
/// sender seals, the receiver opens. Encrypt-then-MAC, so a forged body is
/// rejected before anything is decrypted.
enum SecureEnvelope {
    /// Domain-separation labels. Changing these breaks the wire protocol.
    static let labelMac = "ictrl-v1|mac"
    static let labelEncRequest = "ictrl-v1|enc-req"

    /// Messages older/newer than this are rejected (replay + clock-skew bound).
    static let timestampWindow: TimeInterval = 120

    struct Failure: Error, Sendable {
        let code: String
        let message: String
    }

    // MARK: - Primitives

    static func hmac(_ key: Data, _ data: Data) -> Data {
        Data(HMAC<SHA256>.authenticationCode(for: data, using: SymmetricKey(data: key)))
    }

    /// PRF-CTR stream cipher: keystream_i = HMAC-SHA256(key, nonce || be32(i)).
    /// Same output as iCTRL's; XORs block by block instead of building the
    /// whole keystream first, because the extension seals video frames under
    /// a 50 MB ceiling.
    static func keystreamXOR(key: Data, nonce: Data, data: Data) -> Data {
        let symmetric = SymmetricKey(data: key)
        var out = [UInt8](data)
        var counter: UInt32 = 0
        var offset = 0
        var block = nonce
        block.append(contentsOf: [0, 0, 0, 0])
        let counterAt = block.count - 4
        while offset < out.count {
            block[counterAt] = UInt8(counter >> 24 & 0xff)
            block[counterAt + 1] = UInt8(counter >> 16 & 0xff)
            block[counterAt + 2] = UInt8(counter >> 8 & 0xff)
            block[counterAt + 3] = UInt8(counter & 0xff)
            let stream = HMAC<SHA256>.authenticationCode(for: block, using: symmetric)
            for byte in stream where offset < out.count {
                out[offset] ^= byte
                offset += 1
            }
            counter += 1
        }
        return Data(out)
    }

    static func be64(_ value: UInt64) -> Data {
        var be = value.bigEndian
        return Data(bytes: &be, count: 8)
    }

    /// The lookup id derived from a key: first 4 bytes of SHA256(K), hex.
    static func tokenId(for key: Data) -> String {
        Data(SHA256.hash(data: key)).prefix(4).hexString
    }

    static func randomBytes(_ count: Int) -> Data? {
        var bytes = [UInt8](repeating: 0, count: count)
        guard SecRandomCopyBytes(kSecRandomDefault, count, &bytes) == errSecSuccess else { return nil }
        return Data(bytes)
    }

    // MARK: - Seal / open

    /// Seals `plaintext` into the JSON envelope above (UTF-8 bytes). nil only
    /// when the system RNG fails.
    static func seal(_ plaintext: Data, key: Data, now: Date = Date()) -> Data? {
        guard let nonce = randomBytes(16) else { return nil }
        let tok4 = Data(SHA256.hash(data: key)).prefix(4)
        let ts = Int(now.timeIntervalSince1970)
        let kEnc = hmac(key, Data(labelEncRequest.utf8) + nonce)
        let kMac = hmac(key, Data(labelMac.utf8) + nonce)
        let ct = keystreamXOR(key: kEnc, nonce: nonce, data: plaintext)
        let tag = hmac(kMac, Data("v1".utf8) + tok4 + be64(UInt64(ts)) + nonce + ct)
        let envelope: [String: Any] = [
            "v": 1,
            "tok": tok4.hexString,
            "ts": ts,
            "nonce": nonce.hexString,
            "ct": ct.base64EncodedString(),
            "tag": tag.hexString,
        ]
        return try? JSONSerialization.data(withJSONObject: envelope, options: .withoutEscapingSlashes)
    }

    /// Verifies and decrypts one envelope. Returns the plaintext and the
    /// nonce (hex) so the caller can refuse a replay.
    static func open(_ envelope: Data, key: Data, now: Date = Date()) throws -> (plaintext: Data, nonceHex: String) {
        guard let obj = try? JSONSerialization.jsonObject(with: envelope) as? [String: Any],
              (obj["v"] as? Int) == 1,
              let tokId = obj["tok"] as? String,
              let ts = obj["ts"] as? Int, ts >= 0,
              let nonceHex = obj["nonce"] as? String,
              let ctB64 = obj["ct"] as? String,
              let tagHex = obj["tag"] as? String,
              let nonce = Data(hexString: nonceHex), nonce.count == 16,
              let ct = Data(base64Encoded: ctB64),
              let tag = Data(hexString: tagHex), tag.count == 32,
              let tok4 = Data(hexString: tokId), tok4.count == 4 else {
            throw Failure(code: "bad_envelope", message: "Malformed envelope fields")
        }
        guard tokId == tokenId(for: key) else {
            throw Failure(code: "auth_failed", message: "Envelope is for a different key")
        }
        guard abs(now.timeIntervalSince1970 - Double(ts)) <= timestampWindow else {
            throw Failure(code: "ts_skew", message: "Timestamp outside ±120s window")
        }
        let kMac = hmac(key, Data(labelMac.utf8) + nonce)
        let signed = Data("v1".utf8) + tok4 + be64(UInt64(ts)) + nonce + ct
        guard HMAC<SHA256>.isValidAuthenticationCode(tag, authenticating: signed,
                                                     using: SymmetricKey(data: kMac)) else {
            throw Failure(code: "bad_tag", message: "MAC verification failed")
        }
        let kEnc = hmac(key, Data(labelEncRequest.utf8) + nonce)
        return (keystreamXOR(key: kEnc, nonce: nonce, data: ct), nonceHex)
    }
}

// MARK: - Pairing (X25519 + HKDF)

extension SecureEnvelope {
    static let pairInfo = "allternit-app-phone v1"

    /// The session key both sides derive after the Desktop approves:
    /// HKDF-SHA256(X25519(appPriv, nodePub), salt = phoneId, info = pairInfo).
    static func sessionKey(privateKey: Curve25519.KeyAgreement.PrivateKey,
                           nodePublicKey: Data, phoneId: String) throws -> Data {
        let nodeKey = try Curve25519.KeyAgreement.PublicKey(rawRepresentation: nodePublicKey)
        let shared = try privateKey.sharedSecretFromKeyAgreement(with: nodeKey)
        let key = shared.hkdfDerivedSymmetricKey(using: SHA256.self,
                                                 salt: Data(phoneId.utf8),
                                                 sharedInfo: Data(pairInfo.utf8),
                                                 outputByteCount: 32)
        return key.withUnsafeBytes { Data($0) }
    }

    /// The 6-digit code shown on the phone and on the Desktop:
    /// SHA-256(appPub || nodePub), first 4 bytes big-endian, mod 1 000 000,
    /// zero-padded. Both public keys are the raw 32 bytes.
    static func pairCode(appPublicKey: Data, nodePublicKey: Data) -> String {
        let digest = Array(SHA256.hash(data: appPublicKey + nodePublicKey))
        let n = UInt32(digest[0]) << 24 | UInt32(digest[1]) << 16 | UInt32(digest[2]) << 8 | UInt32(digest[3])
        return String(format: "%06u", n % 1_000_000)
    }
}

// MARK: - Hex helpers

extension Data {
    init?(hexString hex: String) {
        guard hex.count % 2 == 0 else { return nil }
        var bytes = [UInt8]()
        bytes.reserveCapacity(hex.count / 2)
        var index = hex.startIndex
        while index < hex.endIndex {
            let next = hex.index(index, offsetBy: 2)
            guard let byte = UInt8(hex[index..<next], radix: 16) else { return nil }
            bytes.append(byte)
            index = next
        }
        self.init(bytes)
    }

    var hexString: String {
        map { String(format: "%02x", $0) }.joined()
    }
}
