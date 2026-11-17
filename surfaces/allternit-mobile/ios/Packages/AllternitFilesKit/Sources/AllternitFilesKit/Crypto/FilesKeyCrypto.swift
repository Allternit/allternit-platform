import CryptoKit
import Foundation

/// The Files key hierarchy (SPEC section 13), byte for byte the engine's
/// `crypto.rs` and `replica.rs` (platform/files/allternit-files-engine):
///
/// - **FRK**: 32 random bytes per user. Kept on this device only as a
///   `KeyEnvelope` wrapped to this device's X25519 key; opened in memory
///   when a replica is read, never written unwrapped.
/// - **Subkeys**: HKDF-SHA256, no salt, `info` = `files-replica/1/<replica_id>`
///   (chunks) and `files-replica/1/<replica_id>/head` (the head).
/// - **Envelope** (`x25519-hkdf-sha256-xchacha20poly1305`): KEK =
///   HKDF-SHA256(salt = ephemeral_pub || device_pub, ikm = X25519 shared
///   secret, info = `files-envelope/1/<device_id>`), AAD `afe1|<key_id>|<device_id>`.
///
/// Vectors: `contracts/files-drive/vectors/keys-v1.json` (`KeyVectorTests`).
public struct FilesRootKey: Sendable {
    public let keyId: String
    let key: SymmetricKey

    public init(keyId: String, keyBytes: Data) {
        precondition(keyBytes.count == 32, "a Files root key is 32 bytes")
        self.keyId = keyId
        self.key = SymmetricKey(data: keyBytes)
    }

    /// A new FRK, made on the user's first device.
    public static func generate() -> FilesRootKey {
        let bytes = FilesKeyCrypto.random(32)
        let id = "fk_" + FilesKeyCrypto.hex(FilesKeyCrypto.random(12))
        return FilesRootKey(keyId: id, keyBytes: bytes)
    }

    /// HKDF-SHA256 (no salt) with `info` as the label.
    public func subkey(_ info: String) -> Data {
        FilesKeyCrypto.hkdf(ikm: key, salt: Data(), info: Data(info.utf8), length: 32)
    }

    var rawBytes: Data { key.withUnsafeBytes { Data($0) } }
}

public enum FilesKeyCrypto {
    public static let wrapAlg = "x25519-hkdf-sha256-xchacha20poly1305"
    public static let cipher = "xchacha20poly1305"
    /// A sealed head names the catalog parts, nothing else (`replica::MAX_HEAD_BYTES`).
    static let maxHeadBytes = 64 * 1024
    static let maxHeaderJSON = 4096

    // MARK: Primitives

    public static func hkdf(ikm: SymmetricKey, salt: Data, info: Data, length: Int) -> Data {
        HKDF<SHA256>.deriveKey(inputKeyMaterial: ikm, salt: salt, info: info, outputByteCount: length)
            .withUnsafeBytes { Data($0) }
    }

    public static func x25519(_ secret: Curve25519.KeyAgreement.PrivateKey, _ peerPub: Data) throws -> Data {
        let peer = try Curve25519.KeyAgreement.PublicKey(rawRepresentation: peerPub)
        return try secret.sharedSecretFromKeyAgreement(with: peer).withUnsafeBytes { Data($0) }
    }

    static func random(_ count: Int) -> Data {
        var bytes = [UInt8](repeating: 0, count: count)
        let status = SecRandomCopyBytes(kSecRandomDefault, count, &bytes)
        precondition(status == errSecSuccess, "no system randomness")
        return Data(bytes)
    }

    static func hex(_ data: Data) -> String {
        let digits = Array("0123456789abcdef")
        var out = ""
        out.reserveCapacity(data.count * 2)
        for b in data { out.append(digits[Int(b >> 4)]); out.append(digits[Int(b & 0x0F)]) }
        return out
    }

    static func unhex(_ text: String) -> Data? {
        guard text.count % 2 == 0 else { return nil }
        var out = Data(capacity: text.count / 2)
        var index = text.startIndex
        while index < text.endIndex {
            let next = text.index(index, offsetBy: 2)
            guard let byte = UInt8(text[index..<next], radix: 16) else { return nil }
            out.append(byte)
            index = next
        }
        return out
    }

    public static func base64url(_ data: Data) -> String {
        data.base64EncodedString().replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_").replacingOccurrences(of: "=", with: "")
    }

    public static func fromBase64url(_ text: String) -> Data? {
        var s = text.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
        while s.count % 4 != 0 { s.append("=") }
        return Data(base64Encoded: s)
    }

    // MARK: Ids and codes

    /// `rp_` + the first 24 hex digits of
    /// `blake3("files-replica-id/1|<source>|<root>|<host>")` (`replica::replica_id`).
    /// Both ends derive it, so a reader finds a replica without asking the
    /// (sleeping) source.
    public static func replicaId(sourceComputerId: String, rootId: String, hostDeviceId: String) -> String {
        let digest = Blake3Hasher.hashHex(Data("files-replica-id/1|\(sourceComputerId)|\(rootId)|\(hostDeviceId)".utf8))
        return "rp_" + digest.prefix(24)
    }

    /// The 6-digit code for a `KeyShareRequest`:
    /// `u32_be(blake3("files-key-confirm/1|" || device_pub)[0..4]) % 1000000`.
    /// The approving device computes it from the request's `device_pub`, so a
    /// swapped key shows a different code than the new device does.
    public static func confirmCode(devicePub: Data) -> String {
        var hasher = Blake3Hasher()
        hasher.update(Data("files-key-confirm/1|".utf8))
        hasher.update(devicePub)
        let b = hasher.finalizeBytes()
        let n = (UInt32(b[0]) << 24 | UInt32(b[1]) << 16 | UInt32(b[2]) << 8 | UInt32(b[3])) % 1_000_000
        return String(format: "%06u", n)
    }

    // MARK: Envelopes

    static func envelopeKEK(shared: Data, ephemeralPub: Data, devicePub: Data, deviceId: String) -> Data {
        hkdf(ikm: SymmetricKey(data: shared), salt: ephemeralPub + devicePub,
             info: Data("files-envelope/1/\(deviceId)".utf8), length: 32)
    }

    static func envelopeAAD(keyId: String, deviceId: String) -> Data {
        Data("afe1|\(keyId)|\(deviceId)".utf8)
    }

    /// Wraps the FRK to `devicePub` (another device's X25519 key, or this
    /// device's own on the first device) with a fresh ephemeral key.
    public static func wrap(_ frk: FilesRootKey, toDevicePub devicePub: Data, deviceId: String, createdAt: String) throws -> KeyEnvelope {
        let ephemeral = Curve25519.KeyAgreement.PrivateKey()
        let ephemeralPub = ephemeral.publicKey.rawRepresentation
        let shared = try x25519(ephemeral, devicePub)
        let kek = envelopeKEK(shared: shared, ephemeralPub: ephemeralPub, devicePub: devicePub, deviceId: deviceId)
        let nonce = random(XChaCha20Poly1305.nonceBytes)
        let wrapped = try XChaCha20Poly1305.seal(frk.rawBytes, key: kek, nonce: nonce,
                                                 aad: envelopeAAD(keyId: frk.keyId, deviceId: deviceId))
        return KeyEnvelope(keyId: frk.keyId, deviceId: deviceId, wrapAlg: wrapAlg, ephemeralPub: base64url(ephemeralPub),
                           nonce: base64url(nonce), wrappedKey: base64url(wrapped), createdAt: createdAt)
    }

    /// Opens an envelope with this device's secret. Throws `keyUnavailable`
    /// when it was wrapped to another key or was changed.
    public static func unwrap(_ envelope: KeyEnvelope, deviceSecret: Curve25519.KeyAgreement.PrivateKey) throws -> FilesRootKey {
        let bad = FilesError.keyUnavailable("The Files key envelope does not open on this device.")
        guard envelope.wrapAlg == wrapAlg,
              let ephemeralPub = fromBase64url(envelope.ephemeralPub), ephemeralPub.count == 32,
              let nonce = fromBase64url(envelope.nonce), nonce.count == XChaCha20Poly1305.nonceBytes,
              let wrapped = fromBase64url(envelope.wrappedKey) else { throw bad }
        let devicePub = deviceSecret.publicKey.rawRepresentation
        guard let shared = try? x25519(deviceSecret, ephemeralPub) else { throw bad }
        let kek = envelopeKEK(shared: shared, ephemeralPub: ephemeralPub, devicePub: devicePub, deviceId: envelope.deviceId)
        guard let key = try? XChaCha20Poly1305.open(wrapped, key: kek, nonce: nonce,
                                                    aad: envelopeAAD(keyId: envelope.keyId, deviceId: envelope.deviceId)),
              key.count == 32 else { throw bad }
        return FilesRootKey(keyId: envelope.keyId, keyBytes: key)
    }
}

// MARK: - Sealed chunks

/// Opens (and, for tests and future replica pushes, seals) chunks under one
/// replica subkey (`crypto::ChunkSealer`).
public struct ChunkSealer: Sendable {
    public let keyId: String
    let key: Data

    public init(frk: FilesRootKey, replicaId: String) {
        keyId = frk.keyId
        key = frk.subkey("files-replica/1/\(replicaId)")
    }

    /// `blake3::keyed_hash(subkey, raw plaintext hash)` as lower-case hex:
    /// the name the host stores the sealed chunk under.
    public func locator(plaintextHash: String) throws -> String {
        guard let raw = FilesKeyCrypto.unhex(plaintextHash), raw.count == 32 else {
            throw FilesError.invalidResponse("\(plaintextHash) is not a blake3 hash")
        }
        return Blake3Hasher.keyedHashHex(key: key, raw)
    }

    static func aad(_ h: EncryptedChunkHeader) -> Data {
        Data("afc1|\(h.v)|\(h.cipher)|\(h.keyId)|\(h.locator)|\(h.plaintextSize)".utf8)
    }

    /// `u32 big-endian header length | header JSON | ciphertext` (SPEC section 13).
    public static func decodeBlob(_ blob: Data) throws -> (EncryptedChunkHeader, Data) {
        let (json, rest) = try FramedBlob.split(blob, what: "sealed chunk")
        guard let header = try? JSONDecoder().decode(EncryptedChunkHeader.self, from: json) else {
            throw FilesError.integrity(expected: "sealed chunk", actual: "unreadable header")
        }
        return (header, rest)
    }

    /// Opens one blob and checks it is the plaintext named by `plaintextHash`
    /// (locator, AEAD tag, size and blake3 all checked).
    public func open(blob: Data, plaintextHash: String) throws -> Data {
        let (header, ciphertext) = try Self.decodeBlob(blob)
        guard header.v == 1, header.cipher == FilesKeyCrypto.cipher else {
            throw FilesError.integrity(expected: plaintextHash, actual: "unknown sealed format v\(header.v) \(header.cipher)")
        }
        guard header.keyId == keyId else {
            throw FilesError.keyUnavailable("This chunk was sealed with Files key \(header.keyId).")
        }
        guard header.locator == (try locator(plaintextHash: plaintextHash)),
              let nonce = FilesKeyCrypto.fromBase64url(header.nonce), nonce.count == XChaCha20Poly1305.nonceBytes,
              let plaintext = try? XChaCha20Poly1305.open(ciphertext, key: key, nonce: nonce, aad: Self.aad(header)),
              UInt64(plaintext.count) == header.plaintextSize else {
            throw FilesError.integrity(expected: plaintextHash, actual: "sealed chunk failed authentication")
        }
        let actual = Blake3Hasher.hashHex(plaintext)
        guard actual == plaintextHash else { throw FilesError.integrity(expected: plaintextHash, actual: actual) }
        return plaintext
    }

    /// Seals one chunk with a random nonce: `(locator, blob)`. `nonce` is for
    /// test vectors only.
    public func seal(_ plaintext: Data, nonce: Data? = nil) throws -> (String, Data) {
        let nonce = nonce ?? FilesKeyCrypto.random(XChaCha20Poly1305.nonceBytes)
        let header = EncryptedChunkHeader(v: 1, cipher: FilesKeyCrypto.cipher, keyId: keyId,
                                          nonce: FilesKeyCrypto.base64url(nonce),
                                          locator: try locator(plaintextHash: Blake3Hasher.hashHex(plaintext)),
                                          plaintextSize: UInt64(plaintext.count))
        let ciphertext = try XChaCha20Poly1305.seal(plaintext, key: key, nonce: nonce, aad: Self.aad(header))
        return (header.locator, FramedBlob.join(try JSONEncoder().encode(header), ciphertext))
    }
}

enum FramedBlob {
    static func split(_ blob: Data, what: String) throws -> (Data, Data) {
        let bytes = [UInt8](blob.prefix(4))
        guard bytes.count == 4 else { throw FilesError.integrity(expected: what, actual: "truncated") }
        let length = Int(UInt32(bytes[0]) << 24 | UInt32(bytes[1]) << 16 | UInt32(bytes[2]) << 8 | UInt32(bytes[3]))
        guard length <= FilesKeyCrypto.maxHeaderJSON, blob.count >= 4 + length else {
            throw FilesError.integrity(expected: what, actual: "damaged header")
        }
        let start = blob.startIndex
        return (Data(blob[(start + 4)..<(start + 4 + length)]), Data(blob[(start + 4 + length)...]))
    }

    static func join(_ json: Data, _ ciphertext: Data) -> Data {
        let n = UInt32(json.count)
        return Data([UInt8(n >> 24), UInt8((n >> 16) & 0xFF), UInt8((n >> 8) & 0xFF), UInt8(n & 0xFF)]) + json + ciphertext
    }
}

// MARK: - Replica head and catalog (replica.rs)

/// One item of the replicated root, as the source saw it (`replica::CatalogEntry`).
public struct ReplicaCatalogEntry: Codable, Sendable, Hashable {
    public var itemId: ItemId
    public var parentId: ItemId?
    public var name: String
    public var kind: ItemKind
    public var size: UInt64
    public var contentHash: Blake3?
    public var rev: Rev
    public var modifiedAt: String
    public var chunks: [ChunkRef]

    enum CodingKeys: String, CodingKey {
        case itemId = "item_id", parentId = "parent_id", name, kind, size
        case contentHash = "content_hash", rev, modifiedAt = "modified_at", chunks
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        itemId = try c.decode(ItemId.self, forKey: .itemId)
        parentId = try c.decodeIfPresent(ItemId.self, forKey: .parentId)
        name = try c.decode(String.self, forKey: .name)
        kind = try c.decode(ItemKind.self, forKey: .kind)
        size = try c.decode(UInt64.self, forKey: .size)
        contentHash = try c.decodeIfPresent(Blake3.self, forKey: .contentHash)
        rev = try c.decode(Rev.self, forKey: .rev)
        modifiedAt = try c.decode(String.self, forKey: .modifiedAt)
        chunks = try c.decodeIfPresent([ChunkRef].self, forKey: .chunks) ?? []
    }
}

/// The whole root at one cursor (`replica::Catalog`).
public struct ReplicaCatalog: Codable, Sendable {
    public var replicaId: String
    public var sourceComputerId: ComputerId
    public var rootId: RootId
    public var rootItemId: ItemId
    public var cursor: String?
    public var syncedAt: String
    public var items: [ReplicaCatalogEntry]

    enum CodingKeys: String, CodingKey {
        case replicaId = "replica_id", sourceComputerId = "source_computer_id", rootId = "root_id"
        case rootItemId = "root_item_id", cursor, syncedAt = "synced_at", items
    }

    public func entry(_ itemId: ItemId) -> ReplicaCatalogEntry? { items.first { $0.itemId == itemId } }
}

/// The sealed pointer a host keeps (`replica::Head`).
public struct ReplicaHead: Codable, Sendable, Hashable {
    public var v: UInt32
    public var replicaId: String
    public var cursor: String?
    public var syncedAt: String
    public var catalogHash: Blake3
    public var catalogSize: UInt64
    public var parts: [ChunkRef]

    enum CodingKeys: String, CodingKey {
        case v, replicaId = "replica_id", cursor, syncedAt = "synced_at"
        case catalogHash = "catalog_hash", catalogSize = "catalog_size", parts
    }
}

public enum ReplicaReader {
    private struct HeadEnvelope: Codable { var v: UInt32; var key_id: String; var nonce: String }

    /// The `key_id` a sealed head says it was sealed with (readable without a key).
    public static func headKeyId(_ bytes: Data) throws -> String {
        let (json, _) = try FramedBlob.split(bytes, what: "replica head")
        guard let env = try? JSONDecoder().decode(HeadEnvelope.self, from: json), env.v == 1 else {
            throw FilesError.integrity(expected: "replica head", actual: "damaged")
        }
        return env.key_id
    }

    /// `replica::open_head`: subkey `files-replica/1/<id>/head`, AAD `afh1|1|<id>|<key_id>`.
    public static func openHead(_ bytes: Data, frk: FilesRootKey, replicaId: String) throws -> ReplicaHead {
        guard bytes.count <= FilesKeyCrypto.maxHeadBytes else { throw FilesError.integrity(expected: "replica head", actual: "too big") }
        let (json, ciphertext) = try FramedBlob.split(bytes, what: "replica head")
        guard let env = try? JSONDecoder().decode(HeadEnvelope.self, from: json), env.v == 1 else {
            throw FilesError.integrity(expected: "replica head", actual: "damaged")
        }
        guard env.key_id == frk.keyId else { throw FilesError.keyUnavailable("This replica was sealed with Files key \(env.key_id).") }
        let key = frk.subkey("files-replica/1/\(replicaId)/head")
        guard let nonce = FilesKeyCrypto.fromBase64url(env.nonce), nonce.count == XChaCha20Poly1305.nonceBytes,
              let plain = try? XChaCha20Poly1305.open(ciphertext, key: key, nonce: nonce, aad: Data("afh1|1|\(replicaId)|\(env.key_id)".utf8)),
              let head = try? JSONDecoder().decode(ReplicaHead.self, from: plain) else {
            throw FilesError.integrity(expected: "replica head", actual: "failed authentication")
        }
        guard head.replicaId == replicaId else { throw FilesError.integrity(expected: replicaId, actual: head.replicaId) }
        return head
    }

    /// `replica::read_catalog`: every part re-hashed, then the whole.
    /// `fetch(locator)` returns the sealed blob, or nil when the host lacks it.
    public static func readCatalog(head: ReplicaHead, frk: FilesRootKey, maxBytes: UInt64,
                                   fetch: (String) async throws -> Data?) async throws -> ReplicaCatalog {
        guard head.catalogSize <= maxBytes else {
            throw FilesError.replicaUnavailable("This replica's file list is too big to open here. Open it in the Allternit app.")
        }
        let sealer = ChunkSealer(frk: frk, replicaId: head.replicaId)
        var json = Data(capacity: Int(head.catalogSize))
        for part in head.parts {
            guard let blob = try await fetch(try sealer.locator(plaintextHash: part.hash)) else {
                throw FilesError.replicaUnavailable("A part of this replica's file list is missing on the host.")
            }
            json.append(try sealer.open(blob: blob, plaintextHash: part.hash))
        }
        let actual = Blake3Hasher.hashHex(json)
        guard actual == head.catalogHash else { throw FilesError.integrity(expected: head.catalogHash, actual: actual) }
        guard let catalog = try? JSONDecoder().decode(ReplicaCatalog.self, from: json), catalog.replicaId == head.replicaId else {
            throw FilesError.integrity(expected: head.catalogHash, actual: "unreadable catalog")
        }
        return catalog
    }
}
