import CryptoKit
import Foundation

/// Minisign signature check (https://jedisct1.github.io/minisign/), the
/// format Freeweights signs manifests with (`<id>.json.minisig`). Checks both
/// the file signature and the global signature over the trusted comment, and
/// that the key ids match. Accepts prehashed ("ED", BLAKE2b-512) and legacy
/// ("Ed") signatures.
public enum Minisign {
    public enum Failure: Error, Equatable, CustomStringConvertible {
        case badPublicKey
        case badSignatureFile
        case keyMismatch
        case invalidSignature
        case invalidTrustedComment

        public var description: String {
            switch self {
            case .badPublicKey: return "The public key is not a minisign Ed25519 key"
            case .badSignatureFile: return "The signature file is not in minisign format"
            case .keyMismatch: return "Signed by a different key"
            case .invalidSignature: return "The signature does not match the file"
            case .invalidTrustedComment: return "The trusted comment signature does not match"
            }
        }
    }

    public struct PublicKey: Sendable {
        public let keyId: [UInt8]
        let key: Curve25519.Signing.PublicKey

        /// `base64` is the second line of a `.pub` file (starts with "RWQ").
        public init(base64: String) throws {
            guard let raw = Data(base64Encoded: base64.trimmingCharacters(in: .whitespacesAndNewlines)),
                  raw.count == 42, raw[raw.startIndex] == 0x45, raw[raw.startIndex + 1] == 0x64 // "Ed"
            else { throw Failure.badPublicKey }
            keyId = Array(raw[(raw.startIndex + 2)..<(raw.startIndex + 10)])
            guard let k = try? Curve25519.Signing.PublicKey(rawRepresentation: raw[(raw.startIndex + 10)...])
            else { throw Failure.badPublicKey }
            key = k
        }
    }

    /// The Freeweights signing key (key id 2DC28CC7B26A283B, freeweights-v0,
    /// freeweights `spec/keys/freeweights-v0.pub`, docs/verify). Rotation
    /// ships a new key here; the old one stays listed for 90 days.
    public static let freeweightsKeys: [String] = [
        "RWQ7KGqyx4zCLXtUFOqvfv8L3g+8fpKJ1FsuxPyOYMF+4KufbUxKq1Oq",
    ]

    /// Verifies `message` against the text of a `.minisig` file. Returns the
    /// trusted comment on success.
    @discardableResult
    public static func verify(message: Data, signatureFile: String, publicKey: PublicKey) throws -> String {
        let lines = signatureFile.split(whereSeparator: \.isNewline).map(String.init)
        guard lines.count >= 4,
              lines[0].hasPrefix("untrusted comment:"),
              let sigRaw = Data(base64Encoded: lines[1]), sigRaw.count == 74,
              lines[2].hasPrefix("trusted comment: "),
              let globalSig = Data(base64Encoded: lines[3]), globalSig.count == 64
        else { throw Failure.badSignatureFile }
        let sig = Array(sigRaw)
        let alg = (sig[0], sig[1])
        let prehashed: Bool
        switch alg {
        case (0x45, 0x44): prehashed = true   // "ED"
        case (0x45, 0x64): prehashed = false  // "Ed"
        default: throw Failure.badSignatureFile
        }
        guard Array(sig[2..<10]) == publicKey.keyId else { throw Failure.keyMismatch }
        let signature = Data(sig[10..<74])
        let signed: Data = prehashed ? Data(Blake2b.hash(message)) : message
        guard publicKey.key.isValidSignature(signature, for: signed) else { throw Failure.invalidSignature }
        let trusted = String(lines[2].dropFirst("trusted comment: ".count))
        guard publicKey.key.isValidSignature(globalSig, for: signature + Data(trusted.utf8))
        else { throw Failure.invalidTrustedComment }
        return trusted
    }

    /// Verifies against any of the Freeweights keys.
    @discardableResult
    public static func verifyFreeweights(message: Data, signatureFile: String) throws -> String {
        var last: Error = Failure.keyMismatch
        for k in freeweightsKeys {
            do { return try verify(message: message, signatureFile: signatureFile, publicKey: PublicKey(base64: k)) }
            catch Failure.keyMismatch { last = Failure.keyMismatch; continue }
        }
        throw last
    }
}
