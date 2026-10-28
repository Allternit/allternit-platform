import CommonCrypto
import CryptoKit
import Foundation
import Security

// -----------------------------------------------------------------------------
// RFB sign-in for a paired computer's own screen sharing (the bot sandbox runs
// `x11vnc -nopw` and never gets here). Same two types noVNC answers on the web
// (allternit-ai src/components/computers/ComputerViewport.tsx,
// `credentialsrequired`):
//
// - VNC Authentication (2): DES-encrypt the server's 16-byte challenge with
//   the password (RFC 6143 §7.2.2).
// - Apple Remote Desktop (30), what macOS Screen Sharing offers: Diffie-Hellman
//   over the server's prime, then the user name and password (64 bytes each,
//   NUL-terminated, random-filled) AES-128-ECB encrypted with MD5(shared key).
// -----------------------------------------------------------------------------

/// What the person typed for the computer's screen sharing: the account they
/// log in to that computer with, not their Allternit sign-in. Held in memory
/// for the viewer's lifetime only.
struct RFBCredentials: Sendable, Equatable {
    var username: String
    var password: String
}

enum RFBAuth {
    /// Security types the client can answer, best first: none, Apple's
    /// (user name + password), plain VNC password.
    static let supportedTypes: [UInt8] = [1, 30, 2]

    // MARK: - VNC Authentication (2)

    /// The 16-byte response to a VNC Authentication challenge. The key is the
    /// password's first 8 bytes, zero-padded, each byte bit-reversed (the
    /// historical VNC quirk every server expects).
    static func vncResponse(challenge: [UInt8], password: String) -> [UInt8]? {
        guard challenge.count == 16 else { return nil }
        var key = [UInt8](repeating: 0, count: 8)
        for (i, byte) in password.utf8.prefix(8).enumerated() {
            key[i] = reverseBits(byte)
        }
        return ecbEncrypt(algorithm: CCAlgorithm(kCCAlgorithmDES), key: key, input: challenge)
    }

    private static func reverseBits(_ byte: UInt8) -> UInt8 {
        var b = byte, r: UInt8 = 0
        for _ in 0..<8 {
            r = (r << 1) | (b & 1)
            b >>= 1
        }
        return r
    }

    // MARK: - Apple Remote Desktop (30)

    /// The client's answer to the ARD key exchange: 128 bytes of encrypted
    /// credentials, then the client's public key (`prime.count` bytes).
    /// Nil when the server's parameters are unusable (even or empty prime).
    static func ardResponse(
        generator: [UInt8],
        prime: [UInt8],
        serverPublicKey: [UInt8],
        credentials: RFBCredentials
    ) -> [UInt8]? {
        let keyLength = prime.count
        guard keyLength > 0, serverPublicKey.count == keyLength else { return nil }
        guard let modulus = Montgomery(modulus: prime) else { return nil }

        var privateKey = [UInt8](repeating: 0, count: keyLength)
        guard SecRandomCopyBytes(kSecRandomDefault, keyLength, &privateKey) == errSecSuccess else { return nil }

        let clientPublicKey = modulus.power(base: generator, exponent: privateKey, length: keyLength)
        let sharedKey = modulus.power(base: serverPublicKey, exponent: privateKey, length: keyLength)

        var block = [UInt8](repeating: 0, count: 128)
        guard SecRandomCopyBytes(kSecRandomDefault, block.count, &block) == errSecSuccess else { return nil }
        let user = Array(credentials.username.utf8.prefix(63))
        let pass = Array(credentials.password.utf8.prefix(63))
        block.replaceSubrange(0..<user.count, with: user)
        block[user.count] = 0
        block.replaceSubrange(64..<(64 + pass.count), with: pass)
        block[64 + pass.count] = 0

        let aesKey = Array(Insecure.MD5.hash(data: Data(sharedKey)))
        guard let encrypted = ecbEncrypt(algorithm: CCAlgorithm(kCCAlgorithmAES), key: aesKey, input: block) else {
            return nil
        }
        return encrypted + clientPublicKey
    }

    // MARK: - Block cipher

    private static func ecbEncrypt(algorithm: CCAlgorithm, key: [UInt8], input: [UInt8]) -> [UInt8]? {
        var output = [UInt8](repeating: 0, count: input.count + 16)
        var moved = 0
        let status = CCCrypt(
            CCOperation(kCCEncrypt), algorithm, CCOptions(kCCOptionECBMode),
            key, key.count, nil,
            input, input.count,
            &output, output.count, &moved
        )
        guard status == kCCSuccess, moved == input.count else { return nil }
        return Array(output.prefix(moved))
    }
}

/// Modular exponentiation for the ARD Diffie-Hellman exchange (a 1024-bit or
/// larger odd prime), with Montgomery multiplication over 32-bit limbs.
/// Limbs are little-endian; byte arrays in and out are big-endian.
private struct Montgomery {
    let m: [UInt32]
    let n: Int
    /// -m⁻¹ mod 2³².
    let mInv: UInt32
    /// R² mod m, R = 2^(32n).
    let r2: [UInt32]

    init?(modulus bytes: [UInt8]) {
        let limbs = Self.limbs(from: bytes, count: (bytes.count + 3) / 4)
        guard let low = limbs.first, low & 1 == 1 else { return nil }
        m = limbs
        n = limbs.count
        var inv: UInt32 = 1
        for _ in 0..<5 { inv = inv &* (2 &- low &* inv) }
        mInv = 0 &- inv
        // R² mod m by doubling 1 a total of 64n times, reducing as it goes.
        var r = [UInt32](repeating: 0, count: n)
        r[0] = 1
        for _ in 0..<(64 * n) {
            let carry = Self.shiftLeftOne(&r)
            if carry || !Self.less(r, limbs) { Self.subtract(&r, limbs) }
        }
        r2 = r
    }

    /// base^exponent mod m as `length` big-endian bytes.
    func power(base: [UInt8], exponent: [UInt8], length: Int) -> [UInt8] {
        let x = multiply(Self.limbs(from: base, count: n), r2)
        var one = [UInt32](repeating: 0, count: n)
        one[0] = 1
        var acc = multiply(one, r2)
        for byte in exponent {
            for bit in stride(from: 7, through: 0, by: -1) {
                acc = multiply(acc, acc)
                if (byte >> UInt8(bit)) & 1 == 1 { acc = multiply(acc, x) }
            }
        }
        return Self.bytes(from: multiply(acc, one), length: length)
    }

    /// a·b·R⁻¹ mod m (CIOS).
    private func multiply(_ a: [UInt32], _ b: [UInt32]) -> [UInt32] {
        var t = [UInt32](repeating: 0, count: n + 2)
        for i in 0..<n {
            var carry: UInt64 = 0
            let bi = UInt64(b[i])
            for j in 0..<n {
                let x = UInt64(t[j]) + UInt64(a[j]) * bi + carry
                t[j] = UInt32(truncatingIfNeeded: x)
                carry = x >> 32
            }
            var x = UInt64(t[n]) + carry
            t[n] = UInt32(truncatingIfNeeded: x)
            t[n + 1] = UInt32(x >> 32)

            let q = UInt64(t[0] &* mInv)
            x = UInt64(t[0]) + q * UInt64(m[0])
            carry = x >> 32
            for j in 1..<n {
                x = UInt64(t[j]) + q * UInt64(m[j]) + carry
                t[j - 1] = UInt32(truncatingIfNeeded: x)
                carry = x >> 32
            }
            x = UInt64(t[n]) + carry
            t[n - 1] = UInt32(truncatingIfNeeded: x)
            t[n] = t[n + 1] &+ UInt32(x >> 32)
            t[n + 1] = 0
        }
        var result = Array(t[0..<n])
        if t[n] != 0 || !Self.less(result, m) { Self.subtract(&result, m) }
        return result
    }

    // MARK: - Limb helpers

    private static func limbs(from bytes: [UInt8], count: Int) -> [UInt32] {
        var out = [UInt32](repeating: 0, count: count)
        for (i, byte) in bytes.reversed().enumerated() where i / 4 < count {
            out[i / 4] |= UInt32(byte) << UInt32(8 * (i % 4))
        }
        return out
    }

    private static func bytes(from limbs: [UInt32], length: Int) -> [UInt8] {
        var out = [UInt8](repeating: 0, count: length)
        for i in 0..<length where i / 4 < limbs.count {
            out[length - 1 - i] = UInt8(truncatingIfNeeded: limbs[i / 4] >> UInt32(8 * (i % 4)))
        }
        return out
    }

    private static func less(_ a: [UInt32], _ b: [UInt32]) -> Bool {
        for i in stride(from: a.count - 1, through: 0, by: -1) where a[i] != b[i] {
            return a[i] < b[i]
        }
        return false
    }

    /// a -= b (wrapping; callers only subtract when a ≥ b or a overflowed).
    private static func subtract(_ a: inout [UInt32], _ b: [UInt32]) {
        var borrow: UInt64 = 0
        for i in 0..<a.count {
            let d = UInt64(a[i]) &- UInt64(b[i]) &- borrow
            a[i] = UInt32(truncatingIfNeeded: d)
            borrow = (d >> 63) & 1
        }
    }

    /// a <<= 1, returning the bit shifted out.
    private static func shiftLeftOne(_ a: inout [UInt32]) -> Bool {
        var carry: UInt32 = 0
        for i in 0..<a.count {
            let next = a[i] >> 31
            a[i] = (a[i] << 1) | carry
            carry = next
        }
        return carry == 1
    }
}
