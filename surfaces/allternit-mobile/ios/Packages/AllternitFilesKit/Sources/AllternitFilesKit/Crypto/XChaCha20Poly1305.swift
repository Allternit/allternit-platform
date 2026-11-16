import CryptoKit
import Foundation

/// XChaCha20-Poly1305 (draft-irtf-cfrg-xchacha-03), the cipher of every
/// sealed chunk, replica head and key envelope (SPEC section 13).
///
/// CryptoKit has ChaCha20-Poly1305 (RFC 8439, 12-byte nonce) but not the
/// 24-byte-nonce X variant, so this is the draft's construction on top of
/// it: `subkey = HChaCha20(key, nonce[0..<16])`, then ChaCha20-Poly1305 with
/// `subkey` and the 12-byte nonce `00 00 00 00 || nonce[16..<24]`. The
/// output is `ciphertext || 16-byte tag`, the same bytes RustCrypto's
/// `XChaCha20Poly1305` (the engine) and libsodium produce.
/// Checked by `KeyVectorTests` against the draft's vectors
/// (`contracts/files-drive/vectors/keys-v1.json`).
public enum XChaCha20Poly1305 {
    public static let keyBytes = 32
    public static let nonceBytes = 24
    public static let tagBytes = 16

    /// Seals `plaintext`; returns `ciphertext || tag`.
    public static func seal(_ plaintext: Data, key: Data, nonce: Data, aad: Data) throws -> Data {
        let (subkey, inner) = try derive(key: key, nonce: nonce)
        let box = try ChaChaPoly.seal(plaintext, using: subkey, nonce: inner, authenticating: aad)
        return box.ciphertext + box.tag
    }

    /// Opens `ciphertext || tag`. Throws when the tag does not match (a
    /// changed byte, a wrong key or a different AAD).
    public static func open(_ sealed: Data, key: Data, nonce: Data, aad: Data) throws -> Data {
        guard sealed.count >= tagBytes else { throw CryptoKitError.authenticationFailure }
        let (subkey, inner) = try derive(key: key, nonce: nonce)
        let split = sealed.index(sealed.endIndex, offsetBy: -tagBytes)
        let box = try ChaChaPoly.SealedBox(nonce: inner, ciphertext: sealed[sealed.startIndex..<split], tag: sealed[split...])
        return try ChaChaPoly.open(box, using: subkey, authenticating: aad)
    }

    private static func derive(key: Data, nonce: Data) throws -> (SymmetricKey, ChaChaPoly.Nonce) {
        guard key.count == keyBytes, nonce.count == nonceBytes else { throw CryptoKitError.incorrectParameterSize }
        let n = [UInt8](nonce)
        let subkey = hChaCha20(key: [UInt8](key), nonce16: Array(n[0..<16]))
        let inner = try ChaChaPoly.Nonce(data: Data([0, 0, 0, 0] + n[16..<24]))
        return (SymmetricKey(data: subkey), inner)
    }

    /// HChaCha20 (draft section 2.2): the ChaCha20 block function's 20
    /// rounds over constants, key and the 16-byte nonce, without the final
    /// addition; the subkey is state words 0-3 and 12-15, little-endian.
    public static func hChaCha20(key: [UInt8], nonce16: [UInt8]) -> [UInt8] {
        precondition(key.count == 32 && nonce16.count == 16)
        func word(_ b: [UInt8], _ i: Int) -> UInt32 {
            UInt32(b[i]) | UInt32(b[i + 1]) << 8 | UInt32(b[i + 2]) << 16 | UInt32(b[i + 3]) << 24
        }
        var s = [UInt32](repeating: 0, count: 16)
        s[0] = 0x6170_7865; s[1] = 0x3320_646e; s[2] = 0x7962_2d32; s[3] = 0x6b20_6574
        for i in 0..<8 { s[4 + i] = word(key, 4 * i) }
        for i in 0..<4 { s[12 + i] = word(nonce16, 4 * i) }
        @inline(__always) func rotl(_ x: UInt32, _ n: UInt32) -> UInt32 { (x << n) | (x >> (32 - n)) }
        func quarter(_ a: Int, _ b: Int, _ c: Int, _ d: Int) {
            s[a] = s[a] &+ s[b]; s[d] = rotl(s[d] ^ s[a], 16)
            s[c] = s[c] &+ s[d]; s[b] = rotl(s[b] ^ s[c], 12)
            s[a] = s[a] &+ s[b]; s[d] = rotl(s[d] ^ s[a], 8)
            s[c] = s[c] &+ s[d]; s[b] = rotl(s[b] ^ s[c], 7)
        }
        for _ in 0..<10 {
            quarter(0, 4, 8, 12); quarter(1, 5, 9, 13); quarter(2, 6, 10, 14); quarter(3, 7, 11, 15)
            quarter(0, 5, 10, 15); quarter(1, 6, 11, 12); quarter(2, 7, 8, 13); quarter(3, 4, 9, 14)
        }
        var out = [UInt8]()
        out.reserveCapacity(32)
        for w in [s[0], s[1], s[2], s[3], s[12], s[13], s[14], s[15]] {
            out.append(UInt8(w & 0xFF)); out.append(UInt8((w >> 8) & 0xFF))
            out.append(UInt8((w >> 16) & 0xFF)); out.append(UInt8(w >> 24))
        }
        return out
    }
}
